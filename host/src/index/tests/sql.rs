//! A rusqlite reader for `index.db` in tests, replacing the `python3`
//! readers. Every call opens its own read-only connection and drops it
//! before returning, so no handle outlives the call. On Windows an open
//! handle would make the tempdir cleanup fail (the drop-order rule).

pub(crate) use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Open `db` read-only. Never creates the file, and never writes to it.
fn open_read_only(db: &Path) -> Connection {
    Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap_or_else(|error| panic!("open {} read-only: {error}", db.display()))
}

/// Every row of `query` against `db`, each as its column values.
pub(crate) fn sql_rows(db: &Path, query: &str) -> Vec<Vec<Value>> {
    try_rows(db, query).unwrap_or_else(|error| panic!("{query:?}: {error}"))
}

fn try_rows(db: &Path, query: &str) -> rusqlite::Result<Vec<Vec<Value>>> {
    let connection = open_read_only(db);
    let rows = {
        let mut statement = connection.prepare(query)?;
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
    };
    drop(connection);
    rows
}

/// SQLite's own verdict on `db`: `"ok"` when both `PRAGMA
/// integrity_check` and `PRAGMA quick_check` pass, otherwise the first
/// failing check's messages, one per line. A file too damaged for the
/// check to run reports SQLite's error instead.
pub(crate) fn integrity(db: &Path) -> String {
    for pragma in ["PRAGMA integrity_check", "PRAGMA quick_check"] {
        let rows = match try_rows(db, pragma) {
            Ok(rows) => rows,
            Err(error) => return error.to_string(),
        };
        let messages = rows
            .into_iter()
            .map(|row| match row.as_slice() {
                [Value::Text(message)] => message.clone(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>();
        if messages != ["ok"] {
            return messages.join("\n");
        }
    }
    "ok".into()
}

/// Whether `db` passes both SQLite integrity checks.
pub(crate) fn integrity_ok(db: &Path) -> bool {
    integrity(db) == "ok"
}

/// One value as Python's `print` showed it, so assertions written
/// against the old `python3` readers keep their exact text.
pub(crate) fn display(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Integer(number) => number.to_string(),
        Value::Real(number) => format!("{number:?}"),
        Value::Text(text) => text.clone(),
        Value::Blob(bytes) => format!("{bytes:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample(dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join("sample.db");
        let connection = Connection::open(&path).unwrap();
        // Plain `execute`, not `execute_batch`: the index's clippy rules
        // keep batch and transaction calls inside `index/db.rs`.
        for statement in [
            "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, note TEXT)",
            "INSERT INTO t(name, note) VALUES ('first', NULL), ('second', 'x')",
        ] {
            connection.execute(statement, []).unwrap();
        }
        drop(connection);
        path
    }

    #[test]
    fn rows_come_back_as_values_in_query_order() {
        let dir = TempDir::new().unwrap();
        let path = sample(&dir);
        assert_eq!(
            sql_rows(&path, "SELECT id, name, note FROM t ORDER BY id"),
            vec![
                vec![Value::Integer(1), Value::Text("first".into()), Value::Null],
                vec![
                    Value::Integer(2),
                    Value::Text("second".into()),
                    Value::Text("x".into())
                ],
            ]
        );
        assert!(integrity_ok(&path));
        // The reader released its handle, so the file can be removed now
        // (Windows refuses while a handle is open).
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_reader_never_creates_or_writes_the_database() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.db");
        let opened = std::panic::catch_unwind(|| sql_rows(&missing, "SELECT 1"));
        assert!(opened.is_err());
        assert!(!missing.exists());

        let path = sample(&dir);
        let before = fs::read(&path).unwrap();
        let written =
            std::panic::catch_unwind(|| sql_rows(&path, "INSERT INTO t(name) VALUES ('third')"));
        assert!(written.is_err(), "a read-only reader accepted a write");
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_corrupt_database_fails_the_integrity_check() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("corrupt.db");
        let connection = Connection::open(&path).unwrap();
        for statement in ["CREATE TABLE t(v TEXT)", "CREATE INDEX t_by_v ON t(v)"] {
            connection.execute(statement, []).unwrap();
        }
        for n in 0..2000 {
            connection
                .execute("INSERT INTO t(v) VALUES (?1)", [format!("value-{n:06}")])
                .unwrap();
        }
        drop(connection);
        let mut bytes = fs::read(&path).unwrap();
        let page = 4096;
        assert!(bytes.len() > 4 * page);
        // Scribble over the middle of the third page, past the header.
        for byte in &mut bytes[2 * page + 100..3 * page - 100] {
            *byte = 0x5a;
        }
        fs::write(&path, bytes).unwrap();
        assert!(!integrity_ok(&path));
        assert_ne!(integrity(&path), "ok");
    }

    #[test]
    fn values_display_as_the_python_readers_printed_them() {
        assert_eq!(display(&Value::Null), "None");
        assert_eq!(display(&Value::Integer(155)), "155");
        assert_eq!(display(&Value::Text("within".into())), "within");
    }
}
