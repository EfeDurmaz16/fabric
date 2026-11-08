//! The SSH link: `ssh host <NAME> serve <path>` and a request and answer
//! protocol over its standard input and output.
//!
//! Each message is a frame: a 4-byte big-endian length and a JSON header,
//! then a 4-byte big-endian length and a blob, the stored bytes of objects
//! or a manifest. The client sends `hello` first, then one request at a
//! time and reads its answer. Every length is checked against a bound
//! before anything is allocated, every object is checked against its id by
//! the store that keeps it, and a manifest against its snapshot id, so the
//! other side's word decides nothing that is not checked.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use fabric_core::id::{ChunkId, SnapshotId};
use serde_json::{Map, Value, json};

use crate::objects::{MAX_BATCH_BYTES, MAX_MANIFEST_LEN, Objects, max_stored_len};
use crate::target::{DirTarget, Head, Heads, Remote, TreeName};
use crate::{NAME, SyncError};

/// The protocol version both sides speak.
const VERSION: u64 = 1;

/// The longest header either side reads.
const MAX_HEADER: usize = 1 << 20;

/// The most ids one request names.
const MAX_IDS: usize = 4096;

/// The longest blob either side reads: a manifest, or a batch of objects
/// whose last one may pass the batch bound by one stored object.
fn max_blob() -> usize {
    MAX_MANIFEST_LEN.max(MAX_BATCH_BYTES + max_stored_len())
}

fn bad(reason: impl Into<String>) -> SyncError {
    SyncError::Protocol {
        reason: reason.into(),
    }
}

fn write_frame(out: &mut dyn Write, header: &Value, blob: &[u8]) -> io::Result<()> {
    let header = header.to_string();
    let len = |n: usize| u32::try_from(n).map_err(|_| io::Error::other("frame too long"));
    out.write_all(&len(header.len())?.to_be_bytes())?;
    out.write_all(header.as_bytes())?;
    out.write_all(&len(blob.len())?.to_be_bytes())?;
    out.write_all(blob)?;
    out.flush()
}

/// A header and its blob.
type Frame = (Map<String, Value>, Vec<u8>);

/// A read that failed: the link, not the other side's data. A frame cut
/// short is a connection that went away, as one closed between frames is.
fn cut(what: &str) -> impl FnOnce(io::Error) -> SyncError + '_ {
    move |e| SyncError::Unreachable {
        reason: format!("the connection closed while reading {what}: {e}"),
    }
}

/// Reads one frame; `None` at a clean end of input before a frame starts.
fn read_frame(input: &mut dyn Read) -> Result<Option<Frame>, SyncError> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(cut("a frame")(e)),
    }
    let header_len = u32::from_be_bytes(len) as usize;
    if header_len > MAX_HEADER {
        return Err(bad(format!("a header of {header_len} bytes")));
    }
    let mut header = vec![0u8; header_len];
    input.read_exact(&mut header).map_err(cut("a header"))?;
    input.read_exact(&mut len).map_err(cut("a frame"))?;
    let blob_len = u32::from_be_bytes(len) as usize;
    if blob_len > max_blob() {
        return Err(bad(format!("a blob of {blob_len} bytes")));
    }
    let mut blob = vec![0u8; blob_len];
    input.read_exact(&mut blob).map_err(cut("a blob"))?;
    match serde_json::from_slice(&header) {
        Ok(Value::Object(map)) => Ok(Some((map, blob))),
        _ => Err(bad("a header that is not a JSON object")),
    }
}

fn text<'a>(header: &'a Map<String, Value>, key: &str) -> Result<&'a str, SyncError> {
    header
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(format!("no {key} in {}", Value::Object(header.clone()))))
}

fn parse<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, SyncError> {
    text.parse()
        .map_err(|_| bad(format!("{text} is not a {what}")))
}

fn ids(header: &Map<String, Value>, key: &str) -> Result<Vec<ChunkId>, SyncError> {
    let list = header
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("no {key} list")))?;
    if list.len() > MAX_IDS {
        return Err(bad(format!("{} ids in one request", list.len())));
    }
    list.iter()
        .map(|id| parse(id.as_str().unwrap_or_default(), "object id"))
        .collect()
}

fn lens(header: &Map<String, Value>, count: usize, blob: usize) -> Result<Vec<usize>, SyncError> {
    let list = header
        .get("lens")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no lens list"))?;
    let lens: Vec<usize> = list
        .iter()
        .map(|n| n.as_u64().and_then(|n| usize::try_from(n).ok()))
        .collect::<Option<_>>()
        .ok_or_else(|| bad("a length that is not a number"))?;
    let sum = lens.iter().try_fold(0usize, |sum, n| sum.checked_add(*n));
    if lens.len() != count || sum != Some(blob) {
        return Err(bad("the lengths do not match the blob"));
    }
    Ok(lens)
}

fn split(blob: &[u8], lens: &[usize]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(lens.len());
    let mut at = 0;
    for &len in lens {
        out.push(blob[at..at + len].to_vec());
        at += len;
    }
    out
}

fn id_list(ids: &[ChunkId]) -> Value {
    Value::from(ids.iter().map(ToString::to_string).collect::<Vec<_>>())
}

fn head_value(head: Option<Head>) -> Value {
    head.map_or(
        Value::Null,
        |head| json!({ "job": head.job.to_string(), "snapshot": head.snapshot.to_string() }),
    )
}

fn read_head(value: Option<&Value>) -> Result<Option<Head>, SyncError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(Head {
            job: parse(text(map, "job")?, "job id")?,
            snapshot: parse(text(map, "snapshot")?, "snapshot id")?,
        })),
        Some(other) => Err(bad(format!("a head that reads {other}"))),
    }
}

/// Expands a leading `~` to `home`.
fn expand(path: &str, home: Option<&Path>) -> Result<PathBuf, SyncError> {
    let rest = match path.strip_prefix('~') {
        None => return Ok(PathBuf::from(path)),
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/'),
        Some(_) => return Err(bad(format!("{path}: only ~ and ~/ name a home"))),
    };
    let home = home.ok_or_else(|| bad("no HOME to expand ~ in"))?;
    Ok(home.join(rest))
}

/// Answers one client on `input` and `output` for the target directory at `path`.
///
/// This is the other end of an SSH target: `ssh` starts it and it exits
/// when the client says `bye` or closes its input. The target is opened at
/// `hello`, created then when the client pushes.
pub fn serve(path: &str, input: impl Read, output: impl Write) -> Result<(), SyncError> {
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dir = expand(path, home.as_deref())?;
    let mut target: Option<DirTarget> = None;
    while let Some((header, blob)) = read_frame(&mut input)? {
        let op = text(&header, "op")?.to_owned();
        if op == "bye" {
            write_frame(&mut output, &json!({ "ok": true }), &[]).map_err(lost)?;
            return Ok(());
        }
        let answered = if op == "hello" {
            hello(&header, &dir, &mut target)
        } else if let Some(target) = target.as_mut() {
            answer(target, &op, &header, &blob)
        } else {
            Err(bad("a request before hello"))
        };
        let (header, blob) = match answered {
            Ok(answer) => answer,
            Err(SyncError::Protocol { reason }) => return Err(SyncError::Protocol { reason }),
            Err(e) => (refusal(&e), Vec::new()),
        };
        write_frame(&mut output, &header, &blob).map_err(lost)?;
    }
    Ok(())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "it is the map_err of an io::Error"
)]
fn lost(e: io::Error) -> SyncError {
    SyncError::Unreachable {
        reason: e.to_string(),
    }
}

fn refusal(e: &SyncError) -> Value {
    match e {
        SyncError::TargetMoved { head, .. } => {
            json!({ "ok": false, "kind": "moved", "error": e.to_string(), "head": head.to_string() })
        }
        SyncError::BadTree { .. } => {
            json!({ "ok": false, "kind": "absent", "error": e.to_string() })
        }
        _ => json!({ "ok": false, "kind": "refused", "error": e.to_string() }),
    }
}

fn hello(
    header: &Map<String, Value>,
    dir: &Path,
    target: &mut Option<DirTarget>,
) -> Result<(Value, Vec<u8>), SyncError> {
    let version = header.get("version").and_then(Value::as_u64);
    if version != Some(VERSION) {
        return Err(SyncError::Refused {
            reason: format!("{NAME} there speaks version {VERSION}, not {version:?}"),
        });
    }
    let create = header.get("create").and_then(Value::as_bool) == Some(true);
    *target = Some(DirTarget::open(dir, create)?);
    Ok((json!({ "ok": true, "version": VERSION }), Vec::new()))
}

fn answer(
    target: &mut DirTarget,
    op: &str,
    header: &Map<String, Value>,
    blob: &[u8],
) -> Result<(Value, Vec<u8>), SyncError> {
    let snapshot =
        || -> Result<SnapshotId, SyncError> { parse(text(header, "id")?, "snapshot id") };
    let name = || TreeName::parse(text(header, "name")?);
    let done = |value: Value| Ok((value, Vec::new()));
    match op {
        "has_snapshot" => done(json!({ "ok": true, "has": target.has_snapshot(snapshot()?)? })),
        "manifest_chunks" => {
            let (len, ids) = target.manifest_chunks(snapshot()?)?;
            done(json!({ "ok": true, "len": len, "ids": id_list(&ids) }))
        }
        "missing" => {
            let missing = target.missing(&ids(header, "ids")?)?;
            done(json!({ "ok": true, "missing": id_list(&missing) }))
        }
        "get" => {
            let got = target.get(&ids(header, "ids")?)?;
            let lens: Vec<usize> = got.iter().map(Vec::len).collect();
            Ok((json!({ "ok": true, "lens": lens }), got.concat()))
        }
        "put" => {
            let ids = ids(header, "ids")?;
            let lens = lens(header, ids.len(), blob.len())?;
            let chunks: Vec<(ChunkId, Vec<u8>)> = ids.into_iter().zip(split(blob, &lens)).collect();
            target.put(&chunks)?;
            done(json!({ "ok": true }))
        }
        "publish" => {
            let id = target.publish(blob)?;
            done(json!({ "ok": true, "id": id.to_string() }))
        }
        "head" => done(json!({ "ok": true, "head": head_value(target.head(&name()?)?) })),
        "set_head" => {
            let expected = match header.get("expected") {
                None | Some(Value::Null) => None,
                Some(value) => Some(parse(value.as_str().unwrap_or_default(), "snapshot id")?),
            };
            let new = read_head(header.get("new"))?.ok_or_else(|| bad("no new head"))?;
            target.set_head(&name()?, expected, new)?;
            done(json!({ "ok": true }))
        }
        other => Err(bad(format!("an unknown request {other}"))),
    }
}

/// `word` quoted for a POSIX shell when it needs to be.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The remote path as the remote shell should see it: `~/` left for it to expand, the rest quoted.
fn remote_path(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) if !rest.is_empty() => format!("~/{}", shell_word(rest)),
        Some(_) => "~".to_owned(),
        None if path == "~" => "~".to_owned(),
        None => shell_word(path),
    }
}

/// The last bytes the SSH process wrote on its standard error.
type Tail = Arc<Mutex<Vec<u8>>>;

/// How much of the SSH process's standard error is kept for an error.
const TAIL: usize = 4096;

/// A target behind `ssh`.
pub(crate) struct SshRemote {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    tail: Tail,
    reader: Option<JoinHandle<()>>,
    /// The target path there, for errors.
    path: String,
}

impl SshRemote {
    /// Starts `ssh` and says hello; with `create`, the target is made when absent.
    ///
    /// The program is `<NAME>_SSH` when set, else `ssh`, and the command it
    /// runs there is `<NAME>_REMOTE` when set, else `NAME`, found on the
    /// remote `PATH`. The host follows `--`, so no host is read as an option.
    pub(crate) fn connect(
        host: &str,
        port: Option<u16>,
        path: &str,
        create: bool,
    ) -> Result<SshRemote, SyncError> {
        let program = std::env::var_os(crate::env_var("SSH")).unwrap_or_else(|| "ssh".into());
        let remote = std::env::var(crate::env_var("REMOTE")).unwrap_or_else(|_| NAME.to_owned());
        let mut command = Command::new(&program);
        command.arg("-T");
        if let Some(port) = port {
            command.arg("-p").arg(port.to_string());
        }
        command
            .arg("--")
            .arg(host)
            .arg(format!(
                "{} serve {}",
                shell_word(&remote),
                remote_path(path)
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| SyncError::Unreachable {
            reason: format!("{} did not start: {e}", Path::new(&program).display()),
        })?;
        let (Some(input), Some(output), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(SyncError::Unreachable {
                reason: "no pipes to ssh".to_owned(),
            });
        };
        let tail: Tail = Arc::default();
        let kept = Arc::clone(&tail);
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
                kept.extend_from_slice(&buf[..n]);
                let over = kept.len().saturating_sub(TAIL);
                kept.drain(..over);
            }
        });
        let mut remote = SshRemote {
            child,
            input: BufWriter::new(input),
            output: BufReader::new(output),
            tail,
            reader: Some(reader),
            path: path.to_owned(),
        };
        remote.call(
            &json!({ "op": "hello", "version": VERSION, "create": create }),
            &[],
        )?;
        Ok(remote)
    }

    /// What ssh said on its standard error, when it said anything.
    fn said(&mut self) -> String {
        if let Some(reader) = self.reader.take() {
            let _ = self.child.wait();
            let _ = reader.join();
        }
        let tail = self.tail.lock().unwrap_or_else(PoisonError::into_inner);
        let text = String::from_utf8_lossy(&tail).trim().to_owned();
        if text.is_empty() {
            String::new()
        } else {
            format!(": {text}")
        }
    }

    /// Sends one request and reads its answer; a refusal is an error.
    fn call(&mut self, header: &Value, blob: &[u8]) -> Result<Frame, SyncError> {
        let sent = write_frame(&mut self.input, header, blob);
        let answer = match sent {
            Ok(()) => read_frame(&mut self.output),
            Err(e) => Err(lost(e)),
        };
        let (answer, blob) = match answer {
            Ok(Some(answer)) => answer,
            Ok(None) | Err(SyncError::Unreachable { .. }) => {
                let said = self.said();
                return Err(SyncError::Unreachable {
                    reason: format!("the connection closed{said}"),
                });
            }
            Err(e) => return Err(e),
        };
        if answer.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok((answer, blob));
        }
        let error = answer
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("no reason")
            .to_owned();
        Err(match answer.get("kind").and_then(Value::as_str) {
            Some("moved") => SyncError::TargetMoved {
                name: header
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                head: parse(text(&answer, "head")?, "snapshot id")?,
            },
            Some("absent") => SyncError::BadTree {
                path: PathBuf::from(&self.path),
                why: error,
            },
            _ => SyncError::Refused { reason: error },
        })
    }
}

impl Objects for SshRemote {
    fn has_snapshot(&mut self, id: SnapshotId) -> Result<bool, SyncError> {
        let (answer, _) = self.call(&json!({ "op": "has_snapshot", "id": id.to_string() }), &[])?;
        answer
            .get("has")
            .and_then(Value::as_bool)
            .ok_or_else(|| bad("no has in the answer"))
    }

    fn manifest_chunks(&mut self, id: SnapshotId) -> Result<(u64, Vec<ChunkId>), SyncError> {
        let (answer, _) = self.call(
            &json!({ "op": "manifest_chunks", "id": id.to_string() }),
            &[],
        )?;
        let len = answer
            .get("len")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("no len in the answer"))?;
        Ok((len, ids(&answer, "ids")?))
    }

    fn missing(&mut self, asked: &[ChunkId]) -> Result<Vec<ChunkId>, SyncError> {
        let (answer, _) = self.call(&json!({ "op": "missing", "ids": id_list(asked) }), &[])?;
        let missing = ids(&answer, "missing")?;
        let asked: std::collections::HashSet<&ChunkId> = asked.iter().collect();
        if missing.iter().any(|id| !asked.contains(id)) {
            return Err(bad("the answer names an object not asked about"));
        }
        Ok(missing)
    }

    fn get(&mut self, asked: &[ChunkId]) -> Result<Vec<Vec<u8>>, SyncError> {
        let asked = &asked[..asked.len().min(MAX_IDS)];
        let (answer, blob) = self.call(&json!({ "op": "get", "ids": id_list(asked) }), &[])?;
        let list = answer
            .get("lens")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("no lens list"))?;
        if list.is_empty() || list.len() > asked.len() {
            return Err(bad("an answer with no objects or more than asked"));
        }
        let lens = lens(&answer, list.len(), blob.len())?;
        Ok(split(&blob, &lens))
    }

    fn put(&mut self, chunks: &[(ChunkId, Vec<u8>)]) -> Result<(), SyncError> {
        let ids: Vec<ChunkId> = chunks.iter().map(|(id, _)| *id).collect();
        let lens: Vec<usize> = chunks.iter().map(|(_, bytes)| bytes.len()).collect();
        let blob: Vec<u8> = chunks
            .iter()
            .flat_map(|(_, bytes)| bytes.iter().copied())
            .collect();
        self.call(
            &json!({ "op": "put", "ids": id_list(&ids), "lens": lens }),
            &blob,
        )?;
        Ok(())
    }

    fn publish(&mut self, manifest: &[u8]) -> Result<SnapshotId, SyncError> {
        let (answer, _) = self.call(&json!({ "op": "publish" }), manifest)?;
        parse(text(&answer, "id")?, "snapshot id")
    }
}

impl Heads for SshRemote {
    fn head(&mut self, name: &TreeName) -> Result<Option<Head>, SyncError> {
        let (answer, _) = self.call(&json!({ "op": "head", "name": name.as_str() }), &[])?;
        read_head(answer.get("head"))
    }

    fn set_head(
        &mut self,
        name: &TreeName,
        expected: Option<SnapshotId>,
        new: Head,
    ) -> Result<(), SyncError> {
        let expected = expected.map_or(Value::Null, |id| Value::from(id.to_string()));
        self.call(
            &json!({ "op": "set_head", "name": name.as_str(), "expected": expected, "new": head_value(Some(new)) }),
            &[],
        )?;
        Ok(())
    }
}

impl Remote for SshRemote {
    fn close(mut self: Box<Self>) -> Result<(), SyncError> {
        self.call(&json!({ "op": "bye" }), &[])?;
        let status = self.child.wait().map_err(lost)?;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if status.success() {
            Ok(())
        } else {
            let said = self.said();
            Err(SyncError::Unreachable {
                reason: format!("ssh exited with {status}{said}"),
            })
        }
    }
}

impl Drop for SshRemote {
    fn drop(&mut self) {
        // A remote left without `bye` sees its input close and exits.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_path_is_quoted_and_keeps_its_home() {
        assert_eq!(remote_path("~/sync"), "~/sync");
        assert_eq!(remote_path("~/my sync"), "~/'my sync'");
        assert_eq!(remote_path("/srv/a;rm -rf x"), "'/srv/a;rm -rf x'");
        assert_eq!(remote_path("~"), "~");
    }

    #[test]
    fn a_frame_longer_than_its_bound_is_refused_before_it_is_read() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            read_frame(&mut frame.as_slice()),
            Err(SyncError::Protocol { .. })
        ));
    }

    #[test]
    fn a_frame_cut_short_is_a_lost_link_not_bad_data() {
        let mut frame = Vec::new();
        write_frame(&mut frame, &json!({ "ok": true }), &[7; 64]).unwrap();
        for at in [6, frame.len() - 70, frame.len() - 1] {
            assert!(
                matches!(
                    read_frame(&mut &frame[..at]),
                    Err(SyncError::Unreachable { .. })
                ),
                "cut at {at}"
            );
        }
        assert!(read_frame(&mut &frame[..0]).unwrap().is_none());
    }

    #[test]
    fn home_expands_only_as_tilde_or_tilde_slash() {
        let home = Path::new("/home/u");
        assert_eq!(expand("~/x", Some(home)).unwrap(), Path::new("/home/u/x"));
        assert_eq!(expand("~", Some(home)).unwrap(), Path::new("/home/u"));
        assert_eq!(expand("/x", Some(home)).unwrap(), Path::new("/x"));
        assert!(expand("~other/x", Some(home)).is_err());
    }
}
