//! Content-addressed objects on disk.
//!
//! Invariant: a file under `objects/<xx>/<hash>` holds one zstd frame whose
//! decompressed bytes hash to `<hash>`, it reached that name by a flushed
//! temp file and a link, and nothing rewrites it in place. A reader verifies
//! the hash before it hands bytes out, so a corrupt object is an error and
//! never content.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use fabric_core::id::ChunkId;
use fastcdc::v2020::{AVERAGE_MAX, AVERAGE_MIN, FastCDC, MAXIMUM_MAX, MINIMUM_MIN, StreamCDC};

use crate::durable::{self, Published};

/// Why an object could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum ObjectError {
    /// A filesystem call failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the call was made on.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// No object file exists for the id.
    #[error("object {id} is not in the store")]
    Missing {
        /// The id that was asked for.
        id: ChunkId,
    },
    /// The object file does not decompress to bytes that hash to its name.
    #[error("object {id} is corrupt")]
    Corrupt {
        /// The id whose file is wrong.
        id: ChunkId,
    },
    /// The chunking parameters break a rule of the chunker.
    #[error("chunk parameters: {0}")]
    BadParams(&'static str),
}

fn io_err(path: &Path, source: io::Error) -> ObjectError {
    ObjectError::Io {
        path: path.to_owned(),
        source,
    }
}

/// How files are cut into objects and how objects are compressed.
///
/// A store records its parameters in the catalog when it is created and uses
/// them for its whole life, so every node that writes the same bytes with the
/// same parameters writes the same object ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    /// A file of this many bytes or fewer is one object.
    pub whole_file_limit: u64,
    /// The smallest chunk `FastCDC` may cut above the limit.
    pub min_size: u32,
    /// The chunk size `FastCDC` aims for.
    pub avg_size: u32,
    /// The largest chunk `FastCDC` may cut.
    pub max_size: u32,
    /// The zstd level applied to every object.
    pub zstd_level: i32,
}

impl ChunkParams {
    /// The parameters of ADR 0004: whole files to 8 MiB, then 1, 4 and 8 MiB chunks, zstd 3.
    pub const DEFAULT: ChunkParams = ChunkParams {
        whole_file_limit: 8 << 20,
        min_size: 1 << 20,
        avg_size: 4 << 20,
        max_size: 8 << 20,
        zstd_level: 3,
    };

    /// Checks the rules the chunker enforces, so that it never panics on them.
    pub fn validate(&self) -> Result<(), ObjectError> {
        let min = self.min_size as usize;
        let avg = self.avg_size as usize;
        let max = self.max_size as usize;
        if min < MINIMUM_MIN {
            return Err(ObjectError::BadParams("min_size is below 64 bytes"));
        }
        if !(AVERAGE_MIN..=AVERAGE_MAX).contains(&avg) {
            return Err(ObjectError::BadParams(
                "avg_size is outside 256 bytes to 4 MiB",
            ));
        }
        if max > MAXIMUM_MAX {
            return Err(ObjectError::BadParams("max_size is above 16 MiB"));
        }
        if !(min <= avg && avg <= max) {
            return Err(ObjectError::BadParams(
                "sizes must satisfy min <= avg <= max",
            ));
        }
        if !(1..=22).contains(&self.zstd_level) {
            return Err(ObjectError::BadParams("zstd_level is outside 1 to 22"));
        }
        Ok(())
    }

    /// The most bytes one object decompresses to under these parameters.
    fn largest_object(&self) -> usize {
        usize::try_from(self.whole_file_limit.max(u64::from(self.max_size))).unwrap_or(usize::MAX)
    }
}

/// One object after `put`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredChunk {
    /// The object's id.
    pub id: ChunkId,
    /// Its uncompressed length.
    pub len: u64,
    /// Its length on disk.
    pub stored_len: u64,
    /// Whether this call wrote it, or it was already present.
    pub new: bool,
}

/// One file after `put_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    /// The objects that hold the file's bytes, in order.
    pub chunks: Vec<ChunkId>,
    /// The number of bytes read from the file.
    pub size: u64,
    /// How many objects this call wrote.
    pub new_objects: u64,
    /// How many bytes on disk those objects took.
    pub new_bytes: u64,
}

/// The object files of one store.
#[derive(Debug)]
pub struct ObjectStore {
    objects_dir: PathBuf,
    tmp_dir: PathBuf,
    params: ChunkParams,
    dirty_shards: Mutex<HashSet<u8>>,
}

impl ObjectStore {
    pub(crate) fn new(store_root: &Path, params: ChunkParams) -> ObjectStore {
        ObjectStore {
            objects_dir: store_root.join("objects"),
            tmp_dir: store_root.join("tmp"),
            params,
            dirty_shards: Mutex::new(HashSet::new()),
        }
    }

    /// Creates `objects/`, all 256 shard directories and `tmp/`, and flushes `objects/`.
    ///
    /// No shard is ever created on the publish path, so the only directory
    /// entries publish has to flush are the ones inside the shards.
    pub(crate) fn create_dirs(&self) -> Result<(), ObjectError> {
        fs::create_dir_all(&self.objects_dir).map_err(|e| io_err(&self.objects_dir, e))?;
        for shard in 0..=u8::MAX {
            let dir = self.objects_dir.join(format!("{shard:02x}"));
            fs::create_dir_all(&dir).map_err(|e| io_err(&dir, e))?;
        }
        durable::fsync_dir(&self.objects_dir).map_err(|e| io_err(&self.objects_dir, e))?;
        fs::create_dir_all(&self.tmp_dir).map_err(|e| io_err(&self.tmp_dir, e))
    }

    /// Returns the staging directory, which only the lock holder writes and open and gc clear.
    pub(crate) fn tmp_dir(&self) -> &Path {
        &self.tmp_dir
    }

    /// Returns the parameters the store was created with.
    #[must_use]
    pub fn params(&self) -> &ChunkParams {
        &self.params
    }

    /// Returns the path an object with `id` has or would have.
    #[must_use]
    pub fn object_path(&self, id: ChunkId) -> PathBuf {
        let hex = id.to_string();
        self.objects_dir.join(&hex[..2]).join(hex)
    }

    /// Returns true when an object file with `id` exists; it does not verify it.
    #[must_use]
    pub fn contains(&self, id: ChunkId) -> bool {
        self.object_path(id).is_file()
    }

    /// Returns the on-disk length of the object, or `Missing`.
    pub fn stored_len(&self, id: ChunkId) -> Result<u64, ObjectError> {
        let path = self.object_path(id);
        match fs::metadata(&path) {
            Ok(meta) => Ok(meta.len()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ObjectError::Missing { id }),
            Err(e) => Err(io_err(&path, e)),
        }
    }

    /// Writes `bytes` as one object and returns its id; a present object is left as it is.
    pub fn put(&self, bytes: &[u8]) -> Result<StoredChunk, ObjectError> {
        let id = ChunkId::of(bytes);
        let path = self.object_path(id);
        if let Ok(meta) = fs::metadata(&path) {
            return Ok(StoredChunk {
                id,
                len: bytes.len() as u64,
                stored_len: meta.len(),
                new: false,
            });
        }
        let compressed =
            zstd::bulk::compress(bytes, self.params.zstd_level).map_err(|e| io_err(&path, e))?;
        let published = durable::publish_noclobber(&self.tmp_dir, &path, &compressed)
            .map_err(|e| io_err(&path, e))?;
        self.mark_dirty(id);
        Ok(StoredChunk {
            id,
            len: bytes.len() as u64,
            stored_len: compressed.len() as u64,
            new: published == Published::New,
        })
    }

    /// Writes `bytes` as one object or, above the whole-file limit, as `FastCDC` chunks.
    pub fn put_bytes(&self, bytes: &[u8]) -> Result<StoredFile, ObjectError> {
        let mut file = StoredFile {
            chunks: Vec::new(),
            size: bytes.len() as u64,
            new_objects: 0,
            new_bytes: 0,
        };
        if file.size <= self.params.whole_file_limit {
            file.record(self.put(bytes)?);
            return Ok(file);
        }
        let p = &self.params;
        for chunk in FastCDC::new(
            bytes,
            p.min_size as usize,
            p.avg_size as usize,
            p.max_size as usize,
        ) {
            file.record(self.put(&bytes[chunk.offset..chunk.offset + chunk.length])?);
        }
        Ok(file)
    }

    /// Reads the file at `path` into objects by the same rule as `put_bytes`.
    ///
    /// `size` is the number of bytes read, which is the file's length unless
    /// something wrote to it meanwhile.
    pub fn put_file(&self, path: &Path) -> Result<StoredFile, ObjectError> {
        let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
        if meta.len() <= self.params.whole_file_limit {
            let bytes = fs::read(path).map_err(|e| io_err(path, e))?;
            return self.put_bytes(&bytes);
        }
        let reader =
            BufReader::with_capacity(1 << 20, File::open(path).map_err(|e| io_err(path, e))?);
        let p = &self.params;
        let mut file = StoredFile {
            chunks: Vec::new(),
            size: 0,
            new_objects: 0,
            new_bytes: 0,
        };
        for chunk in StreamCDC::new(
            reader,
            p.min_size as usize,
            p.avg_size as usize,
            p.max_size as usize,
        ) {
            let chunk = chunk.map_err(|e| io_err(path, io::Error::other(e)))?;
            file.size += chunk.length as u64;
            file.record(self.put(&chunk.data)?);
        }
        Ok(file)
    }

    /// Reads and verifies one object.
    pub fn read(&self, id: ChunkId) -> Result<Vec<u8>, ObjectError> {
        let path = self.object_path(id);
        let compressed = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing { id });
            }
            Err(e) => return Err(io_err(&path, e)),
        };
        let bound = self.params.largest_object();
        let capacity = match zstd::zstd_safe::get_frame_content_size(&compressed) {
            Ok(Some(n)) if usize::try_from(n).is_ok_and(|n| n <= bound) => {
                usize::try_from(n).unwrap_or(bound)
            }
            Ok(None) => bound,
            _ => return Err(ObjectError::Corrupt { id }),
        };
        let bytes = zstd::bulk::decompress(&compressed, capacity)
            .map_err(|_| ObjectError::Corrupt { id })?;
        if ChunkId::of(&bytes) != id {
            return Err(ObjectError::Corrupt { id });
        }
        Ok(bytes)
    }

    /// Records that the shard holding `id` has an entry the next `flush_shards` must flush.
    ///
    /// `put` calls it for an object it wrote. Publish also calls it for an
    /// object that was already on disk but has no catalog row yet: that
    /// file may have been linked by a process that was killed before its
    /// flush, and only a catalog row proves a shard was flushed after it.
    pub(crate) fn mark_dirty(&self, id: ChunkId) {
        let shard = id.to_bytes()[0];
        self.dirty_shards
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(shard);
    }

    /// Flushes every shard directory marked dirty since the last call.
    pub(crate) fn flush_shards(&self) -> Result<(), ObjectError> {
        let shards: Vec<u8> = self
            .dirty_shards
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .collect();
        for shard in shards {
            let dir = self.objects_dir.join(format!("{shard:02x}"));
            durable::fsync_dir(&dir).map_err(|e| io_err(&dir, e))?;
        }
        Ok(())
    }

    /// Removes an object file; a missing file is not an error.
    pub(crate) fn remove(&self, id: ChunkId) -> Result<(), ObjectError> {
        let path = self.object_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(&path, e)),
        }
    }

    /// Removes every file under `tmp/` and returns how many.
    pub(crate) fn clear_tmp(&self) -> Result<u64, ObjectError> {
        let mut removed = 0;
        for entry in fs::read_dir(&self.tmp_dir).map_err(|e| io_err(&self.tmp_dir, e))? {
            let entry = entry.map_err(|e| io_err(&self.tmp_dir, e))?;
            fs::remove_file(entry.path()).map_err(|e| io_err(&entry.path(), e))?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Lists every object file with its modification time.
    pub(crate) fn scan(&self) -> Result<Vec<(ChunkId, SystemTime)>, ObjectError> {
        let mut found = Vec::new();
        for shard in fs::read_dir(&self.objects_dir).map_err(|e| io_err(&self.objects_dir, e))? {
            let shard = shard.map_err(|e| io_err(&self.objects_dir, e))?.path();
            if !shard.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&shard).map_err(|e| io_err(&shard, e))? {
                let entry = entry.map_err(|e| io_err(&shard, e))?;
                let name = entry.file_name();
                let Some(id) = name.to_str().and_then(|s| s.parse::<ChunkId>().ok()) else {
                    tracing::warn!(path = %entry.path().display(), "not an object name; left alone");
                    continue;
                };
                let meta = entry.metadata().map_err(|e| io_err(&entry.path(), e))?;
                let mtime = meta.modified().map_err(|e| io_err(&entry.path(), e))?;
                found.push((id, mtime));
            }
        }
        Ok(found)
    }
}

impl StoredFile {
    fn record(&mut self, chunk: StoredChunk) {
        self.chunks.push(chunk.id);
        if chunk.new {
            self.new_objects += 1;
            self.new_bytes += chunk.stored_len;
        }
    }
}
