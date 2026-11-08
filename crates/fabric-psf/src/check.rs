//! The rules of section 10, in the order a reader checks them.
//!
//! Every phase walks the whole value in canonical order (keys by their
//! bytes, depth first, a node before its children) and stops at the first
//! value that breaks it, so the code and the field a reader reports are
//! the same in every reader. The other two readers (`scripts/psf_check.py`
//! and `sdk/typescript/src/psf.ts`) follow this file phase by phase.

use serde_json::{Map, Value};

use crate::canon::{self, NoForm, child, element, sorted_keys};
use crate::refusal::{Code, Refusal};

/// The manifest cap: the client API's request parse cap.
pub const MANIFEST_MAX_BYTES: usize = 1 << 20;
/// The cap on a text field.
pub const TEXT_MAX_BYTES: usize = 16 * 1024;
/// The cap on the canonical JSON of `package`.
pub const PACKAGE_MAX_BYTES: usize = 256 * 1024;
/// The cap on a name: `produced.writer`, `attachments[].name`, `attachments[].format`.
pub const NAME_MAX_BYTES: usize = 128;
/// The cap on `caches`.
pub const CACHES_MAX: usize = 256;
/// The cap on `evidence`, `secrets.left_behind` and `secrets.allowlist`.
pub const LIST_MAX: usize = 1024;

/// Checks `bytes` and returns the canonical value they hold.
pub(crate) fn check(bytes: &[u8]) -> Result<Value, Refusal> {
    if bytes.len() > MANIFEST_MAX_BYTES {
        return Err(Refusal::new(Code::TooLarge, ""));
    }
    let value = parse(bytes).ok_or_else(|| Refusal::new(Code::NotJson, ""))?;
    let Value::Object(top) = &value else {
        return Err(Refusal::new(Code::NotJson, ""));
    };
    // Before the form: a v2 document may change a canonical rule, and a
    // v1 reader still names it by its version (section 11).
    if let Some(version) = top.get("psf")
        && version.as_str() != Some("1")
    {
        return Err(Refusal::new(Code::UnsupportedVersion, "psf"));
    }
    match canon::write(&value) {
        Err(NoForm::Number(at) | NoForm::Key(at)) => {
            return Err(Refusal::new(Code::NotCanonical, at));
        }
        Ok(text) if text.as_bytes() != bytes => return Err(Refusal::new(Code::NotCanonical, "")),
        Ok(_) => {}
    }
    shape(top)?;
    if let Some(Value::Array(requires)) = top.get("requires")
        && !requires.is_empty()
    {
        return Err(Refusal::new(Code::UnknownRequirement, "requires[0]"));
    }
    first(&value, Code::TooLarge, too_large)?;
    first(&value, Code::BadId, bad_id)?;
    first(&value, Code::BadPath, bad_path)?;
    first(&value, Code::BadEnum, bad_enum)?;
    duplicates(top)?;
    if str_at(top, &["secrets", "policy"]) != Some("never_inside") {
        return Err(Refusal::new(Code::SecretPolicy, "secrets.policy"));
    }
    credential(&value, "")?;
    fidelity(top)?;
    package(top)?;
    Ok(value)
}

/// One JSON object from UTF-8 bytes, with no lone surrogate, or `None`.
fn parse(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: Value = serde_json::from_str(text).ok()?;
    value.is_object().then_some(value)
}

fn str_at<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    let (last, parents) = keys.split_last()?;
    let mut map = map;
    for key in parents {
        map = map.get(*key)?.as_object()?;
    }
    map.get(*last)?.as_str()
}

/// What a schema node expects.
#[derive(Clone, Copy)]
enum Want {
    /// A string.
    Str,
    /// A string or `null`.
    StrOrNull,
    /// A string of at least one byte.
    Name,
    /// An array of strings.
    Strings,
    /// An object with these fields.
    Obj(&'static [(&'static str, Want)]),
    /// An object with these fields, or `null`.
    ObjOrNull(&'static [(&'static str, Want)]),
    /// An array of objects with these fields.
    List(&'static [(&'static str, Want)]),
    /// `source`: `kind`, then the fields its kind requires.
    Source,
    /// `caches[].key`: `null`, or an object with a string `kind` whose every value is a string.
    Key,
    /// `package`: `null`, or an object with the fields section 5.6 reads.
    Package,
}

const BOUND: &[(&str, Want)] = &[("home", Want::Str), ("root", Want::Str)];
const ATTACHMENT: &[(&str, Want)] = &[
    ("algorithm", Want::Str),
    ("bound", Want::ObjOrNull(BOUND)),
    ("bytes", Want::Str),
    ("digest", Want::Str),
    ("fidelity", Want::Str),
    ("format", Want::Name),
    ("harness", Want::Str),
    ("name", Want::Str),
    ("session", Want::StrOrNull),
];
const CACHE: &[(&str, Want)] = &[
    ("carry", Want::Str),
    ("class", Want::Str),
    ("digest", Want::StrOrNull),
    ("key", Want::Key),
    ("path", Want::Str),
];
const EVIDENCE: &[(&str, Want)] = &[
    ("attempt", Want::Str),
    ("digest", Want::Str),
    ("kind", Want::Str),
];
const FIDELITY: &[(&str, Want)] = &[("level", Want::Str), ("reason", Want::StrOrNull)];
const HARNESS: &[(&str, Want)] = &[
    ("args", Want::Strings),
    ("kind", Want::Str),
    ("program", Want::StrOrNull),
    ("version", Want::StrOrNull),
];
const JOB: &[(&str, Want)] = &[
    ("created_by", Want::Str),
    ("goal", Want::Str),
    ("id", Want::Str),
];
const PRODUCED: &[(&str, Want)] = &[
    ("at_ms", Want::Str),
    ("by", Want::Str),
    ("writer", Want::Name),
];
const SECRETS: &[(&str, Want)] = &[
    ("allowlist", Want::Strings),
    ("left_behind", Want::Strings),
    ("policy", Want::Str),
];
const PLATFORM: &[(&str, Want)] = &[("arch", Want::Str), ("os", Want::Str)];
const WORKSPACE: &[(&str, Want)] = &[
    ("bundle_head", Want::Str),
    ("epoch", Want::Str),
    ("ledger_head", Want::Str),
    ("platform", Want::Obj(PLATFORM)),
    ("root", Want::Str),
];
const HEADS: &[(&str, Want)] = &[("bundle_head", Want::Str), ("ledger_head", Want::Str)];
const PACKAGE: &[(&str, Want)] = &[
    ("job", Want::Str),
    ("version", Want::Str),
    ("workspace", Want::Obj(HEADS)),
];
/// The top level, keys in canonical order.
const TOP: &[(&str, Want)] = &[
    ("attachments", Want::List(ATTACHMENT)),
    ("caches", Want::List(CACHE)),
    ("evidence", Want::List(EVIDENCE)),
    ("fidelity", Want::Obj(FIDELITY)),
    ("harness", Want::Obj(HARNESS)),
    ("job", Want::Obj(JOB)),
    ("package", Want::Package),
    ("produced", Want::Obj(PRODUCED)),
    ("psf", Want::Str),
    ("requires", Want::Strings),
    ("secrets", Want::Obj(SECRETS)),
    ("source", Want::Source),
    ("workspace", Want::Obj(WORKSPACE)),
];

/// `missing_field`: every key v1.0 defines, in canonical order, present
/// and of its JSON type.
fn shape(top: &Map<String, Value>) -> Result<(), Refusal> {
    fields(top, TOP, "")
}

fn fields(map: &Map<String, Value>, want: &[(&str, Want)], path: &str) -> Result<(), Refusal> {
    for (key, want) in want {
        node(map.get(*key), *want, &child(path, key))?;
    }
    Ok(())
}

fn node(value: Option<&Value>, want: Want, path: &str) -> Result<(), Refusal> {
    let missing = || Refusal::new(Code::MissingField, path);
    match (want, value) {
        (Want::Str, Some(Value::String(_)))
        | (Want::StrOrNull | Want::ObjOrNull(_) | Want::Key | Want::Package, Some(Value::Null)) => {
            Ok(())
        }
        (Want::StrOrNull, Some(Value::String(_))) => Ok(()),
        (Want::Name, Some(Value::String(text))) if !text.is_empty() => Ok(()),
        (Want::Strings, Some(Value::Array(items))) => {
            for (index, item) in items.iter().enumerate() {
                node(Some(item), Want::Str, &element(path, index))?;
            }
            Ok(())
        }
        (Want::Obj(want) | Want::ObjOrNull(want), Some(Value::Object(map))) => {
            fields(map, want, path)
        }
        (Want::List(want), Some(Value::Array(items))) => {
            for (index, item) in items.iter().enumerate() {
                node(Some(item), Want::Obj(want), &element(path, index))?;
            }
            Ok(())
        }
        (Want::Source, Some(Value::Object(map))) => {
            node(map.get("kind"), Want::Str, &child(path, "kind"))?;
            let required: &[&str] = match map.get("kind").and_then(Value::as_str) {
                Some("git") => &["commit", "uri"],
                Some("snapshot") => &["snapshot"],
                _ => &[],
            };
            for key in required {
                node(map.get(*key), Want::Str, &child(path, key))?;
            }
            Ok(())
        }
        (Want::Key, Some(Value::Object(map))) => {
            node(map.get("kind"), Want::Str, &child(path, "kind"))?;
            for key in sorted_keys(map) {
                node(map.get(key), Want::Str, &child(path, key))?;
            }
            Ok(())
        }
        (Want::Package, Some(Value::Object(map))) => fields(map, PACKAGE, path),
        _ => Err(missing()),
    }
}

/// Visits every value in canonical order, a node before its children,
/// with its path and its pattern (the path with every index as `[]`).
fn walk<F>(value: &Value, path: &str, pattern: &str, f: &mut F) -> Option<String>
where
    F: FnMut(&str, &Value) -> bool,
{
    if f(pattern, value) {
        return Some(path.to_owned());
    }
    match value {
        Value::Array(items) => {
            let pattern = format!("{pattern}[]");
            items
                .iter()
                .enumerate()
                .find_map(|(index, item)| walk(item, &element(path, index), &pattern, f))
        }
        Value::Object(map) => sorted_keys(map).into_iter().find_map(|key| {
            let item = map.get(key)?;
            walk(item, &child(path, key), &child(pattern, key), f)
        }),
        _ => None,
    }
}

fn first<F>(value: &Value, code: Code, mut rule: F) -> Result<(), Refusal>
where
    F: FnMut(&str, &Value) -> bool,
{
    match walk(value, "", "", &mut rule) {
        Some(field) => Err(Refusal::new(code, field)),
        None => Ok(()),
    }
}

fn under_package(pattern: &str) -> bool {
    pattern.starts_with("package.") || pattern.starts_with("package[")
}

/// `too_large`: the limits of section 7.
fn too_large(pattern: &str, value: &Value) -> bool {
    match value {
        Value::Array(items) => {
            let cap = match pattern {
                "requires" => 16,
                "harness.args" | "attachments" => 64,
                "caches" => CACHES_MAX,
                "evidence" | "secrets.left_behind" | "secrets.allowlist" => LIST_MAX,
                _ => return false,
            };
            items.len() > cap
        }
        Value::String(text) => {
            let cap = match pattern {
                "produced.writer" | "attachments[].name" | "attachments[].format" => NAME_MAX_BYTES,
                "job.goal" | "fidelity.reason" => TEXT_MAX_BYTES,
                p if under_package(p) => TEXT_MAX_BYTES,
                _ => return false,
            };
            text.len() > cap
        }
        Value::Object(_) if pattern == "package" => {
            canon::write(value).map_or(true, |text| text.len() > PACKAGE_MAX_BYTES)
        }
        _ => false,
    }
}

/// `bad_id`: the text forms of section 4, rules 4, 7 and 8.
fn bad_id(pattern: &str, value: &Value) -> bool {
    let Value::String(text) = value else {
        return false;
    };
    match pattern {
        "job.id" | "evidence[].attempt" => !forms::ulid(text),
        "job.created_by" | "produced.by" => !forms::node_id(text),
        "attachments[].session" => !forms::session(text),
        "attachments[].name" => !forms::attachment_name(text),
        "attachments[].digest"
        | "workspace.bundle_head"
        | "workspace.ledger_head"
        | "source.snapshot"
        | "caches[].digest"
        | "evidence[].digest" => !forms::hex(text, &[64]),
        "source.commit" => !forms::hex(text, &[40, 64]),
        "produced.at_ms" | "attachments[].bytes" => forms::uint(text).is_none(),
        "workspace.epoch" => forms::uint(text).is_none_or(|n| n == 0),
        _ => false,
    }
}

/// `bad_path`: the path rules of section 4, rule 9.
fn bad_path(pattern: &str, value: &Value) -> bool {
    let Value::String(text) = value else {
        return false;
    };
    match pattern {
        "workspace.root" | "attachments[].bound.root" | "attachments[].bound.home" => {
            !forms::abs_path(text)
        }
        "caches[].path" | "secrets.left_behind[]" | "secrets.allowlist[]" => !forms::rel_path(text),
        _ => false,
    }
}

/// `bad_enum`: the closed sets, and the harness alphabet.
fn bad_enum(pattern: &str, value: &Value) -> bool {
    let Value::String(text) = value else {
        return false;
    };
    let set: &[&str] = match pattern {
        "source.kind" => &["empty", "git", "snapshot"],
        "caches[].class" => &["build", "deps", "tool_cache"],
        "caches[].carry" => &["carried", "not_carried", "partial"],
        "attachments[].fidelity" => &["native_session", "new_native_session_with_package"],
        "attachments[].algorithm" => &["sha256"],
        "evidence[].kind" => &["checkpoint", "exit", "receipt"],
        "fidelity.level" => &[
            "native_session",
            "new_native_session_with_package",
            "unsupported",
            "workspace_only",
        ],
        "harness.kind" | "attachments[].harness" => return !forms::harness_kind(text),
        _ => return false,
    };
    !set.contains(&text.as_str())
}

/// `duplicate`: attachments by name, then caches by path, strictly ascending by bytes.
fn duplicates(top: &Map<String, Value>) -> Result<(), Refusal> {
    for (list, key) in [("attachments", "name"), ("caches", "path")] {
        let Some(Value::Array(items)) = top.get(list) else {
            continue;
        };
        let names: Vec<&str> = items
            .iter()
            .map(|item| item.get(key).and_then(Value::as_str).unwrap_or_default())
            .collect();
        for (index, pair) in names.windows(2).enumerate() {
            if let [before, after] = pair
                && before.as_bytes() >= after.as_bytes()
            {
                return Err(Refusal::new(
                    Code::Duplicate,
                    child(&element(list, index + 1), key),
                ));
            }
        }
    }
    Ok(())
}

/// `credential_like`: every key and string value, in canonical order.
fn credential(value: &Value, path: &str) -> Result<(), Refusal> {
    let shaped = fabric_core::credential::looks_like_credential;
    match value {
        Value::String(text) if shaped(text) => Err(Refusal::new(Code::CredentialLike, path)),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                credential(item, &element(path, index))?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for key in sorted_keys(map) {
                let at = child(path, key);
                if shaped(key) {
                    return Err(Refusal::new(Code::CredentialLike, at));
                }
                if let Some(item) = map.get(key) {
                    credential(item, &at)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The rank of an attachment fidelity; higher is better.
fn rank(level: &str) -> u8 {
    match level {
        "native_session" => 2,
        "new_native_session_with_package" => 1,
        _ => 0,
    }
}

/// `fidelity_mismatch`: section 5.6.
fn fidelity(top: &Map<String, Value>) -> Result<(), Refusal> {
    let mismatch = |field: &str| Err(Refusal::new(Code::FidelityMismatch, field));
    let kind = str_at(top, &["harness", "kind"]).unwrap_or_default();
    let attachments = top
        .get("attachments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for (index, item) in attachments.iter().enumerate() {
        let get = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default();
        if get("fidelity") == "native_session" && get("harness") != kind {
            return mismatch(&child(&element("attachments", index), "harness"));
        }
    }
    let level = str_at(top, &["fidelity", "level"]).unwrap_or_default();
    let has_reason = top
        .get("fidelity")
        .and_then(|f| f.get("reason"))
        .is_some_and(|reason| !reason.is_null());
    if level == "unsupported" {
        if !attachments.is_empty() {
            return mismatch("fidelity.level");
        }
        if !has_reason {
            return mismatch("fidelity.reason");
        }
    } else {
        if has_reason {
            return mismatch("fidelity.reason");
        }
        let best = attachments
            .iter()
            .map(|item| {
                rank(
                    item.get("fidelity")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .max()
            .unwrap_or(0);
        let derived = match best {
            2 => "native_session",
            1 => "new_native_session_with_package",
            _ => "workspace_only",
        };
        if level != derived {
            return mismatch("fidelity.level");
        }
    }
    if level == "new_native_session_with_package" && top.get("package").is_none_or(Value::is_null) {
        return mismatch("package");
    }
    Ok(())
}

/// `package_mismatch`: section 5.6.
fn package(top: &Map<String, Value>) -> Result<(), Refusal> {
    let Some(Value::Object(package)) = top.get("package") else {
        return Ok(());
    };
    let pairs: [(&[&str], Option<&str>, &str); 4] = [
        (&["version"], Some("1"), "package.version"),
        (&["job"], str_at(top, &["job", "id"]), "package.job"),
        (
            &["workspace", "bundle_head"],
            str_at(top, &["workspace", "bundle_head"]),
            "package.workspace.bundle_head",
        ),
        (
            &["workspace", "ledger_head"],
            str_at(top, &["workspace", "ledger_head"]),
            "package.workspace.ledger_head",
        ),
    ];
    for (keys, want, field) in pairs {
        if str_at(package, keys) != want {
            return Err(Refusal::new(Code::PackageMismatch, field));
        }
    }
    Ok(())
}

/// The text forms the rules name.
pub(crate) mod forms {
    use fabric_core::path::{AbsPath, RelPath};

    /// A ULID in uppercase Crockford base32, 26 characters, at most `7ZZZ...`.
    pub(crate) fn ulid(text: &str) -> bool {
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        text.len() == 26
            && text.bytes().all(|b| ALPHABET.contains(&b))
            && text.as_bytes().first().is_some_and(|b| *b <= b'7')
    }

    /// `fab1` and 52 characters of lowercase base32 whose last four bits are zero.
    pub(crate) fn node_id(text: &str) -> bool {
        let Some(body) = text.strip_prefix("fab1") else {
            return false;
        };
        body.len() == 52
            && body
                .bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
            && body
                .as_bytes()
                .last()
                .is_some_and(|b| matches!(b, b'a' | b'q'))
    }

    /// 1 to 128 characters of `[A-Za-z0-9._:-]`, not starting with `-` or `.`.
    pub(crate) fn session(text: &str) -> bool {
        (1..=128).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
            && !text.starts_with(['-', '.'])
    }

    /// 1 to 128 bytes of `[A-Za-z0-9._-]`.
    pub(crate) fn attachment_name(text: &str) -> bool {
        (1..=128).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    }

    /// Lowercase hex of one of `lens`.
    pub(crate) fn hex(text: &str, lens: &[usize]) -> bool {
        lens.contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// `"0"` or `[1-9][0-9]*` up to `u64::MAX`.
    pub(crate) fn uint(text: &str) -> Option<u64> {
        let canonical = text == "0"
            || (text.bytes().all(|b| b.is_ascii_digit())
                && !text.starts_with('0')
                && !text.is_empty());
        if canonical { text.parse().ok() } else { None }
    }

    /// `[a-z][a-z0-9_]{0,31}`.
    pub(crate) fn harness_kind(text: &str) -> bool {
        (1..=32).contains(&text.len())
            && text.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }

    /// `/`, or `/` and a relative path: `fabric_core::path::AbsPath`.
    pub(crate) fn abs_path(text: &str) -> bool {
        AbsPath::parse(text).is_ok()
    }

    /// A non-empty path not starting with `/`: `fabric_core::path::RelPath`.
    pub(crate) fn rel_path(text: &str) -> bool {
        RelPath::parse(text).is_ok()
    }
}
