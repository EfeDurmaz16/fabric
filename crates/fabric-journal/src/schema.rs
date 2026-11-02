//! The journal's schema and its migrations.
//!
//! Invariant: a file is a journal only when its `application_id` is
//! [`APPLICATION_ID`]; its `user_version` is the schema version. Version
//! `n` becomes `n + 1` only by running `MIGRATIONS[n - 1]` in its own
//! `BEGIN IMMEDIATE` transaction that ends by setting `user_version`, and
//! no migration rewrites an event's `body` or `digest`.

/// `"FBRJ"`: the `application_id` of every journal.
pub const APPLICATION_ID: i32 = 0x4642_524A;

/// The schema version this crate writes and reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The pragmas every connection runs before anything else.
pub(crate) const CONNECTION_PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA fullfsync = ON;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 10000;
";

/// Version 1, written whole by `Journal::create`.
pub(crate) const V1: &str = "
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;

CREATE TABLE jobs (
  job_id       BLOB PRIMARY KEY CHECK (length(job_id) = 16),
  mode         TEXT NOT NULL CHECK (mode = 'managed'),
  revision     INTEGER NOT NULL CHECK (revision >= 1),
  generation   INTEGER NOT NULL CHECK (generation >= 1),
  state        TEXT NOT NULL CHECK (state IN ('active','finished','cancelled')),
  primary_task BLOB NOT NULL CHECK (length(primary_task) = 16),
  intent       BLOB NOT NULL,
  source       BLOB NOT NULL,
  policy       BLOB NOT NULL,
  head_seq     INTEGER NOT NULL,
  created_ms   INTEGER NOT NULL,
  updated_ms   INTEGER NOT NULL
) STRICT;

CREATE TABLE operations (
  operation_id      BLOB PRIMARY KEY CHECK (length(operation_id) = 16),
  op_key            TEXT NOT NULL UNIQUE,
  request_digest    BLOB NOT NULL CHECK (length(request_digest) = 32),
  job_id            BLOB REFERENCES jobs(job_id),
  kind              TEXT NOT NULL CHECK (kind IN ('create_job','start_attempt','register_task','finish_task','cancel_job')),
  expected_revision INTEGER,
  accepted_rev      INTEGER,
  status            TEXT NOT NULL CHECK (status IN ('accepted','dispatched','succeeded','failed','cancelled','rejected','unknown')),
  result            BLOB NOT NULL,
  first_seq         INTEGER,
  accepted_ms       INTEGER NOT NULL,
  updated_ms        INTEGER NOT NULL,
  CHECK ((status = 'rejected') = (accepted_rev IS NULL))
) STRICT;
CREATE INDEX operations_by_job ON operations (job_id, first_seq);

CREATE TABLE tasks (
  task_id     BLOB PRIMARY KEY CHECK (length(task_id) = 16),
  job_id      BLOB NOT NULL REFERENCES jobs(job_id),
  parent_id   BLOB REFERENCES tasks(task_id),
  kind        TEXT NOT NULL CHECK (kind IN ('primary','child')),
  kind_body   BLOB NOT NULL,
  restart     TEXT NOT NULL CHECK (restart IN ('block','explicit_only')),
  state       TEXT NOT NULL CHECK (state IN ('open','succeeded','failed','cancelled')),
  created_rev INTEGER NOT NULL
) STRICT;
CREATE INDEX tasks_by_job ON tasks (job_id, created_rev);

CREATE TABLE attempts (
  attempt_id         BLOB PRIMARY KEY CHECK (length(attempt_id) = 16),
  task_id            BLOB NOT NULL REFERENCES tasks(task_id),
  job_id             BLOB NOT NULL REFERENCES jobs(job_id),
  generation         INTEGER NOT NULL CHECK (generation >= 1),
  resource           BLOB NOT NULL,
  initial_checkpoint BLOB CHECK (initial_checkpoint IS NULL OR length(initial_checkpoint) = 32),
  granted_by         BLOB NOT NULL REFERENCES operations(operation_id),
  status             TEXT NOT NULL CHECK (status IN ('granted','running','succeeded','failed','cancelled','checkpointed','lost','unknown')),
  status_body        BLOB NOT NULL,
  last_checkpoint    BLOB CHECK (last_checkpoint IS NULL OR length(last_checkpoint) = 32),
  granted_ms         INTEGER NOT NULL,
  ended_ms           INTEGER,
  UNIQUE (job_id, generation)
) STRICT;

CREATE TABLE events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id       BLOB NOT NULL REFERENCES jobs(job_id),
  revision     INTEGER NOT NULL CHECK (revision >= 1),
  operation_id BLOB REFERENCES operations(operation_id),
  kind         TEXT NOT NULL,
  body         BLOB NOT NULL,
  digest       BLOB NOT NULL CHECK (length(digest) = 32),
  prev_digest  BLOB CHECK (prev_digest IS NULL OR length(prev_digest) = 32),
  at_ms        INTEGER NOT NULL
) STRICT;
CREATE INDEX events_by_job ON events (job_id, seq);

CREATE TABLE evidence (
  evidence_id BLOB PRIMARY KEY CHECK (length(evidence_id) = 16),
  job_id      BLOB NOT NULL REFERENCES jobs(job_id),
  attempt_id  BLOB REFERENCES attempts(attempt_id),
  revision    INTEGER NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('checkpoint','exit')),
  ref         TEXT NOT NULL,
  digest      BLOB CHECK (digest IS NULL OR length(digest) = 32),
  seq         INTEGER NOT NULL REFERENCES events(seq)
) STRICT;

CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
  BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
  BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER evidence_append_only_update BEFORE UPDATE ON evidence
  BEGIN SELECT RAISE(ABORT, 'evidence is append-only'); END;
CREATE TRIGGER evidence_append_only_delete BEFORE DELETE ON evidence
  BEGIN SELECT RAISE(ABORT, 'evidence is append-only'); END;
";

/// `MIGRATIONS[n - 1]` takes a journal from version `n` to `n + 1`. Version
/// 1 is the first, so there is none yet; each new version adds one here
/// and a fixture under `tests/fixtures/v<n>.sqlite` for the one before.
pub(crate) const MIGRATIONS: &[&str] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_one_migration_per_version_after_the_first() {
        assert_eq!(MIGRATIONS.len() + 1, SCHEMA_VERSION as usize);
    }

    #[test]
    fn the_application_id_spells_fbrj() {
        assert_eq!(&APPLICATION_ID.to_be_bytes(), b"FBRJ");
    }
}
