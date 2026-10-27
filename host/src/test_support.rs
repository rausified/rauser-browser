//! Subprocess tools used by host tests, never by the production host.

use std::process::Command;

/// setup-python exposes `python` on Windows; `python3` is the Unix command.
/// Use UTF-8 for redirected output too, so SQLite content is not encoded with
/// a Windows console code page. No shell or launcher alias is involved.
pub(crate) fn python() -> Command {
    let mut command = Command::new(if cfg!(windows) { "python" } else { "python3" });
    command.args(["-X", "utf8"]);
    command
}

#[test]
fn sqlite_subprocess_preserves_unicode_paths_and_text() {
    let folder = tempfile::tempdir().unwrap();
    let database = folder.path().join("索引 café.db");
    let output = python()
        .args([
            "-c",
            "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('create table notes (title text)'); c.execute('insert into notes values (?)', (sys.argv[2],)); c.commit(); print(c.execute('select title from notes').fetchone()[0])",
        ])
        .arg(&database)
        .arg("Ticket ✓ — 文")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "Ticket ✓ — 文"
    );
    assert!(database.is_file());
}
