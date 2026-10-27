//! A child process whose stdout arrives as lines and whose stderr arrives as
//! raw chunks, read on one thread per pipe. Every runner spawns its harness
//! through [`LineChild::spawn`], which goes through [`ProcessSlot::spawn`] so
//! cancel and timeout reach the child's whole process group.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};

use super::harness_api::ProcessSlot;

/// The longest stdout line delivered; the rest of a longer line is dropped.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The largest stderr chunk delivered at once.
const STDERR_CHUNK: usize = 64 * 1024;

/// Something the child wrote, or the end of its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildLine {
    /// One stdout line without its `\n`, decoded lossily.
    Stdout(String),
    /// A chunk of stderr, at most 64 KiB.
    Stderr(Vec<u8>),
    /// Both pipes reached end of file. Sent once.
    Closed,
}

/// A spawned child with piped stdio. See the module documentation.
pub struct LineChild {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<ChildLine>,
}

impl LineChild {
    /// Spawn `command` with all three standard streams piped, through
    /// `slot`. After [`ProcessSlot::kill`] this fails with
    /// [`io::ErrorKind::Interrupted`].
    pub fn spawn(slot: &ProcessSlot, mut command: Command) -> io::Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = slot.spawn(&mut command)?;
        let stdin = child.stdin.take();
        let (sender, lines) = mpsc::channel();
        let open = Arc::new(AtomicUsize::new(2));
        match child.stdout.take() {
            Some(stdout) => reader(sender.clone(), Arc::clone(&open), move |send| {
                read_lines(stdout, send)
            }),
            None => finish(&sender, &open),
        }
        match child.stderr.take() {
            Some(stderr) => reader(sender.clone(), Arc::clone(&open), move |send| {
                read_chunks(stderr, send)
            }),
            None => finish(&sender, &open),
        }
        Ok(Self {
            child,
            stdin,
            lines,
        })
    }

    /// Write `line` and a `\n` to the child's stdin.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the child's stdin is closed",
            ));
        };
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    /// Close the child's stdin, so it reads end of file.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The next line, chunk or `Closed`, or `None` when nothing arrives
    /// within `timeout`. After `Closed` this always waits out `timeout`, so a
    /// caller polling for the exit does not spin.
    pub fn next(&self, timeout: Duration) -> Option<ChildLine> {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                thread::sleep(timeout);
                None
            }
        }
    }

    /// The child's exit status if it has exited.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// SIGKILL the child's process group, which also ends anything it left
    /// running in the background holding the pipes open.
    pub fn kill_group(&self) {
        let _ = kill_process_group(Pid::from_child(&self.child), Signal::KILL);
    }

    /// Close stdin and wait for the child to exit.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        self.close_stdin();
        self.child.wait()
    }
}

/// Run `read` on its own thread; the last pipe to finish sends `Closed`.
fn reader(
    sender: Sender<ChildLine>,
    open: Arc<AtomicUsize>,
    read: impl FnOnce(&dyn Fn(ChildLine) -> bool) + Send + 'static,
) {
    thread::spawn(move || {
        read(&|line| sender.send(line).is_ok());
        finish(&sender, &open);
    });
}

fn finish(sender: &Sender<ChildLine>, open: &AtomicUsize) {
    if open.fetch_sub(1, Ordering::AcqRel) == 1 {
        let _ = sender.send(ChildLine::Closed);
    }
}

/// Send each `\n`-terminated line (and a final unterminated one), at most
/// [`MAX_LINE_BYTES`] of it, until end of file or a read error.
fn read_lines(pipe: impl Read, send: &dyn Fn(ChildLine) -> bool) {
    let mut reader = BufReader::new(pipe);
    let mut line = Vec::new();
    let mut overlong = false;
    loop {
        let buffer = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(buffer) => buffer,
        };
        let (taken, ended) = match buffer.iter().position(|&byte| byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (buffer.len(), false),
        };
        let content = &buffer[..taken - usize::from(ended)];
        if !overlong {
            let room = MAX_LINE_BYTES - line.len();
            if content.len() > room {
                line.extend_from_slice(&content[..room]);
                overlong = true;
            } else {
                line.extend_from_slice(content);
            }
        }
        reader.consume(taken);
        if ended {
            if !send(ChildLine::Stdout(decode(&mut line, overlong))) {
                return;
            }
            overlong = false;
        }
    }
    if !line.is_empty() {
        send(ChildLine::Stdout(decode(&mut line, overlong)));
    }
}

/// Take `line` as text. A cut line first loses a character split by the
/// cut, and the text never exceeds [`MAX_LINE_BYTES`].
fn decode(line: &mut Vec<u8>, cut: bool) -> String {
    let mut bytes = std::mem::take(line);
    if cut && bytes.len() == MAX_LINE_BYTES {
        // Back off to the lead byte of a character the cut may have split,
        // unless that character ends exactly at the cut.
        let lead = bytes
            .iter()
            .rposition(|&byte| byte & 0b1100_0000 != 0b1000_0000)
            .unwrap_or(0);
        if bytes.len() - lead < 4 && std::str::from_utf8(&bytes[lead..]).is_err() {
            bytes.truncate(lead);
        }
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_LINE_BYTES {
        let mut end = MAX_LINE_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Send stderr in chunks of at most 64 KiB until end of file or an error.
fn read_chunks(mut pipe: impl Read, send: &dyn Fn(ChildLine) -> bool) {
    let mut chunk = vec![0_u8; STDERR_CHUNK];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if !send(ChildLine::Stderr(chunk[..read].to_vec())) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sh(body: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(body).env("LC_ALL", "C");
        command
    }

    /// Everything the child sends until `Closed`, which is included.
    fn collect(child: &LineChild) -> Vec<ChildLine> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut lines = Vec::new();
        while Instant::now() < deadline {
            if let Some(line) = child.next(Duration::from_millis(50)) {
                let closed = line == ChildLine::Closed;
                lines.push(line);
                if closed {
                    return lines;
                }
            }
        }
        panic!("no Closed within the deadline: {} lines", lines.len());
    }

    fn stdout(lines: &[ChildLine]) -> Vec<&str> {
        lines
            .iter()
            .filter_map(|line| match line {
                ChildLine::Stdout(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn stderr(lines: &[ChildLine]) -> Vec<u8> {
        lines
            .iter()
            .filter_map(|line| match line {
                ChildLine::Stderr(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn stdout_lines_split_across_writes_arrive_whole() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(
            &slot,
            sh("printf 'hel'; sleep 0.1; printf 'lo\\nwor'; sleep 0.1; printf 'ld\\n'; printf tail"),
        )
        .unwrap();
        let lines = collect(&child);
        assert_eq!(stdout(&lines), ["hello", "world", "tail"]);
        assert_eq!(lines.last(), Some(&ChildLine::Closed));
        assert_eq!(
            lines
                .iter()
                .filter(|line| **line == ChildLine::Closed)
                .count(),
            1
        );
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn overlong_line_is_truncated_and_next_line_survives() {
        let slot = ProcessSlot::default();
        // 'x' then two-byte characters, so the byte cap falls inside one.
        let count = MAX_LINE_BYTES / 2 + 1000;
        let child = LineChild::spawn(
            &slot,
            sh(&format!(
                "printf x; yes 'é' | head -n {count} | tr -d '\\n'; printf '\\nnext\\n'"
            )),
        )
        .unwrap();
        let lines = collect(&child);
        let lines = stdout(&lines);
        assert_eq!(lines.len(), 2, "{:?}", lines.iter().map(|l| l.len()));
        let long = lines[0];
        assert_eq!(long.len(), MAX_LINE_BYTES - 1);
        assert!(long.starts_with("xé"));
        assert!(long.ends_with('é'));
        assert!(!long.contains('\u{FFFD}'));
        assert_eq!(lines[1], "next");
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn stderr_is_separate_from_stdout() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(&slot, sh("echo out; echo err >&2; echo more")).unwrap();
        let lines = collect(&child);
        assert_eq!(stdout(&lines), ["out", "more"]);
        assert_eq!(stderr(&lines), b"err\n");
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn write_line_reaches_child_stdin() {
        let slot = ProcessSlot::default();
        let mut child = LineChild::spawn(
            &slot,
            sh("read first; echo \"got:$first\"; read second; echo \"got:$second\"; cat; echo eof"),
        )
        .unwrap();
        child.write_line("ping").unwrap();
        child.write_line("{\"id\":2}").unwrap();
        child.close_stdin();
        let lines = collect(&child);
        assert_eq!(stdout(&lines), ["got:ping", "got:{\"id\":2}", "eof"]);
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn killed_slot_yields_closed() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(&slot, sh("echo started; sleep 30; echo late")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "the child never started");
            if child.next(Duration::from_millis(50)) == Some(ChildLine::Stdout("started".into())) {
                break;
            }
        }
        let killed = Instant::now();
        slot.kill();
        let lines = collect(&child);
        assert_eq!(lines, [ChildLine::Closed]);
        assert!(killed.elapsed() < Duration::from_secs(5));
        assert!(!child.wait().unwrap().success());
        assert!(
            LineChild::spawn(&slot, sh("true"))
                .is_err_and(|e| e.kind() == io::ErrorKind::Interrupted)
        );
    }
}
