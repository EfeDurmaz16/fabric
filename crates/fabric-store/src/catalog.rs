//! The `SQLite` catalog of snapshots and objects.
//!
//! Invariant: a `snapshot_chunks` row is written only after the object it
//! names is durable on disk, in one transaction with its snapshot row, and
//! `chunks.refcount` equals the number of `snapshot_chunks` rows that name
//! the chunk. A snapshot's `bundle_base` always names a row, because publish
//! refuses a base it does not have and forget refuses a base a later
//! snapshot still needs. `SQLite` is the authority for what is published; an
//! object file with no row is invisible to readers and belongs to gc.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fabric_core::id::{ChunkId, JobId, SnapshotId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::checkpoint;
use crate::durable;
use crate::objects::{ChunkParams, ObjectError};

/// The catalog schema this crate reads and writes.
pub const SCHEMA_VERSION: i64 = 1;

/// Why the catalog refused.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// `SQLite` reported an error.
    #[error("catalog: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The catalog was written by another schema version.
    #[error("catalog schema version {found}, this crate reads {expected}")]
    SchemaVersion {
        /// The version in the file.
        found: i64,
        /// The version this crate reads.
        expected: i64,
    },
    /// A chunking parameter is missing from the catalog or does not parse.
    #[error("catalog setting {key} is missing or malformed")]
    Setting {
        /// The setting's key.
        key: &'static str,
    },
    /// The recorded chunking parameters break the chunker's rules.
    #[error(transparent)]
    Params(ObjectError),
    /// A filesystem call on the catalog file failed.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

const SCHEMA: &str = "
CREATE TABLE schema_version (version INTEGER NOT NULL);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE chunks (
  id BLOB PRIMARY KEY,
  stored_len INTEGER NOT NULL,
  refcount INTEGER NOT NULL,
  unreferenced_since INTEGER
) WITHOUT ROWID;
CREATE TABLE snapshots (
  id BLOB PRIMARY KEY,
  job_id BLOB NOT NULL,
  parent BLOB,
  bundle_base BLOB,
  created_at INTEGER NOT NULL,
  manifest_len INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE snapshot_chunks (
  snapshot_id BLOB NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
  role INTEGER NOT NULL,
  ordinal INTEGER NOT NULL,
  chunk_id BLOB NOT NULL REFERENCES chunks(id),
  PRIMARY KEY (snapshot_id, role, ordinal)
) WITHOUT ROWID;
CREATE INDEX snapshot_chunks_by_chunk ON snapshot_chunks(chunk_id);
";

/// One snapshot the catalog knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    /// The snapshot id: blake3 of the manifest bytes.
    pub id: SnapshotId,
    /// The job the snapshot belongs to.
    pub job_id: JobId,
    /// The snapshot it follows.
    pub parent: Option<SnapshotId>,
    /// The snapshot whose refs its bundle excludes; `None` for a full bundle or no git state.
    pub bundle_base: Option<SnapshotId>,
    /// Unix milliseconds at capture.
    pub created_at: u64,
    /// The byte length of the manifest.
    pub manifest_len: u64,
}

/// Which list of a snapshot a chunk row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The manifest's own bytes.
    Manifest = 0,
    /// A file, bundle or exclude object the manifest names.
    Data = 1,
}

/// A chunk id with the length of its file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkRef {
    pub id: ChunkId,
    pub stored_len: u64,
}

/// The open catalog.
pub struct Catalog {
    conn: Connection,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Catalog")
    }
}

fn connect(path: &Path) -> Result<Connection, CatalogError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA fullfsync = ON;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 10000;",
    )?;
    Ok(conn)
}

/// Reads a column that holds a count or a time, refusing a negative value.
fn get_u64(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(idx)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(idx, value))
}

fn opt_snapshot(bytes: Option<[u8; 32]>) -> Option<SnapshotId> {
    bytes.map(SnapshotId::from_bytes)
}

impl Catalog {
    /// Creates a new catalog file with the schema and the chunking parameters.
    ///
    /// The database is built under a temporary name and renamed into place
    /// after the schema commits, so a `catalog.sqlite` that exists is always
    /// complete and a kill during creation leaves nothing to clean by hand.
    pub(crate) fn create(path: &Path, params: &ChunkParams) -> Result<Catalog, CatalogError> {
        params.validate().map_err(CatalogError::Params)?;
        let tmp = path.with_extension("sqlite.tmp");
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = tmp.clone().into_os_string();
            name.push(suffix);
            match fs::remove_file(&name) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(CatalogError::Io {
                        path: name.into(),
                        source: e,
                    });
                }
            }
        }
        Self::build(&tmp, params)?;
        checkpoint::reached("create.before_rename");
        fs::rename(&tmp, path).map_err(|source| CatalogError::Io {
            path: path.to_owned(),
            source,
        })?;
        if let Some(parent) = path.parent() {
            durable::fsync_dir(parent).map_err(|source| CatalogError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        Self::open(path)
    }

    /// Writes the schema and settings into a fresh database at `path` and closes it.
    fn build(path: &Path, params: &ChunkParams) -> Result<(), CatalogError> {
        let mut conn = connect(path)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [SCHEMA_VERSION],
        )?;
        let settings: [(&str, String); 5] = [
            ("whole_file_limit", params.whole_file_limit.to_string()),
            ("min_size", params.min_size.to_string()),
            ("avg_size", params.avg_size.to_string()),
            ("max_size", params.max_size.to_string()),
            ("zstd_level", params.zstd_level.to_string()),
        ];
        for (key, value) in settings {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        // Closing the last connection checkpoints the WAL into the file.
        conn.close().map_err(|(_, e)| e)?;
        Ok(())
    }

    /// Opens an existing catalog, refusing another schema version.
    pub(crate) fn open(path: &Path) -> Result<Catalog, CatalogError> {
        let conn = connect(path)?;
        let found: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        if found != SCHEMA_VERSION {
            return Err(CatalogError::SchemaVersion {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(Catalog { conn })
    }

    fn setting<T: std::str::FromStr>(&self, key: &'static str) -> Result<T, CatalogError> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        value
            .and_then(|v| v.parse().ok())
            .ok_or(CatalogError::Setting { key })
    }

    /// Returns the chunking parameters recorded at creation.
    pub fn params(&self) -> Result<ChunkParams, CatalogError> {
        let params = ChunkParams {
            whole_file_limit: self.setting("whole_file_limit")?,
            min_size: self.setting("min_size")?,
            avg_size: self.setting("avg_size")?,
            max_size: self.setting("max_size")?,
            zstd_level: self.setting("zstd_level")?,
        };
        params.validate().map_err(CatalogError::Params)?;
        Ok(params)
    }

    /// Maps a row; a negative length or time is an error, never 0.
    fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRow> {
        Ok(SnapshotRow {
            id: SnapshotId::from_bytes(row.get(0)?),
            job_id: JobId::from_bytes(row.get(1)?),
            parent: opt_snapshot(row.get(2)?),
            bundle_base: opt_snapshot(row.get(3)?),
            created_at: get_u64(row, 4)?,
            manifest_len: get_u64(row, 5)?,
        })
    }

    const SNAPSHOT_COLUMNS: &'static str =
        "id, job_id, parent, bundle_base, created_at, manifest_len";

    /// Returns one snapshot row, if the catalog has it.
    pub fn snapshot(&self, id: SnapshotId) -> Result<Option<SnapshotRow>, CatalogError> {
        let sql = format!(
            "SELECT {} FROM snapshots WHERE id = ?1",
            Self::SNAPSHOT_COLUMNS
        );
        Ok(self
            .conn
            .query_row(&sql, [id.to_bytes()], Self::row_to_snapshot)
            .optional()?)
    }

    /// Returns every snapshot, oldest first.
    pub fn snapshots(&self) -> Result<Vec<SnapshotRow>, CatalogError> {
        let sql = format!(
            "SELECT {} FROM snapshots ORDER BY created_at, id",
            Self::SNAPSHOT_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], Self::row_to_snapshot)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns the chunk ids of one role of a snapshot, in order.
    pub(crate) fn snapshot_chunks(
        &self,
        id: SnapshotId,
        role: Role,
    ) -> Result<Vec<ChunkRef>, CatalogError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT sc.chunk_id, c.stored_len FROM snapshot_chunks sc
             JOIN chunks c ON c.id = sc.chunk_id
             WHERE sc.snapshot_id = ?1 AND sc.role = ?2 ORDER BY sc.ordinal",
        )?;
        let rows = stmt.query_map(params![id.to_bytes(), role as i64], |r| {
            Ok(ChunkRef {
                id: ChunkId::from_bytes(r.get(0)?),
                stored_len: get_u64(r, 1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records a snapshot and every chunk it reaches in one transaction.
    ///
    /// `before_commit` runs after every row is written and before the
    /// transaction commits; the crash tests stop the process there.
    pub(crate) fn insert_snapshot(
        &mut self,
        row: &SnapshotRow,
        manifest: &[ChunkRef],
        data: &[ChunkRef],
        before_commit: impl FnOnce(),
    ) -> Result<(), CatalogError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO snapshots (id, job_id, parent, bundle_base, created_at, manifest_len)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.id.to_bytes(),
                row.job_id.to_bytes(),
                row.parent.map(SnapshotId::to_bytes),
                row.bundle_base.map(SnapshotId::to_bytes),
                i64::try_from(row.created_at).unwrap_or(i64::MAX),
                i64::try_from(row.manifest_len).unwrap_or(i64::MAX),
            ],
        )?;
        {
            let mut insert_chunk = tx.prepare_cached(
                "INSERT INTO chunks (id, stored_len, refcount) VALUES (?1, ?2, 0)
                 ON CONFLICT(id) DO NOTHING",
            )?;
            let mut insert_ref = tx.prepare_cached(
                "INSERT INTO snapshot_chunks (snapshot_id, role, ordinal, chunk_id)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut bump = tx.prepare_cached(
                "UPDATE chunks SET refcount = refcount + 1, unreferenced_since = NULL
                 WHERE id = ?1",
            )?;
            let snapshot = row.id.to_bytes();
            for (role, list) in [(Role::Manifest, manifest), (Role::Data, data)] {
                for (ordinal, chunk) in list.iter().enumerate() {
                    let id = chunk.id.to_bytes();
                    insert_chunk.execute(params![
                        id,
                        i64::try_from(chunk.stored_len).unwrap_or(i64::MAX)
                    ])?;
                    insert_ref.execute(params![
                        snapshot,
                        role as i64,
                        i64::try_from(ordinal).unwrap_or(i64::MAX),
                        id
                    ])?;
                    bump.execute([id])?;
                }
            }
        }
        before_commit();
        tx.commit()?;
        Ok(())
    }

    /// Removes a snapshot and releases its references; returns false when it was not there.
    pub(crate) fn forget_snapshot(&mut self, id: SnapshotId) -> Result<bool, CatalogError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = id.to_bytes();
        tx.execute(
            "UPDATE chunks SET refcount = refcount - (
               SELECT COUNT(*) FROM snapshot_chunks sc
               WHERE sc.snapshot_id = ?1 AND sc.chunk_id = chunks.id)
             WHERE id IN (SELECT chunk_id FROM snapshot_chunks WHERE snapshot_id = ?1)",
            [snapshot],
        )?;
        let removed = tx.execute("DELETE FROM snapshots WHERE id = ?1", [snapshot])?;
        tx.commit()?;
        Ok(removed == 1)
    }

    /// Stamps every chunk with no references that has no stamp yet; returns how many.
    pub(crate) fn mark_unreferenced(&mut self, now_ms: u64) -> Result<u64, CatalogError> {
        let changed = self.conn.execute(
            "UPDATE chunks SET unreferenced_since = ?1
             WHERE refcount = 0 AND unreferenced_since IS NULL",
            [i64::try_from(now_ms).unwrap_or(i64::MAX)],
        )?;
        Ok(changed as u64)
    }

    /// Returns the unreferenced chunks whose grace has run out.
    pub(crate) fn expired(
        &self,
        now_ms: u64,
        grace_ms: u64,
    ) -> Result<Vec<ChunkRef>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, stored_len FROM chunks
             WHERE refcount = 0 AND unreferenced_since IS NOT NULL
               AND unreferenced_since + ?1 <= ?2",
        )?;
        let rows = stmt.query_map(
            params![
                i64::try_from(grace_ms).unwrap_or(i64::MAX),
                i64::try_from(now_ms).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok(ChunkRef {
                    id: ChunkId::from_bytes(r.get(0)?),
                    stored_len: get_u64(r, 1)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Deletes chunk rows that still have no references.
    pub(crate) fn delete_unreferenced(&mut self, ids: &[ChunkId]) -> Result<(), CatalogError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt =
                tx.prepare_cached("DELETE FROM chunks WHERE id = ?1 AND refcount = 0")?;
            for id in ids {
                stmt.execute([id.to_bytes()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns true when some snapshot's bundle excludes the refs of `id`.
    pub fn is_bundle_base(&self, id: SnapshotId) -> Result<bool, CatalogError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT 1 FROM snapshots WHERE bundle_base = ?1")?;
        Ok(stmt.exists([id.to_bytes()])?)
    }

    /// Returns every snapshot whose bundle base has no row, with that base.
    pub fn broken_chains(&self) -> Result<Vec<(SnapshotId, SnapshotId)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, bundle_base FROM snapshots
             WHERE bundle_base IS NOT NULL
               AND bundle_base NOT IN (SELECT id FROM snapshots)
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                SnapshotId::from_bytes(r.get(0)?),
                SnapshotId::from_bytes(r.get(1)?),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns true when the catalog has a row for the chunk.
    pub fn has_chunk(&self, id: ChunkId) -> Result<bool, CatalogError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT 1 FROM chunks WHERE id = ?1")?;
        Ok(stmt.exists([id.to_bytes()])?)
    }

    /// Returns every chunk id the catalog has a row for.
    pub fn chunk_ids(&self) -> Result<HashSet<ChunkId>, CatalogError> {
        let mut stmt = self.conn.prepare("SELECT id FROM chunks")?;
        let rows = stmt.query_map([], |r| Ok(ChunkId::from_bytes(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns the chunks whose stored refcount differs from the count of rows naming them.
    pub fn refcount_mismatches(&self) -> Result<Vec<ChunkId>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id FROM chunks c
             WHERE c.refcount != (SELECT COUNT(*) FROM snapshot_chunks sc WHERE sc.chunk_id = c.id)",
        )?;
        let rows = stmt.query_map([], |r| Ok(ChunkId::from_bytes(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
