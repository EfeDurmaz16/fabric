//! A node's cache of the packs a job's bundle chain unpacks into (ADR 0014, Restore).
//!
//! A restore replayed every link of the chain, so a resume grew with the
//! job's history: about 30 ms a link in the end-to-end fixture, up to 16
//! links. The cache keeps, per job, the pack files `git bundle unbundle`
//! wrote for each link it unpacked on this node; the next restore of the
//! same chain copies them in (a clone on APFS, a hard link elsewhere) and
//! unpacks only the links the cache does not hold.
//!
//! Invariant: a restore puts a cached file into a repository only when its
//! bytes are the ones `git bundle unbundle` wrote when it unpacked that
//! link on this node, into a repository that held exactly the links below
//! it in the same chain. Each file is checked against the length and the
//! digest recorded with it before it is used, and a job's cache holds one
//! chain's links, oldest first, never a link of another chain beside them:
//! a restore of a chain that parts from the cached one drops the cached
//! links from where they part. The cache is never the only copy of
//! anything; every file in it can be made again from the bundles in the
//! store, so a file that is missing, changed or cut short costs only the
//! replay of the links from that one on, and nothing here is flushed. The
//! store's one-writer rule (ADR 0014) makes the restore that holds the
//! store the cache's only writer.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fabric_core::id::{ChunkId, JobId, SnapshotId};

/// The directory under the store root that holds one cache per job.
const CACHE_DIR: &str = "bundle-cache";

/// The file of a job's cache that lists its links, oldest first.
const RECORD: &str = "links";

/// The first line of `RECORD`; a record with another one is not read.
const RECORD_HEADER: &str = "fabric bundle cache 1\n";

/// How many jobs keep a cache on one node; the ones a restore used last stay.
const MAX_JOBS: usize = 32;

/// How long a cache no restore used is kept.
const MAX_IDLE: Duration = Duration::from_hours(14 * 24);

/// The block of `digest`.
const BLOCK: usize = 1 << 20;

/// One file a link's unpacking wrote into `objects/pack`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedFile {
    /// Its name, `pack-<id>.pack`, `.idx` or `.rev`.
    name: String,
    /// Its length in bytes.
    len: u64,
    /// `digest` of its bytes.
    digest: ChunkId,
}

/// One link of the cached chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedLink {
    /// The snapshot of the link.
    snapshot: SnapshotId,
    /// False for a link whose bundle is empty, because the refs of the one
    /// below already reached every object; it is in the chain all the same.
    bundle: bool,
    /// The files its unpacking wrote; none for an empty bundle, or when
    /// every object was already there.
    files: Vec<CachedFile>,
}

/// The cache of one job's chain.
#[derive(Debug)]
pub(crate) struct BundleCache {
    dir: PathBuf,
    links: Vec<CachedLink>,
}

impl BundleCache {
    /// Opens the cache of `job` in the store at `store_root`; one that is
    /// absent, or whose record cannot be read, holds no link.
    pub(crate) fn open(store_root: &Path, job: JobId) -> BundleCache {
        let dir = store_root.join(CACHE_DIR).join(job.to_string());
        let links = fs::read_to_string(dir.join(RECORD))
            .map(|text| parse_record(&text))
            .unwrap_or_default();
        BundleCache { dir, links }
    }

    /// Returns how many links of `chain`, oldest first, the cache holds
    /// with every file as it was recorded.
    ///
    /// The cached links from the first one that is not `chain`'s, or whose
    /// files changed, are dropped, unless `chain` ends before it: a restore
    /// of an older snapshot of the same chain keeps the newer links.
    pub(crate) fn reuse(&mut self, chain: &[SnapshotId]) -> usize {
        let mut held = 0;
        while held < chain.len()
            && held < self.links.len()
            && self.links[held].snapshot == chain[held]
            && self.intact(held)
        {
            held += 1;
        }
        if held < chain.len() && held < self.links.len() {
            self.truncate(held);
        }
        held
    }

    /// True when the `index`th link has a bundle, which a restore counts.
    pub(crate) fn has_bundle(&self, index: usize) -> bool {
        self.links[index].bundle
    }

    /// The files of the `index`th link, to copy into a repository.
    pub(crate) fn files(&self, index: usize) -> impl Iterator<Item = (&str, PathBuf)> {
        self.links[index]
            .files
            .iter()
            .map(|file| (file.name.as_str(), self.dir.join(&file.name)))
    }

    /// Records `snapshot` as the next link, with or without a `bundle`,
    /// and the files its unpacking wrote, `written`, which are copied in.
    ///
    /// The caller adds the links of one chain in order, right after
    /// `reuse`. On an error the cache may hold part of the link, which the
    /// record does not name; the next `add` of that link replaces it.
    pub(crate) fn add(
        &mut self,
        snapshot: SnapshotId,
        bundle: bool,
        written: &[PathBuf],
    ) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let record = self.dir.join(RECORD);
        if self.links.is_empty() {
            fs::write(&record, RECORD_HEADER)?;
        }
        let mut files = Vec::with_capacity(written.len());
        for from in written {
            let name = from
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| pack_name(name))
                .ok_or_else(|| {
                    io::Error::other(format!("{} is not a pack file", from.display()))
                })?;
            let to = self.dir.join(name);
            match fs::remove_file(&to) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            place(from, &to)?;
            let (len, digest) = digest(&mut File::open(&to)?)?;
            files.push(CachedFile {
                name: name.to_owned(),
                len,
                digest,
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        crate::checkpoint::reached("restore.cache_files_placed");
        let link = CachedLink {
            snapshot,
            bundle,
            files,
        };
        fs::OpenOptions::new()
            .append(true)
            .open(&record)?
            .write_all(record_line(&link).as_bytes())?;
        self.links.push(link);
        Ok(())
    }

    /// Marks the cache as used now, so `evict` keeps it longest, and
    /// removes the caches of other jobs beyond `MAX_JOBS` or idle for
    /// longer than `MAX_IDLE`.
    pub(crate) fn touch_and_evict(&self) {
        let now = SystemTime::now();
        if let Ok(record) = fs::OpenOptions::new()
            .append(true)
            .open(self.dir.join(RECORD))
        {
            let _ = record.set_modified(now);
        }
        if let Some(root) = self.dir.parent() {
            evict(root, &self.dir, now, MAX_JOBS, MAX_IDLE);
        }
    }

    /// True when every file of the `index`th link has its recorded length and digest.
    fn intact(&self, index: usize) -> bool {
        self.links[index].files.iter().all(|file| {
            File::open(self.dir.join(&file.name))
                .and_then(|mut opened| digest(&mut opened))
                .is_ok_and(|found| found == (file.len, file.digest))
        })
    }

    /// Drops the links from `keep` on, their files and their lines.
    fn truncate(&mut self, keep: usize) {
        for link in self.links.drain(keep..) {
            for file in link.files {
                let _ = fs::remove_file(self.dir.join(&file.name));
            }
        }
        let mut text = RECORD_HEADER.to_owned();
        for link in &self.links {
            text.push_str(&record_line(link));
        }
        let record = self.dir.join(RECORD);
        let tmp = self.dir.join(format!("{RECORD}.tmp"));
        let rewritten = fs::write(&tmp, text).and_then(|()| fs::rename(&tmp, &record));
        if let Err(e) = rewritten {
            // A record that names dropped files only fails `intact` later.
            tracing::warn!(path = %record.display(), error = %e, "could not rewrite a bundle cache record");
            self.links.clear();
            let _ = fs::remove_file(&record);
        }
    }
}

/// Puts a copy of `from` at `to`, which must not exist: an APFS clone on
/// macOS, which shares no later write, and elsewhere a hard link, or a copy
/// where the two lie on different file systems. A cached file is checked
/// before every use, so a write through a link only drops it.
pub(crate) fn place(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        // `std::fs::copy` clones with `fclonefileat` where the volume can.
        fs::copy(from, to).map(|_| ())
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        fs::hard_link(from, to).or_else(|_| fs::copy(from, to).map(|_| ()))
    }
}

/// True for a file name `git index-pack` writes into `objects/pack`, and
/// nothing that could name a path elsewhere.
fn pack_name(name: &str) -> bool {
    name.starts_with("pack-")
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// The length of `file` and the blake3 digest of the blake3 digests of its
/// 1 MiB blocks, read with one block in memory.
fn digest(file: &mut File) -> io::Result<(u64, ChunkId)> {
    let mut buf = vec![0u8; BLOCK];
    let mut blocks = Vec::new();
    let mut len = 0u64;
    loop {
        let mut filled = 0;
        while filled < BLOCK {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if filled == 0 {
            break;
        }
        len += filled as u64;
        blocks.extend_from_slice(&ChunkId::of(&buf[..filled]).to_bytes());
        if filled < BLOCK {
            break;
        }
    }
    Ok((len, ChunkId::of(&blocks)))
}

/// One line of the record: the snapshot, `bundle` or `empty`, then `name:len:digest` per file.
fn record_line(link: &CachedLink) -> String {
    use std::fmt::Write as _;
    let mut text = link.snapshot.to_string();
    text.push_str(if link.bundle { " bundle" } else { " empty" });
    for file in &link.files {
        let _ = write!(text, " {}:{}:{}", file.name, file.len, file.digest);
    }
    text.push('\n');
    text
}

/// Reads the links of a record, oldest first, up to the first line that
/// does not parse or is cut short.
fn parse_record(text: &str) -> Vec<CachedLink> {
    let Some(body) = text.strip_prefix(RECORD_HEADER) else {
        return Vec::new();
    };
    let mut links = Vec::new();
    for line in body.split_inclusive('\n') {
        let Some(line) = line.strip_suffix('\n') else {
            break;
        };
        let mut fields = line.split(' ');
        let Some(snapshot) = fields.next().and_then(|id| id.parse().ok()) else {
            break;
        };
        let bundle = match fields.next() {
            Some("bundle") => true,
            Some("empty") => false,
            _ => break,
        };
        let files: Option<Vec<CachedFile>> = fields
            .map(|field| {
                let mut parts = field.split(':');
                let name = parts.next().filter(|name| pack_name(name))?;
                let len = parts.next()?.parse().ok()?;
                let digest = parts.next()?.parse().ok()?;
                parts.next().is_none().then(|| CachedFile {
                    name: name.to_owned(),
                    len,
                    digest,
                })
            })
            .collect();
        let Some(files) = files else {
            break;
        };
        links.push(CachedLink {
            snapshot,
            bundle,
            files,
        });
    }
    links
}

/// Removes, best effort, every cache under `root` but `keep` that no
/// restore used for `max_idle`, and the ones beyond the `max_jobs` used
/// last, `keep` counted among them.
fn evict(root: &Path, keep: &Path, now: SystemTime, max_jobs: usize, max_idle: Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut caches: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let used = fs::metadata(path.join(RECORD))
            .or_else(|_| entry.metadata())
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        caches.push((used, path));
    }
    caches.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
    for (rank, (used, path)) in caches.iter().enumerate() {
        let idle = now.duration_since(*used).unwrap_or_default();
        if (rank + 1 >= max_jobs || idle > max_idle)
            && let Err(e) = fs::remove_dir_all(path)
        {
            tracing::warn!(path = %path.display(), error = %e, "could not remove a bundle cache");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(n: u8) -> SnapshotId {
        SnapshotId::from_bytes([n; 32])
    }

    /// A cache in a fresh store root with one file per link, each holding `link <n>`.
    fn cache_with(root: &Path, links: u8) -> BundleCache {
        let staged = root.join("staged");
        fs::create_dir_all(&staged).unwrap();
        let mut cache = BundleCache::open(root, JobId::from_bytes([7; 16]));
        for n in 0..links {
            let file = staged.join(format!("pack-{n}.pack"));
            fs::write(&file, format!("link {n}")).unwrap();
            cache.add(snapshot(n), true, &[file]).unwrap();
        }
        cache
    }

    #[test]
    fn a_cache_reads_back_the_links_it_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_with(dir.path(), 3);
        let again = BundleCache::open(dir.path(), JobId::from_bytes([7; 16]));
        assert_eq!(again.links, cache.links);
        let chain = [snapshot(0), snapshot(1), snapshot(2)];
        let mut again = again;
        assert_eq!(again.reuse(&chain), 3);
        let (name, path) = again.files(1).next().unwrap();
        assert_eq!(name, "pack-1.pack");
        assert_eq!(fs::read_to_string(path).unwrap(), "link 1");
    }

    #[test]
    fn a_line_cut_short_ends_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_with(dir.path(), 2);
        let record = cache.dir.join(RECORD);
        let mut text = fs::read_to_string(&record).unwrap();
        text.truncate(text.len() - 5);
        fs::write(&record, text).unwrap();
        let again = BundleCache::open(dir.path(), JobId::from_bytes([7; 16]));
        assert_eq!(again.links.len(), 1);
        fs::write(&record, "fabric bundle cache 9\n").unwrap();
        assert!(
            BundleCache::open(dir.path(), JobId::from_bytes([7; 16]))
                .links
                .is_empty()
        );
    }

    #[test]
    fn a_name_that_could_leave_the_cache_is_refused() {
        for name in [
            "../pack-1.pack",
            "pack-1/../../x",
            "objects",
            "pack-1.pack/x",
        ] {
            assert!(!pack_name(name), "{name}");
        }
        let line = format!(
            "{RECORD_HEADER}{} bundle ../x:1:{}\n",
            snapshot(0),
            ChunkId::of(b"")
        );
        assert!(parse_record(&line).is_empty());
    }

    #[test]
    fn a_changed_or_missing_file_drops_its_link_and_the_ones_after() {
        let dir = tempfile::tempdir().unwrap();
        let chain = [snapshot(0), snapshot(1), snapshot(2)];
        for damage in [
            |path: &Path| fs::write(path, "link 9").unwrap(),
            |path: &Path| fs::write(path, "link").unwrap(),
            |path: &Path| fs::remove_file(path).unwrap(),
        ] {
            let mut cache = cache_with(dir.path(), 3);
            damage(&cache.dir.join("pack-1.pack"));
            assert_eq!(cache.reuse(&chain), 1);
            assert!(!cache.dir.join("pack-2.pack").exists());
            let again = BundleCache::open(dir.path(), JobId::from_bytes([7; 16]));
            assert_eq!(again.links.len(), 1);
            fs::remove_dir_all(dir.path().join(CACHE_DIR)).unwrap();
        }
    }

    #[test]
    fn a_chain_that_parts_drops_the_cached_links_from_there_and_an_older_one_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = cache_with(dir.path(), 3);
        assert_eq!(cache.reuse(&[snapshot(0), snapshot(1)]), 2);
        assert_eq!(cache.links.len(), 3);
        assert_eq!(cache.reuse(&[snapshot(0), snapshot(9), snapshot(10)]), 1);
        assert_eq!(cache.links.len(), 1);
        assert!(!cache.dir.join("pack-1.pack").exists());
        assert_eq!(cache.reuse(&[snapshot(5)]), 0);
        assert!(cache.links.is_empty());
    }

    #[test]
    fn eviction_keeps_the_caches_used_last_and_drops_idle_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        for (n, age) in [(1u64, 10), (2, 20), (3, 30), (4, 40), (5, 100)] {
            let cache = root.join(format!("job{n}"));
            fs::create_dir(&cache).unwrap();
            let record = File::create(cache.join(RECORD)).unwrap();
            record.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        let keep = root.join("job4");
        evict(root, &keep, now, 3, Duration::from_secs(50));
        let mut left: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["job1", "job2", "job4"]);
    }

    #[test]
    fn the_digest_covers_every_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut bytes = vec![0u8; BLOCK * 2 + 3];
        fs::write(&path, &bytes).unwrap();
        let first = digest(&mut File::open(&path).unwrap()).unwrap();
        assert_eq!(first.0, bytes.len() as u64);
        bytes[BLOCK + 1] = 1;
        fs::write(&path, &bytes).unwrap();
        assert_ne!(digest(&mut File::open(&path).unwrap()).unwrap(), first);
    }
}
