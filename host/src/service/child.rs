//! A child process whose stdout arrives as lines and whose stderr arrives as
//! raw chunks, read on one thread per pipe; stdin is written on a thread of
//! its own, so a consumer never blocks writing while the child waits for its
//! output to be read. Every runner spawns its harness
//! through [`LineChild::spawn`], which goes through [`ProcessSlot::spawn`] so
//! cancel and timeout reach the child's whole process group.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};

use super::harness_api::{ProcessSlot, has_exited};

/// The longest stdout line delivered; the rest of a longer line is dropped.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The largest stderr chunk delivered at once.
const STDERR_CHUNK: usize = 64 * 1024;
/// How many lines or chunks may wait between the readers and the consumer.
/// A full queue blocks the readers, so the pipes fill and a child that
/// writes faster than its output is consumed blocks too: a backlog stays
/// within this many messages (16 MiB of stdout at most) plus what the pipes
/// hold, however much the child writes.
const QUEUE_CAPACITY: usize = 16;

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
    /// Reaps the child, so it forgets the group once the id may be reused.
    slot: ProcessSlot,
    /// The exit status, once reaped.
    status: Option<ExitStatus>,
    /// Feeds the stdin writer thread; dropping it closes the child's stdin
    /// once what was queued is written.
    stdin: Option<Sender<Vec<u8>>>,
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
        let stdin = child.stdin.take().map(writer);
        let (sender, lines) = mpsc::sync_channel(QUEUE_CAPACITY);
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
            slot: slot.clone(),
            status: None,
            stdin,
            lines,
        })
    }

    /// Queue `line` and a `\n` for the child's stdin, without waiting for
    /// the child to read it: a child that writes output while it reads its
    /// input can block on the bounded output queue until the consumer
    /// drains it, so writing here must never wait on the child. Fails with
    /// [`io::ErrorKind::BrokenPipe`] once stdin is closed or an earlier
    /// write failed, as when the child has exited.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.stdin
            .as_ref()
            .and_then(|stdin| stdin.send(bytes).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "the child's stdin is closed"))
    }

    /// Close the child's stdin, so it reads end of file after what was
    /// already queued.
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

    /// Whether the child has exited, without reaping it.
    pub fn has_exited(&self) -> bool {
        self.status.is_some() || has_exited(&self.child).unwrap_or(true)
    }

    /// The child's exit status if it has exited. An exited child's process
    /// group is first sent SIGKILL, which ends anything it left running in
    /// the background holding the pipes open; the unreaped leader still
    /// holds the group's id then, so the signal cannot reach a reused one.
    /// The child is then reaped and the slot forgets its group.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        if !has_exited(&self.child)? {
            return Ok(None);
        }
        let _ = kill_process_group(Pid::from_child(&self.child), Signal::KILL);
        self.status = self.slot.try_reap(&mut self.child)?;
        Ok(self.status)
    }

    /// Close stdin and wait for the child to exit, then reap it and forget
    /// its group in the slot. Output not yet consumed is discarded first:
    /// the readers stop and close the pipes, so a child still writing sees a
    /// broken pipe rather than blocking on a full one forever.
    pub fn wait(self) -> io::Result<ExitStatus> {
        let Self {
            mut child,
            slot,
            status,
            stdin,
            lines,
        } = self;
        drop(stdin);
        drop(lines);
        match status {
            Some(status) => Ok(status),
            None => slot.reap(&mut child),
        }
    }
}

/// Write what arrives on the returned sender to `stdin` on a thread of its
/// own. The thread closes stdin once the sender is dropped and the queue is
/// written, or at the first failed write, after which sends fail. A child
/// that never reads leaves the thread blocked until it exits, when the write
/// fails with a broken pipe.
fn writer(mut stdin: ChildStdin) -> Sender<Vec<u8>> {
    let (sender, queue) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        for bytes in queue {
            if stdin
                .write_all(&bytes)
                .and_then(|()| stdin.flush())
                .is_err()
            {
                return;
            }
        }
    });
    sender
}

/// Run `read` on its own thread; the last pipe to finish sends `Closed`.
/// A send blocks while the queue is full and fails once the receiver is
/// gone, which ends the thread and closes its pipe.
fn reader(
    sender: SyncSender<ChildLine>,
    open: Arc<AtomicUsize>,
    read: impl FnOnce(&dyn Fn(ChildLine) -> bool) + Send + 'static,
) {
    thread::spawn(move || {
        read(&|line| sender.send(line).is_ok());
        finish(&sender, &open);
    });
}

fn finish(sender: &SyncSender<ChildLine>, open: &AtomicUsize) {
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
        // `sleep` is forked before "started" is printed: a kill racing the
        // fork could miss the new process and leave it holding the pipes.
        let child =
            LineChild::spawn(&slot, sh("sleep 30 & echo started; wait; echo late")).unwrap();
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

    /// The last counter `name` holds, written by a flooding child.
    fn produced(dir: &std::path::Path, name: &str) -> usize {
        // The child rewrites the file each line, so a read may catch it empty.
        loop {
            if let Ok(text) = std::fs::read_to_string(dir.join(name))
                && let Ok(count) = text.trim().parse()
            {
                return count;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// A child writing 1000-byte lines to both pipes as fast as it can,
    /// counting them in `out` and `err`, gets ahead of a slow consumer by no
    /// more than the queue, the pipes and the readers' buffers hold.
    #[test]
    fn a_flooding_child_is_held_back_by_a_slow_consumer() {
        let slot = ProcessSlot::default();
        let dir = tempfile::tempdir().unwrap();
        let mut command = sh("line=$(printf '%999s' '' | tr ' ' x); \
             (i=0; while :; do i=$((i+1)); echo \"$line\" >&2; echo $i > err; done) & \
             i=0; while :; do i=$((i+1)); echo \"$line\"; echo $i > out; done");
        command.current_dir(dir.path());
        let child = LineChild::spawn(&slot, command).unwrap();
        let (mut lines, mut stderr_bytes) = (0, 0);
        let slow = Instant::now() + Duration::from_secs(2);
        while Instant::now() < slow {
            match child.next(Duration::from_millis(50)) {
                Some(ChildLine::Stdout(_)) => lines += 1,
                Some(ChildLine::Stderr(bytes)) => stderr_bytes += bytes.len(),
                Some(ChildLine::Closed) => panic!("the flooding child closed its pipes"),
                None => {}
            }
            thread::sleep(Duration::from_millis(20));
        }
        let (out, err) = (produced(dir.path(), "out"), produced(dir.path(), "err"));
        slot.kill();
        // Queue, a 64 KiB pipe, the 8 KiB line buffer and the line in hand.
        let out_bound = QUEUE_CAPACITY + 64 + 8 + 2;
        assert!(
            out <= lines + out_bound,
            "stdout ran {} lines ahead of the consumer",
            out - lines
        );
        // Queue of 64 KiB chunks, the pipe and the chunk in hand.
        let err_bound = (QUEUE_CAPACITY + 1) * 64 + 64 + 2;
        let consumed = stderr_bytes / 1000;
        assert!(
            err <= consumed + err_bound,
            "stderr ran {} lines ahead of the consumer",
            err - consumed
        );
        let lines = collect(&child);
        assert!(lines.len() <= 2 * QUEUE_CAPACITY + 200, "{}", lines.len());
        assert!(!child.wait().unwrap().success());
    }

    /// Killing a child that floods both pipes, while nothing reads them,
    /// ends its output promptly instead of after a backlog.
    #[test]
    fn a_killed_flooding_child_closes_promptly() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(&slot, sh("yes >&2 & echo started; yes")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "the child never started");
            if child.next(Duration::from_millis(50)) == Some(ChildLine::Stdout("started".into())) {
                break;
            }
        }
        thread::sleep(Duration::from_millis(500));
        let killed = Instant::now();
        slot.kill();
        let lines = collect(&child);
        assert!(
            killed.elapsed() < Duration::from_secs(5),
            "{:?}",
            killed.elapsed()
        );
        assert_eq!(lines.last(), Some(&ChildLine::Closed));
        assert!(
            lines.len() <= 2 * QUEUE_CAPACITY + 64 * 1024,
            "{}",
            lines.len()
        );
        assert!(!child.wait().unwrap().success());
    }

    /// Waiting on a child that is still flooding its pipes, with nothing
    /// reading them, stops the readers so the child sees a broken pipe and
    /// the wait ends.
    #[test]
    fn waiting_on_an_unread_flooding_child_ends() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(&slot, sh("yes >&2 & yes")).unwrap();
        thread::sleep(Duration::from_millis(300));
        let (done, waited) = mpsc::channel();
        thread::spawn(move || {
            let _ = done.send(child.wait());
        });
        let status = waited.recv_timeout(Duration::from_secs(5));
        slot.kill();
        assert!(status.is_ok(), "the wait never ended");
    }

    /// A child that echoes its input as it reads it, like `cat`, takes a
    /// prompt far larger than the queue and the pipes hold without the
    /// writes blocking: the consumer drains its output while the prompt is
    /// still going in.
    #[test]
    fn a_large_prompt_to_an_echoing_child_does_not_block() {
        let slot = ProcessSlot::default();
        let mut child = LineChild::spawn(&slot, sh("exec cat")).unwrap();
        let watchdog = slot.clone();
        let (stop, stopped) = mpsc::channel::<()>();
        thread::spawn(move || {
            if stopped.recv_timeout(Duration::from_secs(5)).is_err() {
                watchdog.kill();
            }
        });
        let count = 30_000;
        let started = Instant::now();
        for i in 0..count {
            child.write_line(&format!("line {i:05}")).unwrap();
        }
        child.close_stdin();
        let written = started.elapsed();
        let lines = collect(&child);
        let _ = stop.send(());
        assert!(written < Duration::from_secs(4), "writing took {written:?}");
        let lines = stdout(&lines);
        assert_eq!(lines.len(), count);
        assert_eq!(lines[count - 1], format!("line {:05}", count - 1));
        assert!(child.wait().unwrap().success());
    }

    /// Once its leader is reaped, a group's id can be reused, so the slot
    /// forgets it and a later kill signals nothing.
    #[test]
    fn reaping_the_child_forgets_its_group() {
        let slot = ProcessSlot::default();
        let child = LineChild::spawn(&slot, sh("true")).unwrap();
        assert!(slot.group().is_some());
        assert!(child.wait().unwrap().success());
        assert_eq!(slot.group(), None);

        let mut child = LineChild::spawn(&slot, sh("exit 3")).unwrap();
        assert!(slot.group().is_some());
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the child never exited");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(3));
        assert_eq!(slot.group(), None);
        // A later kill still refuses new spawns.
        slot.kill();
        assert!(slot.was_killed());
    }

    /// `try_wait` ends what the exited child left running in its group.
    #[test]
    fn try_wait_after_exit_kills_the_rest_of_the_group() {
        let slot = ProcessSlot::default();
        let mut child = LineChild::spawn(&slot, sh("sleep 30 & echo $!")).unwrap();
        let group = slot.group().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the child never exited");
            thread::sleep(Duration::from_millis(10));
        }
        let pid = Pid::from_raw(group).unwrap();
        while rustix::process::test_kill_process_group(pid).is_ok() {
            assert!(Instant::now() < deadline, "the background sleep survived");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
