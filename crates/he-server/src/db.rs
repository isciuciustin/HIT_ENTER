//! SQLite plumbing: opening the file, applying migrations, and the two
//! primitives every table needs.
//!
//! Nothing here knows what a user or an invite is. The point of the module is
//! that the *pragmas* live in one place — a second connection opened with
//! `foreign_keys` off would silently stop enforcing the schema's cascades.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use uuid::Uuid;

use crate::error::Result;

/// Embedded at compile time, so a released binary carries its own schema and
/// there is no directory to ship alongside it.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// SQLite is single-writer; more connections buy concurrent *reads* and
/// nothing else. A handful is plenty for a space of friends, and keeping the
/// number small keeps lock contention legible.
const MAX_CONNECTIONS: u32 = 5;

/// How long a writer waits for another writer before giving up. Generous: on a
/// laptop that just woke up, the alternative to waiting is a spurious error.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Opens (creating if needed) the database at `path` and migrates it forward.
pub async fn open(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // WAL lets readers run while a writer holds the lock, which is the
        // whole reason a chat server is usable on one file.
        .journal_mode(SqliteJournalMode::Wal)
        // The schema's ON DELETE CASCADE is not advisory. SQLite defaults this
        // to *off*, per connection.
        .foreign_keys(true)
        // With WAL, NORMAL risks losing the last commits to a power cut but
        // never corrupts the database. Correct trade for chat messages.
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT);

    let pool = SqlitePoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;
    restrict_permissions(path);

    Ok(pool)
}

/// Makes the database readable only by its owner.
///
/// `server_meta.secret_key` lives in this file, and PLAN §11 says to treat it
/// like an SSH host key: 0600 on disk. Messages are plaintext in here too
/// (PLAN §10), which is a documented trust model for *the host* — not for
/// every other account on a shared machine.
///
/// Best effort: on a filesystem that cannot express this (a mounted vfat
/// stick, Windows) the server still runs, because failing to boot over file
/// permissions would help nobody.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // WAL and shared-memory files hold committed data too, and SQLite
        // creates them alongside the database.
        for suffix in ["", "-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            if sidecar.exists()
                && let Err(err) =
                    std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600))
            {
                tracing::warn!(path = %sidecar.display(), %err, "could not restrict database permissions");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A fresh UUIDv7, formatted the way every ID column stores it.
///
/// v7 puts a millisecond timestamp in the high bits, so the hex string sorts
/// chronologically: `ORDER BY id` is `ORDER BY time`, and a cursor is just an
/// ID (PLAN §8).
pub fn new_id() -> String {
    Uuid::now_v7().to_string()
}

/// Unix seconds, the unit of every `*_at` column.
///
/// Before 1970 the clock is broken in a way we cannot fix here, so it clamps
/// rather than failing a registration over it.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_chronologically() {
        let mut ids: Vec<String> = (0..64).map(|_| new_id()).collect();
        let generated = ids.clone();
        ids.sort();
        assert_eq!(
            ids, generated,
            "UUIDv7 IDs must already be in creation order — pagination cursors \
             and `ORDER BY id` both depend on it"
        );
    }

    #[tokio::test]
    async fn migrations_apply_to_an_empty_file() -> Result<()> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let pool = open(&path).await?;

        // Re-running is a no-op: every start of the server does exactly this.
        MIGRATOR.run(&pool).await?;

        let (foreign_keys,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await?;
        assert_eq!(foreign_keys, 1, "cascades are part of the schema");

        let (journal,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await?;
        assert_eq!(journal.to_lowercase(), "wal");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the file holds the server's secret key — PLAN §11"
            );
        }

        Ok(())
    }
}
