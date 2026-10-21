//! Content-addressed objects on disk.
//!
//! Invariant: a file under `objects/<xx>/<hash>` holds one zstd frame whose
//! decompressed bytes hash to `<hash>`, it reached that name by a flushed
//! temp file and a link, and nothing rewrites it in place. A reader verifies
//! the hash before it hands bytes out, so a corrupt object is an error and
//! never content.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use fabric_core::id::ChunkId;
use fastcdc::v2020::{AVERAGE_MAX, AVERAGE_MIN, FastCDC, MAXIMUM_MAX, MINIMUM_MIN, StreamCDC};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

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
    /// The path is not a regular file: a symlink, FIFO, socket or device.
    #[error("{path} is not a regular file")]
    NotRegular {
        /// The path.
        path: PathBuf,
    },
    /// The bytes are longer than any object may be.
    #[error("{len} bytes is above the {MAX_OBJECT_LEN} byte object bound")]
    TooLarge {
        /// The length that was offered.
        len: usize,
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

/// Opens `path` for reading when it is a regular file, and returns `None` for anything else.
///
/// The open does not follow a symlink in the last component and does not
/// wait on a FIFO, and the type is read from the open descriptor, so what
/// the caller reads is the file that was checked.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<File>> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(Errno::LOOP) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(file.metadata()?.is_file().then_some(file))
}

/// The largest object a store writes: the bulk frame bound of ADR 0005.
pub const MAX_OBJECT_LEN: u32 = 8 << 20;

const _: () = assert!(MAX_OBJECT_LEN as usize <= MAXIMUM_MAX);

/// How files are cut into objects and how objects are compressed.
///
/// A store records its parameters in the catalog when it is created and uses
/// them for its whole life, so every node that writes the same bytes with the
/// same parameters writes the same object ids. Only `new` builds a value, so
/// every value keeps the chunker's rules and no object exceeds
/// `MAX_OBJECT_LEN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    whole_file_limit: u32,
    min_size: u32,
    avg_size: u32,
    max_size: u32,
    zstd_level: i32,
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

    /// Checks the rules of the chunker and of the object bound, and builds the parameters.
    ///
    /// The chunker needs `64 <= min_size <= avg_size <= max_size` and an
    /// average from 256 bytes to 4 MiB; `max_size` and `whole_file_limit`
    /// may not exceed `MAX_OBJECT_LEN`, and the zstd level is 1 to 22.
    pub fn new(
        whole_file_limit: u32,
        min_size: u32,
        avg_size: u32,
        max_size: u32,
        zstd_level: i32,
    ) -> Result<ChunkParams, ObjectError> {
        let (min, avg, max) = (min_size as usize, avg_size as usize, max_size as usize);
        if min < MINIMUM_MIN {
            return Err(ObjectError::BadParams("min_size is below 64 bytes"));
        }
        if !(AVERAGE_MIN..=AVERAGE_MAX).contains(&avg) {
            return Err(ObjectError::BadParams(
                "avg_size is outside 256 bytes to 4 MiB",
            ));
        }
        if !(min <= avg && avg <= max) {
            return Err(ObjectError::BadParams(
                "sizes must satisfy min <= avg <= max",
            ));
        }
        if max_size > MAX_OBJECT_LEN || whole_file_limit > MAX_OBJECT_LEN {
            return Err(ObjectError::BadParams(
                "max_size and whole_file_limit may not exceed 8 MiB",
            ));
        }
        if !(1..=22).contains(&zstd_level) {
            return Err(ObjectError::BadParams("zstd_level is outside 1 to 22"));
        }
        Ok(ChunkParams {
            whole_file_limit,
            min_size,
            avg_size,
            max_size,
            zstd_level,
        })
    }

    /// A file of this many bytes or fewer is one object.
    #[must_use]
    pub const fn whole_file_limit(&self) -> u32 {
        self.whole_file_limit
    }

    /// The smallest chunk `FastCDC` may cut above the limit.
    #[must_use]
    pub const fn min_size(&self) -> u32 {
        self.min_size
    }

    /// The chunk size `FastCDC` aims for.
    #[must_use]
    pub const fn avg_size(&self) -> u32 {
        self.avg_size
    }

    /// The largest chunk `FastCDC` may cut.
    #[must_use]
    pub const fn max_size(&self) -> u32 {
        self.max_size
    }

    /// The zstd level applied to every object.
    #[must_use]
    pub const fn zstd_level(&self) -> i32 {
        self.zstd_level
    }
}

/// One object after `put`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredChunk {
    /// The object's id.
    pub id: ChunkId,
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
    /// Objects this process linked since the last `flush_shards`; their bytes are the ones it hashed.
    fresh: Mutex<HashSet<ChunkId>>,
}

impl ObjectStore {
    pub(crate) fn new(store_root: &Path, params: ChunkParams) -> ObjectStore {
        ObjectStore {
            objects_dir: store_root.join("objects"),
            tmp_dir: store_root.join("tmp"),
            params,
            dirty_shards: Mutex::new(HashSet::new()),
            fresh: Mutex::new(HashSet::new()),
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
    pub(crate) fn object_path(&self, id: ChunkId) -> PathBuf {
        let hex = id.to_string();
        self.objects_dir.join(&hex[..2]).join(hex)
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
    ///
    /// Bytes above `MAX_OBJECT_LEN` are refused: `read` would report such an object as corrupt.
    pub fn put(&self, bytes: &[u8]) -> Result<StoredChunk, ObjectError> {
        if bytes.len() > MAX_OBJECT_LEN as usize {
            return Err(ObjectError::TooLarge { len: bytes.len() });
        }
        let id = ChunkId::of(bytes);
        let path = self.object_path(id);
        if let Ok(meta) = fs::metadata(&path) {
            return Ok(StoredChunk {
                id,
                stored_len: meta.len(),
                new: false,
            });
        }
        let compressed =
            zstd::bulk::compress(bytes, self.params.zstd_level).map_err(|e| io_err(&path, e))?;
        let published = durable::publish_noclobber(&self.tmp_dir, &path, &compressed)
            .map_err(|e| io_err(&path, e))?;
        if published == Published::New {
            self.fresh
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id);
        }
        self.mark_dirty(id);
        Ok(StoredChunk {
            id,
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
        if file.size <= u64::from(self.params.whole_file_limit) {
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

    /// Reads the regular file at `path` into objects by the same rule as `put_bytes`.
    ///
    /// A symlink, FIFO, socket or device at `path` is refused, not followed or read.
    pub fn put_file(&self, path: &Path) -> Result<StoredFile, ObjectError> {
        let file = open_regular(path)
            .map_err(|e| io_err(path, e))?
            .ok_or_else(|| ObjectError::NotRegular {
                path: path.to_owned(),
            })?;
        self.put_reader(file, path)
    }

    /// Reads `reader` to its end into objects by the same rule as `put_bytes`.
    ///
    /// At most `whole_file_limit + 1` bytes are held at once: a reader that
    /// turns out longer, including a file that grew after it was measured,
    /// goes through the streaming chunker from the bytes already read on.
    /// `size` is the number of bytes read. `path` names the source in errors.
    pub fn put_reader(
        &self,
        mut reader: impl Read,
        path: &Path,
    ) -> Result<StoredFile, ObjectError> {
        let limit = u64::from(self.params.whole_file_limit);
        let mut head = Vec::new();
        (&mut reader)
            .take(limit + 1)
            .read_to_end(&mut head)
            .map_err(|e| io_err(path, e))?;
        if head.len() as u64 <= limit {
            return self.put_bytes(&head);
        }
        let reader = BufReader::with_capacity(1 << 20, io::Cursor::new(head).chain(reader));
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

    /// Returns the length an object decompresses to, read from its frame header.
    ///
    /// The header is not checked against the bytes here; `read` does that.
    /// A header that records no length, or one above `MAX_OBJECT_LEN`, is corrupt.
    pub(crate) fn content_len(&self, id: ChunkId) -> Result<u64, ObjectError> {
        /// `ZSTD_FRAMEHEADERSIZE_MAX`: no frame header is longer.
        const FRAME_HEADER_MAX: u64 = 18;
        let path = self.object_path(id);
        let file = match open_regular(&path) {
            Ok(Some(file)) => file,
            Ok(None) => return Err(ObjectError::Corrupt { id }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing { id });
            }
            Err(e) => return Err(io_err(&path, e)),
        };
        let mut head = Vec::new();
        file.take(FRAME_HEADER_MAX)
            .read_to_end(&mut head)
            .map_err(|e| io_err(&path, e))?;
        match zstd::zstd_safe::get_frame_content_size(&head) {
            Ok(Some(len)) if len <= u64::from(MAX_OBJECT_LEN) => Ok(len),
            _ => Err(ObjectError::Corrupt { id }),
        }
    }

    /// Reads and verifies one object.
    ///
    /// No frame of `MAX_OBJECT_LEN` bytes compresses to more than zstd's
    /// compression bound for that length, so a longer file is corrupt and is
    /// refused before it is loaded: a grown file never decides the size of
    /// the buffer.
    pub fn read(&self, id: ChunkId) -> Result<Vec<u8>, ObjectError> {
        let path = self.object_path(id);
        let bound = MAX_OBJECT_LEN as usize;
        let stored_bound = zstd::zstd_safe::compress_bound(bound) as u64;
        let file = match open_regular(&path) {
            Ok(Some(file)) => file,
            Ok(None) => return Err(ObjectError::Corrupt { id }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing { id });
            }
            Err(e) => return Err(io_err(&path, e)),
        };
        let mut compressed = Vec::new();
        file.take(stored_bound + 1)
            .read_to_end(&mut compressed)
            .map_err(|e| io_err(&path, e))?;
        if compressed.len() as u64 > stored_bound {
            return Err(ObjectError::Corrupt { id });
        }
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

    /// Returns an object's file as stored, one zstd frame, without decompressing it.
    ///
    /// This is what a peer receives for a chunk (ADR 0013, Chunks travel as
    /// stored); the receiver decompresses and checks the hash. A file larger
    /// than zstd's worst case for `MAX_OBJECT_LEN` is `Corrupt`, so a damaged
    /// store never makes this node read or send more than one bulk frame.
    pub fn read_stored(&self, id: ChunkId) -> Result<Vec<u8>, ObjectError> {
        let path = self.object_path(id);
        let bound = zstd::zstd_safe::compress_bound(MAX_OBJECT_LEN as usize) as u64;
        match fs::metadata(&path) {
            Ok(meta) if meta.len() > bound => return Err(ObjectError::Corrupt { id }),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing { id });
            }
            Err(e) => return Err(io_err(&path, e)),
        }
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ObjectError::Missing { id }),
            Err(e) => Err(io_err(&path, e)),
        }
    }

    /// True when this process linked the object since the last `flush_shards`.
    ///
    /// Such an object holds the bytes `put` hashed, and its shard is already
    /// marked. Any other object without a catalog row may have been linked
    /// by a process that died before its flush.
    pub(crate) fn is_fresh(&self, id: ChunkId) -> bool {
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
    }

    /// Records that the shard holding `id` has an entry the next `flush_shards` must flush.
    ///
    /// `put` calls it for an object it wrote. Publish also calls it for an
    /// object that was already on disk but has no catalog row yet: that
    /// file may have been linked by a process that was killed before its
    /// flush, and only a catalog row proves a shard was flushed after it.
    pub(crate) fn mark_dirty(&self, id: ChunkId) {
        self.mark_dirty_shard(id.to_bytes()[0]);
    }

    /// Flushes every shard directory marked dirty since the last call.
    pub(crate) fn flush_shards(&self) -> Result<(), ObjectError> {
        let shards: Vec<u8> = self
            .dirty_shards
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .collect();
        for (index, shard) in shards.iter().enumerate() {
            let dir = self.objects_dir.join(format!("{shard:02x}"));
            if let Err(e) = durable::fsync_dir(&dir) {
                // These shards stay owed a flush, and their objects stay fresh.
                for owed in &shards[index..] {
                    self.mark_dirty_shard(*owed);
                }
                return Err(io_err(&dir, e));
            }
        }
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(())
    }

    fn mark_dirty_shard(&self, shard: u8) {
        self.dirty_shards
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(shard);
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
