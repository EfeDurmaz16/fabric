//! Bridge protocol v1 on the wire (docs/bridge-protocol.md): the messages
//! each side sends, their parsers and their encoders.
//!
//! Invariant: a message is parsed from a JSON value by a walker that names
//! every field it reads, so an unknown field, a missing one, an integer
//! that is a float or out of range, an id or a string that breaks its rule
//! are each refused with the reason the protocol names, never read past.
//! A parsed message writes back to the same JSON value, which the golden
//! fixtures check for every message. Parsing a message Fabric sends uses
//! the worker's codes (`invalid`, `unsupported`, `hello_refused`,
//! `protocol_error`), so the Rust test double refuses exactly as the
//! Capsule worker must; parsing a message the worker sends refuses with
//! `protocol_violation`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use fabric_client::frame::MAX_SAFE_INTEGER;
use fabric_core::id::JobId;
use fabric_core::managed::{AttemptId, Generation, OperationId, OperationKey, TaskId};
use serde_json::{Map, Value};

/// The bridge protocol major this build speaks.
pub const PROTOCOL: u64 = 1;
/// The cap on every frame Fabric sends, hello included, and on the worker's handshake answer.
pub const MAX_REQUEST_BYTES: u64 = 1 << 20;
/// The cap on every other frame the worker sends.
pub const MAX_FRAME_BYTES: u64 = 16 << 20;
/// The most requests and observes one connection may have outstanding.
pub const MAX_IN_FLIGHT: u64 = 64;
/// The most stderr bytes Fabric keeps of one worker.
pub const MAX_STDERR_BYTES: u64 = 1 << 20;
/// The longest `message` a worker may write.
pub const MAX_MESSAGE_CHARS: usize = 1024;
/// The longest shutdown drain.
pub const MAX_DRAIN_MS: u64 = 30_000;
/// The most `progress` frames one request or observe may get, `seq` 1 to this.
pub const MAX_PROGRESS: u64 = 16;

/// Why a message was refused: the protocol's `code` and `reason`, and a
/// diagnostic that names the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The code: `invalid`, `unsupported`, `hello_refused` or
    /// `protocol_error` for a message to the worker, `protocol_violation`
    /// for one from it.
    pub code: &'static str,
    /// The reason, as the protocol's tables name it.
    pub reason: &'static str,
    /// Which field, for a log line; never compared.
    pub detail: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.code, self.reason, self.detail)
    }
}

impl std::error::Error for Refusal {}

/// A field-level fault before its code is known.
#[derive(Debug)]
struct Bad {
    reason: &'static str,
    detail: String,
}

fn bad(reason: &'static str, detail: impl Into<String>) -> Bad {
    Bad {
        reason,
        detail: detail.into(),
    }
}

impl Bad {
    fn code(self, code: &'static str) -> Refusal {
        Refusal {
            code,
            reason: self.reason,
            detail: self.detail,
        }
    }
}

type Walk<T> = Result<T, Bad>;

/// An object being read: every key it holds must be read, or it is an unknown field.
struct Obj<'a> {
    map: &'a Map<String, Value>,
    read: Vec<&'static str>,
    what: &'static str,
}

impl<'a> Obj<'a> {
    /// A value that is not an object where one is expected reads as the
    /// object missing: the protocol's tables have no other reason for it.
    fn new(value: &'a Value, what: &'static str) -> Walk<Obj<'a>> {
        match value {
            Value::Object(map) => Ok(Obj {
                map,
                read: Vec::new(),
                what,
            }),
            _ => Err(bad("missing_field", format!("{what} is not an object"))),
        }
    }

    fn req(&mut self, key: &'static str) -> Walk<&'a Value> {
        self.read.push(key);
        self.map
            .get(key)
            .ok_or_else(|| bad("missing_field", format!("{}.{key}", self.what)))
    }

    /// An optional field: absent, never `null`.
    fn opt(&mut self, key: &'static str) -> Walk<Option<&'a Value>> {
        self.read.push(key);
        match self.map.get(key) {
            Some(Value::Null) => Err(bad(
                "missing_field",
                format!("{}.{key} is null; an optional field is left out", self.what),
            )),
            other => Ok(other),
        }
    }

    fn finish(self) -> Walk<()> {
        match self.map.keys().find(|k| !self.read.contains(&k.as_str())) {
            Some(key) => Err(bad("unknown_field", format!("{}.{key}", self.what))),
            None => Ok(()),
        }
    }
}

fn int(value: &Value, what: &str) -> Walk<u64> {
    // serde_json reads a fraction or an exponent, `1.0` and `-0` included,
    // as a float, which `as_u64` refuses.
    value
        .as_u64()
        .filter(|n| *n <= MAX_SAFE_INTEGER)
        .ok_or_else(|| {
            bad(
                "bad_integer",
                format!("{what} is not an integer in 0..=2^53-1"),
            )
        })
}

/// An integer that bounds something and so cannot be 0: an exec's
/// `timeout_ms` and `max_output_bytes`. A 0 asks for no time or no
/// output, which no command can meet, so it is refused as `limit` before
/// any effect rather than run with a guessed bound.
fn bound(value: &Value, what: &str) -> Walk<u64> {
    match int(value, what)? {
        0 => Err(bad("limit", format!("{what} is 0"))),
        n => Ok(n),
    }
}

fn boolean(value: &Value, what: &str) -> Walk<bool> {
    value
        .as_bool()
        .ok_or_else(|| bad("missing_field", format!("{what} is not a boolean")))
}

fn string<'a>(value: &'a Value, what: &str) -> Walk<&'a str> {
    value
        .as_str()
        .ok_or_else(|| bad("bad_string", format!("{what} is not a string")))
}

/// A string of 1 to `max` characters with no control character, `\n`
/// and `\t` allowed when `text` is set.
fn text<'a>(value: &'a Value, what: &str, max: usize, multiline: bool) -> Walk<&'a str> {
    let s = string(value, what)?;
    let count = s.chars().count();
    if count == 0 || count > max {
        return Err(bad(
            "bad_string",
            format!("{what} has {count} characters, not 1 to {max}"),
        ));
    }
    if s.chars()
        .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t')))
    {
        return Err(bad(
            "bad_string",
            format!("{what} holds a control character"),
        ));
    }
    Ok(s)
}

fn owned(value: &Value, what: &str, max: usize) -> Walk<String> {
    text(value, what, max, false).map(str::to_owned)
}

fn array<'a>(value: &'a Value, what: &str) -> Walk<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| bad("missing_field", format!("{what} is not an array")))
}

/// A token of 1 to 128 of `[A-Za-z0-9._:-]`: a request id or an approval ref.
fn token(value: &Value, what: &str) -> Walk<String> {
    let s = value
        .as_str()
        .ok_or_else(|| bad("bad_id", format!("{what} is not a string")))?;
    OperationKey::parse(s)
        .map(|_| s.to_owned())
        .map_err(|e| bad("bad_id", format!("{what}: {e}")))
}

fn id<T: FromStr>(value: &Value, what: &str) -> Walk<T> {
    value
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("bad_id", format!("{what} is not a canonical ULID")))
}

/// A string from a closed set; anything else is `unknown_variant`.
fn variant<T: Copy>(value: &Value, what: &str, names: &[(&str, T)]) -> Walk<T> {
    let s = value
        .as_str()
        .ok_or_else(|| bad("unknown_variant", format!("{what} is not a name")))?;
    names
        .iter()
        .find(|(name, _)| *name == s)
        .map(|(_, v)| *v)
        .ok_or_else(|| bad("unknown_variant", format!("{what} {s:?}")))
}

fn string_map(value: &Value, what: &'static str) -> Walk<BTreeMap<String, String>> {
    let Value::Object(map) = value else {
        return Err(bad("missing_field", format!("{what} is not an object")));
    };
    map.iter()
        .map(|(k, v)| Ok((k.clone(), text(v, what, 4096, false)?.to_owned())))
        .collect()
}

fn names(value: &Value, what: &str) -> Walk<Vec<String>> {
    array(value, what)?
        .iter()
        .map(|v| owned(v, what, 256))
        .collect()
}

fn names_value(names: &[String]) -> Value {
    Value::Array(names.iter().cloned().map(Value::String).collect())
}

fn map_value(map: &BTreeMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

fn object(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// An inclusive range of protocol majors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Versions {
    /// The lowest.
    pub min: u64,
    /// The highest.
    pub max: u64,
}

impl Versions {
    /// The range this build speaks: 1 to 1.
    pub const V1: Versions = Versions {
        min: PROTOCOL,
        max: PROTOCOL,
    };

    fn parse(value: &Value) -> Walk<Versions> {
        let mut o = Obj::new(value, "versions")?;
        let min = int(o.req("min")?, "versions.min")?;
        let max = int(o.req("max")?, "versions.max")?;
        o.finish()?;
        Ok(Versions { min, max })
    }

    fn to_value(self) -> Value {
        object(vec![("min", self.min.into()), ("max", self.max.into())])
    }

    /// The highest version both ranges hold.
    #[must_use]
    pub fn common(self, other: Versions) -> Option<u64> {
        let high = self.max.min(other.max);
        (high >= self.min.max(other.min)).then_some(high)
    }
}

/// The transport limits of one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest frame Fabric sends.
    pub max_request_bytes: u64,
    /// The largest frame the worker sends after the handshake.
    pub max_frame_bytes: u64,
    /// The most requests and observes outstanding at once.
    pub max_in_flight: u64,
    /// The most stderr bytes Fabric keeps.
    pub max_stderr_bytes: u64,
}

impl Limits {
    /// The caps of protocol v1, with 8 requests in flight and 64 KiB of stderr.
    pub const DEFAULT: Limits = Limits {
        max_request_bytes: MAX_REQUEST_BYTES,
        max_frame_bytes: MAX_FRAME_BYTES,
        max_in_flight: 8,
        max_stderr_bytes: 64 << 10,
    };

    fn parse(value: &Value) -> Walk<Limits> {
        let mut o = Obj::new(value, "limits")?;
        let limits = Limits {
            max_request_bytes: int(o.req("max_request_bytes")?, "limits.max_request_bytes")?,
            max_frame_bytes: int(o.req("max_frame_bytes")?, "limits.max_frame_bytes")?,
            max_in_flight: int(o.req("max_in_flight")?, "limits.max_in_flight")?,
            max_stderr_bytes: int(o.req("max_stderr_bytes")?, "limits.max_stderr_bytes")?,
        };
        o.finish()?;
        Ok(limits)
    }

    /// Whether every limit is within the protocol's caps.
    #[must_use]
    pub fn within_caps(&self) -> bool {
        (1..=MAX_REQUEST_BYTES).contains(&self.max_request_bytes)
            && (1..=MAX_FRAME_BYTES).contains(&self.max_frame_bytes)
            && (1..=MAX_IN_FLIGHT).contains(&self.max_in_flight)
            && self.max_stderr_bytes <= MAX_STDERR_BYTES
    }

    /// Whether no limit of `self` is above the same limit of `asked`.
    #[must_use]
    pub fn at_most(&self, asked: &Limits) -> bool {
        self.max_request_bytes <= asked.max_request_bytes
            && self.max_frame_bytes <= asked.max_frame_bytes
            && self.max_in_flight <= asked.max_in_flight
            && self.max_stderr_bytes <= asked.max_stderr_bytes
    }

    fn to_value(self) -> Value {
        object(vec![
            ("max_request_bytes", self.max_request_bytes.into()),
            ("max_frame_bytes", self.max_frame_bytes.into()),
            ("max_in_flight", self.max_in_flight.into()),
            ("max_stderr_bytes", self.max_stderr_bytes.into()),
        ])
    }
}

/// The scoped identity of a provider resource: Capsule's `ResourceLocator` on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locator {
    /// The provider, as the profile names it.
    pub provider: String,
    /// The provider scope, exactly the profile's `scope_keys`.
    pub scope: BTreeMap<String, String>,
    /// The resource kind, one of the profile's `resource_kinds`.
    pub kind: String,
    /// The provider's id of the resource.
    pub native_id: String,
    /// The provider's long-running operation id, kept apart from the resource id.
    pub native_operation_id: Option<String>,
}

const MAX_LOCATOR_CHARS: usize = 512;

impl Locator {
    fn parse(value: &Value, what: &'static str) -> Walk<Locator> {
        let mut o = Obj::new(value, what)?;
        let provider = owned(o.req("provider")?, "locator.provider", MAX_LOCATOR_CHARS)?;
        let scope_value = o.req("scope")?;
        let Value::Object(scope_map) = scope_value else {
            return Err(bad(
                "missing_field",
                format!("{what}.scope is not an object"),
            ));
        };
        let mut scope = BTreeMap::new();
        for (key, value) in scope_map {
            let value = text(value, "locator.scope", MAX_LOCATOR_CHARS, false)
                .map_err(|e| bad("bad_locator", e.detail))?;
            scope.insert(key.clone(), value.to_owned());
        }
        let kind = owned(o.req("kind")?, "locator.kind", MAX_LOCATOR_CHARS)?;
        let native_id = locator_id(o.req("native_id")?, "locator.native_id")?;
        let native_operation_id = o
            .opt("native_operation_id")?
            .map(|v| locator_id(v, "locator.native_operation_id"))
            .transpose()?;
        o.finish()?;
        Ok(Locator {
            provider,
            scope,
            kind,
            native_id,
            native_operation_id,
        })
    }

    /// Parses a locator from its JSON value, structure only.
    pub fn from_value(value: &Value) -> Result<Locator, Refusal> {
        Locator::parse(value, "locator").map_err(|e| e.code("invalid"))
    }

    /// The JSON value of the locator.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut fields = vec![
            ("provider", Value::String(self.provider.clone())),
            ("scope", map_value(&self.scope)),
            ("kind", Value::String(self.kind.clone())),
            ("native_id", Value::String(self.native_id.clone())),
        ];
        if let Some(op) = &self.native_operation_id {
            fields.push(("native_operation_id", Value::String(op.clone())));
        }
        object(fields)
    }

    /// Whether the locator has the shape `profile` gives every locator.
    fn fits(&self, profile: &Profile) -> Walk<()> {
        let keys: Vec<&String> = self.scope.keys().collect();
        let mut want: Vec<&String> = profile.scope_keys.iter().collect();
        want.sort();
        if self.provider != profile.provider
            || keys != want
            || !profile.resource_kinds.contains(&self.kind)
        {
            return Err(bad(
                "bad_locator",
                format!(
                    "the locator does not fit profile {}: provider, scope keys or kind",
                    profile.profile
                ),
            ));
        }
        Ok(())
    }
}

/// A native id: 1 to 512 characters, no control character, no `?` or `#`,
/// no URL userinfo.
fn locator_id(value: &Value, what: &str) -> Walk<String> {
    let s =
        text(value, what, MAX_LOCATOR_CHARS, false).map_err(|e| bad("bad_locator", e.detail))?;
    let userinfo = s.find("://").is_some_and(|at| {
        s[at + 3..]
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    });
    if s.contains(['?', '#']) || userinfo {
        return Err(bad(
            "bad_locator",
            format!("{what} holds a query, a fragment or userinfo"),
        ));
    }
    Ok(s.to_owned())
}

/// How a profile's provider tells time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// Each observation advances one interval: deterministic deadlines.
    Virtual,
    /// The wall clock.
    Wall,
}

/// What an action does to a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Makes one.
    Create,
    /// Changes or runs inside one.
    Mutate,
    /// Removes one.
    Destroy,
}

/// Whether an action needs a fresh approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRule {
    /// Policy decides; a supplied approval must be valid.
    Optional,
    /// No valid fresh approval means `denied`.
    Required,
}

/// What holds for a repeated key across worker restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dedupe {
    /// The provider deduplicates on the key within its scope.
    ProviderKey,
    /// The worker finds a resource it made by its operation label.
    LabelQuery,
    /// Nothing; Fabric must not send it again.
    None,
}

/// Whether the worker may pull an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePull {
    /// Never.
    Never,
}

/// One action a profile offers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one capability flag of the profile document, named as on the wire"
)]
pub struct ActionSpec {
    /// Its name, such as `sandbox.create`.
    pub action: String,
    /// What it does to a resource.
    pub effect: Effect,
    /// Whether it answers `result` only after a terminal state was observed.
    pub terminal_only: bool,
    /// Whether `observe` serves it.
    pub observe: bool,
    /// Whether `cancel` stops it.
    pub cancel: bool,
    /// Whether it needs a fresh approval.
    pub approval: ApprovalRule,
    /// What holds for a repeated key across worker restarts.
    pub dedupe: Dedupe,
    /// Whether `secret_names` may be nonempty.
    pub secrets: bool,
}

/// A profile's limits on requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileLimits {
    /// The largest `deadline_ms`.
    pub max_deadline_ms: u64,
    /// The largest `wait_ms` of an observe.
    pub max_observe_wait_ms: u64,
    /// The largest `max_output_bytes` of an exec.
    pub max_output_bytes: u64,
    /// The largest exec `stdin`.
    pub max_stdin_bytes: u64,
    /// The most exec arguments.
    pub max_argv: u64,
    /// The most receipts on a result.
    pub max_receipts: u64,
}

/// A capability profile: what the worker can invoke and verify, sent whole
/// in `hello_ack` and enforced by the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Its name.
    pub profile: String,
    /// Its version.
    pub version: u64,
    /// The provider every locator names.
    pub provider: String,
    /// The scope keys every locator holds.
    pub scope_keys: Vec<String>,
    /// The resource kinds a locator may name.
    pub resource_kinds: Vec<String>,
    /// How the provider tells time.
    pub clock: Clock,
    /// The interval between observations.
    pub observe_interval_ms: u64,
    /// Whether the worker may pull an image; absent when the provider has none.
    pub image_pull: Option<ImagePull>,
    /// The actions it offers.
    pub actions: Vec<ActionSpec>,
    /// Its limits.
    pub limits: ProfileLimits,
}

impl Profile {
    fn parse(value: &Value) -> Walk<Profile> {
        let mut o = Obj::new(value, "profile")?;
        let profile = owned(o.req("profile")?, "profile.profile", 64)?;
        let version = int(o.req("version")?, "profile.version")?;
        let provider = owned(o.req("provider")?, "profile.provider", 64)?;
        let scope_keys = names(o.req("scope_keys")?, "profile.scope_keys")?;
        let resource_kinds = names(o.req("resource_kinds")?, "profile.resource_kinds")?;
        let clock = variant(
            o.req("clock")?,
            "profile.clock",
            &[("virtual", Clock::Virtual), ("wall", Clock::Wall)],
        )?;
        let observe_interval_ms =
            int(o.req("observe_interval_ms")?, "profile.observe_interval_ms")?;
        let image_pull = o
            .opt("image_pull")?
            .map(|v| variant(v, "profile.image_pull", &[("never", ImagePull::Never)]))
            .transpose()?;
        let actions = array(o.req("actions")?, "profile.actions")?
            .iter()
            .map(ActionSpec::parse)
            .collect::<Walk<Vec<_>>>()?;
        let mut l = Obj::new(o.req("limits")?, "profile.limits")?;
        let limits = ProfileLimits {
            max_deadline_ms: int(l.req("max_deadline_ms")?, "max_deadline_ms")?,
            max_observe_wait_ms: int(l.req("max_observe_wait_ms")?, "max_observe_wait_ms")?,
            max_output_bytes: int(l.req("max_output_bytes")?, "max_output_bytes")?,
            max_stdin_bytes: int(l.req("max_stdin_bytes")?, "max_stdin_bytes")?,
            max_argv: int(l.req("max_argv")?, "max_argv")?,
            max_receipts: int(l.req("max_receipts")?, "max_receipts")?,
        };
        l.finish()?;
        o.finish()?;
        if scope_keys.is_empty() || resource_kinds.is_empty() || version == 0 {
            return Err(bad(
                "missing_field",
                "a profile names a version, scope keys and resource kinds",
            ));
        }
        Ok(Profile {
            profile,
            version,
            provider,
            scope_keys,
            resource_kinds,
            clock,
            observe_interval_ms,
            image_pull,
            actions,
            limits,
        })
    }

    /// Parses a capability profile document, such as `profiles/fixture.json`.
    pub fn from_value(value: &Value) -> Result<Profile, Refusal> {
        Profile::parse(value).map_err(|e| e.code("protocol_violation"))
    }

    /// The action of this name the profile offers, if any.
    #[must_use]
    pub fn action(&self, name: &str) -> Option<&ActionSpec> {
        self.actions.iter().find(|a| a.action == name)
    }

    /// The JSON value of the profile.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut fields = vec![
            ("profile", Value::String(self.profile.clone())),
            ("version", self.version.into()),
            ("provider", Value::String(self.provider.clone())),
            ("scope_keys", names_value(&self.scope_keys)),
            ("resource_kinds", names_value(&self.resource_kinds)),
            (
                "clock",
                match self.clock {
                    Clock::Virtual => "virtual",
                    Clock::Wall => "wall",
                }
                .into(),
            ),
            ("observe_interval_ms", self.observe_interval_ms.into()),
        ];
        if let Some(ImagePull::Never) = self.image_pull {
            fields.push(("image_pull", "never".into()));
        }
        fields.push((
            "actions",
            Value::Array(self.actions.iter().map(ActionSpec::to_value).collect()),
        ));
        let l = self.limits;
        fields.push((
            "limits",
            object(vec![
                ("max_deadline_ms", l.max_deadline_ms.into()),
                ("max_observe_wait_ms", l.max_observe_wait_ms.into()),
                ("max_output_bytes", l.max_output_bytes.into()),
                ("max_stdin_bytes", l.max_stdin_bytes.into()),
                ("max_argv", l.max_argv.into()),
                ("max_receipts", l.max_receipts.into()),
            ]),
        ));
        object(fields)
    }
}

impl ActionSpec {
    fn parse(value: &Value) -> Walk<ActionSpec> {
        let mut o = Obj::new(value, "profile.actions[]")?;
        let spec = ActionSpec {
            action: owned(o.req("action")?, "action", 128)?,
            effect: variant(
                o.req("effect")?,
                "effect",
                &[
                    ("create", Effect::Create),
                    ("mutate", Effect::Mutate),
                    ("destroy", Effect::Destroy),
                ],
            )?,
            terminal_only: boolean(o.req("terminal_only")?, "terminal_only")?,
            observe: boolean(o.req("observe")?, "observe")?,
            cancel: boolean(o.req("cancel")?, "cancel")?,
            approval: variant(
                o.req("approval")?,
                "approval",
                &[
                    ("optional", ApprovalRule::Optional),
                    ("required", ApprovalRule::Required),
                ],
            )?,
            dedupe: variant(
                o.req("dedupe")?,
                "dedupe",
                &[
                    ("provider_key", Dedupe::ProviderKey),
                    ("label_query", Dedupe::LabelQuery),
                    ("none", Dedupe::None),
                ],
            )?,
            secrets: boolean(o.req("secrets")?, "secrets")?,
        };
        o.finish()?;
        Ok(spec)
    }

    fn to_value(&self) -> Value {
        object(vec![
            ("action", Value::String(self.action.clone())),
            (
                "effect",
                match self.effect {
                    Effect::Create => "create",
                    Effect::Mutate => "mutate",
                    Effect::Destroy => "destroy",
                }
                .into(),
            ),
            ("terminal_only", self.terminal_only.into()),
            ("observe", self.observe.into()),
            ("cancel", self.cancel.into()),
            (
                "approval",
                match self.approval {
                    ApprovalRule::Optional => "optional",
                    ApprovalRule::Required => "required",
                }
                .into(),
            ),
            (
                "dedupe",
                match self.dedupe {
                    Dedupe::ProviderKey => "provider_key",
                    Dedupe::LabelQuery => "label_query",
                    Dedupe::None => "none",
                }
                .into(),
            ),
            ("secrets", self.secrets.into()),
        ])
    }
}

/// The 32-byte digest Fabric computes for an operation, 64 lowercase hex on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    fn parse(value: &Value, what: &str) -> Walk<Digest> {
        let s = value.as_str().unwrap_or_default();
        let lower_hex = s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if s.len() != 64 || !lower_hex {
            return Err(bad("bad_id", format!("{what} is not 64 lowercase hex")));
        }
        let mut out = [0; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| bad("bad_id", format!("{what} is not hex")))?;
        }
        Ok(Digest(out))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// The journal operation a request belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRef {
    /// The operation id the journal committed.
    pub id: OperationId,
    /// Its idempotency key.
    pub key: OperationKey,
    /// Fabric's digest of the request.
    pub digest: Digest,
}

/// The managed identities an effect belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    /// The job.
    pub job: JobId,
    /// The task.
    pub task: TaskId,
    /// The attempt.
    pub attempt: AttemptId,
    /// The attempt's fencing generation.
    pub generation: Generation,
}

impl Scope {
    fn parse(value: &Value) -> Walk<Scope> {
        let mut o = Obj::new(value, "scope")?;
        let job = id(o.req("job")?, "scope.job")?;
        let task = id(o.req("task")?, "scope.task")?;
        let attempt = id(o.req("attempt")?, "scope.attempt")?;
        let generation = o
            .req("generation")?
            .as_str()
            .and_then(|s| s.parse::<Generation>().ok())
            .ok_or_else(|| {
                bad(
                    "bad_integer",
                    "scope.generation is not a canonical decimal string of at least 1",
                )
            })?;
        o.finish()?;
        Ok(Scope {
            job,
            task,
            attempt,
            generation,
        })
    }

    fn to_value(self) -> Value {
        object(vec![
            ("job", self.job.to_string().into()),
            ("task", self.task.to_string().into()),
            ("attempt", self.attempt.to_string().into()),
            ("generation", self.generation.to_string().into()),
        ])
    }
}

/// The parameters of `sandbox.create`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreateParams {
    /// The image.
    pub image: String,
    /// Plain, non-secret environment values.
    pub env: BTreeMap<String, String>,
    /// The names of the secrets to give it; never values.
    pub secret_names: Vec<String>,
    /// Labels.
    pub labels: BTreeMap<String, String>,
}

/// The parameters of `sandbox.exec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecParams {
    /// The argument vector.
    pub argv: Vec<String>,
    /// Standard input, or none.
    pub stdin: Option<String>,
    /// The working directory, or the sandbox's.
    pub cwd: Option<String>,
    /// The command's own timeout in milliseconds, at least 1.
    pub timeout_ms: u64,
    /// The most output bytes kept per stream, at least 1.
    pub max_output_bytes: u64,
    /// The names of the secrets to give it.
    pub secret_names: Vec<String>,
}

/// The action a request asks for, with its target and parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `sandbox.create`.
    Create(CreateParams),
    /// `sandbox.exec` on a sandbox.
    Exec {
        /// The sandbox.
        target: Locator,
        /// What to run.
        params: ExecParams,
    },
    /// `sandbox.destroy` of a sandbox.
    Destroy {
        /// The sandbox.
        target: Locator,
    },
    /// An action v1 does not define. Fabric refuses it before sending
    /// unless the negotiated profile offers it, and parses it only so a
    /// refusal can name it.
    Other {
        /// Its name.
        name: String,
        /// Its target.
        target: Option<Locator>,
        /// Its parameters.
        params: Map<String, Value>,
    },
}

/// The name of `sandbox.create`.
pub const CREATE: &str = "sandbox.create";
/// The name of `sandbox.exec`.
pub const EXEC: &str = "sandbox.exec";
/// The name of `sandbox.destroy`.
pub const DESTROY: &str = "sandbox.destroy";

impl Action {
    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Action::Create(_) => CREATE,
            Action::Exec { .. } => EXEC,
            Action::Destroy { .. } => DESTROY,
            Action::Other { name, .. } => name,
        }
    }

    /// The resource it acts on, if any.
    #[must_use]
    pub fn target(&self) -> Option<&Locator> {
        match self {
            Action::Create(_) => None,
            Action::Exec { target, .. } | Action::Destroy { target } => Some(target),
            Action::Other { target, .. } => target.as_ref(),
        }
    }

    fn params_value(&self) -> Value {
        match self {
            Action::Create(p) => object(vec![
                ("image", p.image.clone().into()),
                ("env", map_value(&p.env)),
                ("secret_names", names_value(&p.secret_names)),
                ("labels", map_value(&p.labels)),
            ]),
            Action::Exec { params: p, .. } => object(vec![
                ("argv", names_value(&p.argv)),
                ("stdin", p.stdin.clone().map_or(Value::Null, Value::String)),
                ("cwd", p.cwd.clone().map_or(Value::Null, Value::String)),
                ("timeout_ms", p.timeout_ms.into()),
                ("max_output_bytes", p.max_output_bytes.into()),
                ("secret_names", names_value(&p.secret_names)),
            ]),
            Action::Destroy { .. } => Value::Object(Map::new()),
            Action::Other { params, .. } => Value::Object(params.clone()),
        }
    }

    fn parse(name: &str, target: Option<Locator>, params: &Value) -> Walk<Action> {
        let mut o = Obj::new(params, "params")?;
        let action = match name {
            CREATE => {
                if target.is_some() {
                    return Err(bad("unknown_field", "sandbox.create takes no target"));
                }
                Action::Create(CreateParams {
                    image: owned(o.req("image")?, "params.image", 1024)?,
                    env: string_map(o.req("env")?, "params.env")?,
                    secret_names: names(o.req("secret_names")?, "params.secret_names")?,
                    labels: string_map(o.req("labels")?, "params.labels")?,
                })
            }
            EXEC => {
                let target = target.ok_or_else(|| bad("missing_field", "sandbox.exec target"))?;
                let argv = array(o.req("argv")?, "params.argv")?;
                if argv.is_empty() {
                    return Err(bad("limit", "params.argv is empty"));
                }
                let argv = argv
                    .iter()
                    .map(|a| {
                        let s = string(a, "params.argv")?;
                        if s.chars().any(char::is_control) {
                            return Err(bad("bad_string", "params.argv holds a control character"));
                        }
                        Ok(s.to_owned())
                    })
                    .collect::<Walk<Vec<_>>>()?;
                let nullable = |v: &Value, what: &str, multiline: bool| -> Walk<Option<String>> {
                    match v {
                        Value::Null => Ok(None),
                        v => {
                            let s = string(v, what)?;
                            if s.chars()
                                .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t')))
                            {
                                return Err(bad(
                                    "bad_string",
                                    format!("{what} holds a control character"),
                                ));
                            }
                            Ok(Some(s.to_owned()))
                        }
                    }
                };
                Action::Exec {
                    target,
                    params: ExecParams {
                        argv,
                        stdin: nullable(o.req("stdin")?, "params.stdin", true)?,
                        cwd: nullable(o.req("cwd")?, "params.cwd", false)?,
                        timeout_ms: bound(o.req("timeout_ms")?, "params.timeout_ms")?,
                        max_output_bytes: bound(
                            o.req("max_output_bytes")?,
                            "params.max_output_bytes",
                        )?,
                        secret_names: names(o.req("secret_names")?, "params.secret_names")?,
                    },
                }
            }
            DESTROY => Action::Destroy {
                target: target.ok_or_else(|| bad("missing_field", "sandbox.destroy target"))?,
            },
            other => {
                let Value::Object(params) = params else {
                    return Err(bad("missing_field", "params is not an object"));
                };
                return Ok(Action::Other {
                    name: other.to_owned(),
                    target,
                    params: params.clone(),
                });
            }
        };
        o.finish()?;
        Ok(action)
    }
}

/// Fabric's narrowing of the worker's secrets policy for one operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RequestPolicy {
    /// The secrets block: `Some` with `allowed` left out denies every secret.
    pub secrets: Option<SecretsPolicy>,
}

/// A secrets block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecretsPolicy {
    /// The allowlist; left out, the block denies every secret.
    pub allowed: Option<Vec<String>>,
}

/// One effect Fabric asks the worker for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The transport correlation, unique per connection.
    pub request_id: String,
    /// The journal operation.
    pub operation: OperationRef,
    /// The managed identities it belongs to.
    pub scope: Scope,
    /// The action, its target and its parameters.
    pub action: Action,
    /// The fresh approval slot: a ref the worker resolves, never a credential.
    pub approval: Option<String>,
    /// Fabric's narrowing of policy.
    pub policy: Option<RequestPolicy>,
    /// Milliseconds from when the worker reads the frame.
    pub deadline_ms: u64,
}

impl Request {
    fn parse(o: &mut Obj<'_>) -> Walk<Request> {
        let request_id = token(o.req("request_id")?, "request_id")?;
        let mut op = Obj::new(o.req("operation")?, "operation")?;
        let operation = OperationRef {
            id: id(op.req("id")?, "operation.id")?,
            key: op
                .req("key")?
                .as_str()
                .and_then(|s| OperationKey::parse(s).ok())
                .ok_or_else(|| bad("bad_id", "operation.key"))?,
            digest: Digest::parse(op.req("digest")?, "operation.digest")?,
        };
        op.finish()?;
        let scope = Scope::parse(o.req("scope")?)?;
        let action = text(o.req("action")?, "action", 128, false)?.to_owned();
        let target = o
            .opt("target")?
            .map(|v| Locator::parse(v, "target"))
            .transpose()?;
        let params = o.req("params")?;
        let approval = match o.req("approval")? {
            Value::Null => None,
            v => {
                let mut a = Obj::new(v, "approval")?;
                let reference = token(a.req("ref")?, "approval.ref")?;
                a.finish()?;
                Some(reference)
            }
        };
        let policy = o.opt("policy")?.map(parse_policy).transpose()?;
        let deadline_ms = int(o.req("deadline_ms")?, "deadline_ms")?;
        if deadline_ms == 0 {
            return Err(bad("limit", "deadline_ms is 0"));
        }
        let action = Action::parse(&action, target, params)?;
        Ok(Request {
            request_id,
            operation,
            scope,
            action,
            approval,
            policy,
            deadline_ms,
        })
    }

    /// The JSON value of the request.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut fields = vec![
            ("type", "request".into()),
            ("request_id", self.request_id.clone().into()),
            (
                "operation",
                object(vec![
                    ("id", self.operation.id.to_string().into()),
                    ("key", self.operation.key.as_str().into()),
                    ("digest", self.operation.digest.to_string().into()),
                ]),
            ),
            ("scope", self.scope.to_value()),
            ("action", self.action.name().into()),
        ];
        if let Some(target) = self.action.target() {
            fields.push(("target", target.to_value()));
        }
        fields.push(("params", self.action.params_value()));
        fields.push((
            "approval",
            self.approval
                .as_ref()
                .map_or(Value::Null, |r| object(vec![("ref", r.clone().into())])),
        ));
        if let Some(policy) = &self.policy {
            fields.push(("policy", policy_value(policy)));
        }
        fields.push(("deadline_ms", self.deadline_ms.into()));
        object(fields)
    }

    /// The content the operation digest covers: everything but the
    /// transport correlation and the operation itself.
    #[must_use]
    pub fn digest_content(&self) -> Value {
        let mut value = self.to_value();
        if let Value::Object(map) = &mut value {
            map.remove("type");
            map.remove("request_id");
            map.remove("operation");
            map.remove("deadline_ms");
        }
        value
    }
}

fn parse_policy(value: &Value) -> Walk<RequestPolicy> {
    let mut p = Obj::new(value, "policy")?;
    let secrets = p
        .opt("secrets")?
        .map(|v| {
            let mut s = Obj::new(v, "policy.secrets")?;
            let allowed = s
                .opt("allowed")?
                .map(|a| names(a, "policy.secrets.allowed"))
                .transpose()?;
            s.finish()?;
            Ok(SecretsPolicy { allowed })
        })
        .transpose()?;
    p.finish()?;
    Ok(RequestPolicy { secrets })
}

fn policy_value(policy: &RequestPolicy) -> Value {
    let mut fields = Vec::new();
    if let Some(secrets) = &policy.secrets {
        let mut inner = Vec::new();
        if let Some(allowed) = &secrets.allowed {
            inner.push(("allowed", names_value(allowed)));
        }
        fields.push(("secrets", object(inner)));
    }
    object(fields)
}

/// A read of an earlier operation's resource, waiting at most `wait_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observe {
    /// The transport correlation.
    pub request_id: String,
    /// The original operation.
    pub operation_id: OperationId,
    /// The original operation's managed identities, under the request's
    /// rules: the worker refuses a resource another job made.
    pub scope: Scope,
    /// The original action.
    pub action: String,
    /// The resource.
    pub resource: Locator,
    /// The longest wait.
    pub wait_ms: u64,
}

/// A request to stop waiting on an earlier request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancel {
    /// The transport correlation of the cancel itself.
    pub request_id: String,
    /// The request it cancels.
    pub cancels: String,
    /// That request's operation.
    pub operation_id: OperationId,
}

/// The handshake Fabric opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The protocol majors Fabric speaks.
    pub versions: Versions,
    /// Fabric's name and version.
    pub fabric: String,
    /// The profile Fabric wants.
    pub profile: String,
    /// Fabric's limits.
    pub limits: Limits,
}

/// A message Fabric sends the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToWorker {
    /// The handshake.
    Hello(Hello),
    /// An effect.
    Request(Box<Request>),
    /// A read.
    Observe(Observe),
    /// A cancel.
    Cancel(Cancel),
    /// A drain and exit.
    Shutdown {
        /// The transport correlation.
        request_id: String,
        /// The longest drain.
        drain_ms: u64,
    },
}

impl ToWorker {
    /// The JSON value of the message.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            ToWorker::Hello(h) => object(vec![
                ("type", "hello".into()),
                ("versions", h.versions.to_value()),
                ("fabric", h.fabric.clone().into()),
                ("profile", h.profile.clone().into()),
                ("limits", h.limits.to_value()),
            ]),
            ToWorker::Request(r) => r.to_value(),
            ToWorker::Observe(o) => object(vec![
                ("type", "observe".into()),
                ("request_id", o.request_id.clone().into()),
                ("operation_id", o.operation_id.to_string().into()),
                ("scope", o.scope.to_value()),
                ("action", o.action.clone().into()),
                ("resource", o.resource.to_value()),
                ("wait_ms", o.wait_ms.into()),
            ]),
            ToWorker::Cancel(c) => object(vec![
                ("type", "cancel".into()),
                ("request_id", c.request_id.clone().into()),
                ("cancels", c.cancels.clone().into()),
                ("operation_id", c.operation_id.to_string().into()),
            ]),
            ToWorker::Shutdown {
                request_id,
                drain_ms,
            } => object(vec![
                ("type", "shutdown".into()),
                ("request_id", request_id.clone().into()),
                ("drain_ms", (*drain_ms).into()),
            ]),
        }
    }

    /// Parses a message to the worker by its structure alone, as Fabric
    /// reads a fixture: any action name parses. Refusals carry the codes
    /// the worker answers with.
    pub fn from_value(value: &Value) -> Result<ToWorker, Refusal> {
        let mut o = Obj::new(value, "message").map_err(|e| e.code("protocol_error"))?;
        let kind =
            o.req("type").ok().and_then(Value::as_str).ok_or_else(|| {
                bad("unexpected", "a message without a type").code("protocol_error")
            })?;
        let message = match kind {
            "hello" => parse_hello(&mut o).map_err(hello_invalid)?,
            "request" => ToWorker::Request(Box::new(
                Request::parse(&mut o).map_err(|e| e.code("invalid"))?,
            )),
            "observe" => parse_observe(&mut o).map_err(|e| e.code("invalid"))?,
            "cancel" => {
                let c = Cancel {
                    request_id: token(
                        o.req("request_id").map_err(|e| e.code("invalid"))?,
                        "request_id",
                    )
                    .map_err(|e| e.code("invalid"))?,
                    cancels: token(o.req("cancels").map_err(|e| e.code("invalid"))?, "cancels")
                        .map_err(|e| e.code("invalid"))?,
                    operation_id: id(
                        o.req("operation_id").map_err(|e| e.code("invalid"))?,
                        "operation_id",
                    )
                    .map_err(|e| e.code("invalid"))?,
                };
                ToWorker::Cancel(c)
            }
            "shutdown" => {
                let request_id = token(
                    o.req("request_id").map_err(|e| e.code("invalid"))?,
                    "request_id",
                )
                .map_err(|e| e.code("invalid"))?;
                let drain_ms = int(
                    o.req("drain_ms").map_err(|e| e.code("invalid"))?,
                    "drain_ms",
                )
                .map_err(|e| e.code("invalid"))?;
                if !(1..=MAX_DRAIN_MS).contains(&drain_ms) {
                    return Err(bad("limit", "drain_ms is not 1 to 30000").code("invalid"));
                }
                ToWorker::Shutdown {
                    request_id,
                    drain_ms,
                }
            }
            other => {
                return Err(bad("unexpected", format!("a message of type {other:?}"))
                    .code("protocol_error"));
            }
        };
        if kind == "hello" {
            o.finish().map_err(hello_invalid)?;
        } else {
            o.finish().map_err(|e| e.code("invalid"))?;
        }
        if let ToWorker::Hello(h) = &message
            && (h.versions.min == 0 || h.versions.min > h.versions.max || !h.limits.within_caps())
        {
            return Err(
                bad("invalid", "versions out of order or limits above the caps")
                    .code("hello_refused"),
            );
        }
        Ok(message)
    }

    /// Parses a message as the worker must on reading it in `phase`:
    /// before the handshake only a hello it can serve, after it only a
    /// session message that fits the negotiated `profile`.
    pub fn parse_as_worker(
        value: &Value,
        phase: Phase,
        worker: Versions,
        profiles: &[Profile],
        profile: &Profile,
    ) -> Result<ToWorker, Refusal> {
        let message = ToWorker::from_value(value)?;
        match (phase, &message) {
            (Phase::Handshake, ToWorker::Hello(h)) => {
                if h.versions.common(worker).is_none() {
                    return Err(
                        bad("unsupported_version", "no common version").code("hello_refused")
                    );
                }
                if !profiles.iter().any(|p| p.profile == h.profile) {
                    return Err(bad("unsupported_profile", h.profile.clone()).code("hello_refused"));
                }
            }
            (Phase::Handshake, _) | (Phase::Session, ToWorker::Hello(_)) => {
                return Err(bad("unexpected", "a message out of its phase").code("protocol_error"));
            }
            (Phase::Session, ToWorker::Request(r)) => fits_request(r, profile)?,
            (Phase::Session, ToWorker::Observe(o)) => fits_observe(o, profile)?,
            (Phase::Session, ToWorker::Cancel(_) | ToWorker::Shutdown { .. }) => {}
        }
        Ok(message)
    }
}

/// A hello that does not parse is refused as `invalid`, the one
/// `hello_refused` reason for it.
fn hello_invalid(e: Bad) -> Refusal {
    let Bad { reason, detail } = e;
    Refusal {
        code: "hello_refused",
        reason: "invalid",
        detail: format!("{reason}: {detail}"),
    }
}

/// Where a connection is: before the handshake or after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before `hello_ack`.
    Handshake,
    /// After it.
    Session,
}

fn parse_hello(o: &mut Obj<'_>) -> Walk<ToWorker> {
    Ok(ToWorker::Hello(Hello {
        versions: Versions::parse(o.req("versions")?)?,
        fabric: owned(o.req("fabric")?, "fabric", 64)?,
        profile: owned(o.req("profile")?, "profile", 64)?,
        limits: Limits::parse(o.req("limits")?)?,
    }))
}

fn parse_observe(o: &mut Obj<'_>) -> Walk<ToWorker> {
    let observe = Observe {
        request_id: token(o.req("request_id")?, "request_id")?,
        operation_id: id(o.req("operation_id")?, "operation_id")?,
        scope: Scope::parse(o.req("scope")?)?,
        action: owned(o.req("action")?, "action", 128)?,
        resource: Locator::parse(o.req("resource")?, "resource")?,
        wait_ms: int(o.req("wait_ms")?, "wait_ms")?,
    };
    if observe.wait_ms == 0 {
        return Err(bad("limit", "wait_ms is 0"));
    }
    Ok(ToWorker::Observe(observe))
}

/// Whether `request` fits `profile`: an action it offers, a target of
/// its shape, and limits within its own. Refusals carry the worker's codes.
pub fn fits_request(request: &Request, profile: &Profile) -> Result<(), Refusal> {
    let Some(spec) = profile.action(request.action.name()) else {
        return Err(bad("action", request.action.name().to_owned()).code("unsupported"));
    };
    if matches!(request.action, Action::Other { .. }) {
        return Err(bad("action", spec.action.clone()).code("unsupported"));
    }
    let invalid = |e: Bad| e.code("invalid");
    if let Some(target) = request.action.target() {
        target.fits(profile).map_err(invalid)?;
    }
    let l = &profile.limits;
    if request.deadline_ms > l.max_deadline_ms {
        return Err(bad("limit", "deadline_ms above the profile's").code("invalid"));
    }
    if let Action::Exec { params, .. } = &request.action {
        let argv = u64::try_from(params.argv.len()).unwrap_or(u64::MAX);
        let stdin = params
            .stdin
            .as_ref()
            .map_or(0, |s| u64::try_from(s.len()).unwrap_or(u64::MAX));
        if argv > l.max_argv
            || params.max_output_bytes > l.max_output_bytes
            || stdin > l.max_stdin_bytes
        {
            return Err(bad("limit", "exec params above the profile's limits").code("invalid"));
        }
    }
    Ok(())
}

/// Whether `observe` fits `profile`: an action it offers with observe,
/// a resource of its shape and a wait within its own.
pub fn fits_observe(observe: &Observe, profile: &Profile) -> Result<(), Refusal> {
    observable(&observe.action, &observe.resource, observe.wait_ms, profile)
}

/// Whether `profile` observes `action` on `resource` within `wait_ms`:
/// the profile's part of [`fits_observe`], which does not depend on the
/// observing operation's scope.
pub fn observable(
    action: &str,
    resource: &Locator,
    wait_ms: u64,
    profile: &Profile,
) -> Result<(), Refusal> {
    let Some(spec) = profile.action(action) else {
        return Err(bad("action", action.to_owned()).code("unsupported"));
    };
    if !spec.observe {
        return Err(bad("observe", action.to_owned()).code("unsupported"));
    }
    resource.fits(profile).map_err(|e| e.code("invalid"))?;
    if wait_ms > profile.limits.max_observe_wait_ms {
        return Err(bad("limit", "wait_ms above the profile's").code("invalid"));
    }
    Ok(())
}

/// The state a `result` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultState {
    /// A terminal state was observed.
    Completed,
    /// A terminal cancellation was observed.
    Cancelled,
    /// Accepted, not terminal: only for an action that is not terminal-only.
    Accepted,
    /// Running, not terminal: only for an action that is not terminal-only.
    Running,
}

const RESULT_STATES: &[(&str, ResultState)] = &[
    ("completed", ResultState::Completed),
    ("cancelled", ResultState::Cancelled),
    ("accepted", ResultState::Accepted),
    ("running", ResultState::Running),
];

/// One receipt the worker wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Its digest.
    pub digest: Digest,
    /// Where it is.
    pub reference: String,
}

/// A `result`: the action's outcome as the worker observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultMessage {
    /// The request it answers.
    pub request_id: String,
    /// Its operation.
    pub operation_id: OperationId,
    /// What was observed.
    pub state: ResultState,
    /// The full locator.
    pub resource: Locator,
    /// The action's output.
    pub output: Map<String, Value>,
    /// Whether this answers a repeat with no new provider call.
    pub replayed: bool,
    /// Receipts.
    pub receipts: Vec<Receipt>,
    /// A receipt write that failed after the action completed.
    pub evidence_error: Option<String>,
}

/// An error code and the effects it allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The request is malformed or conflicts with an earlier one.
    Invalid,
    /// Not in the negotiated profile.
    Unsupported,
    /// Policy, approval, secret or scope refusal.
    Denied,
    /// Busy or shutting down.
    Unavailable,
    /// A definite provider failure.
    ProviderFailed,
    /// Accepted and not terminal within the bound.
    Pending,
    /// Nobody can tell.
    Unknown,
}

impl ErrorCode {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::Invalid => "invalid",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::Denied => "denied",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::ProviderFailed => "provider_failed",
            ErrorCode::Pending => "pending",
            ErrorCode::Unknown => "unknown",
        }
    }

    /// Whether it refuses before any effect.
    #[must_use]
    pub fn is_refusal(self) -> bool {
        matches!(
            self,
            ErrorCode::Invalid
                | ErrorCode::Unsupported
                | ErrorCode::Denied
                | ErrorCode::Unavailable
        )
    }

    fn reasons(self) -> &'static [&'static str] {
        match self {
            ErrorCode::Invalid => &[
                "unknown_field",
                "missing_field",
                "bad_integer",
                "bad_id",
                "bad_string",
                "bad_locator",
                "limit",
                "duplicate_request_id",
                "idempotency_conflict",
            ],
            ErrorCode::Unsupported => &["action", "observe", "cancel"],
            ErrorCode::Denied => &[
                "policy_denied",
                "approval_required",
                "approval_expired",
                "approval_scope",
                "secret_not_allowed",
                "scope_mismatch",
                "owner_mismatch",
            ],
            ErrorCode::Unavailable => &["busy", "shutting_down"],
            ErrorCode::ProviderFailed => &["provider_refused", "not_found", "quota"],
            ErrorCode::Pending => &["observation_deadline", "shutting_down"],
            ErrorCode::Unknown => &["reply_lost", "observation_failed"],
        }
    }
}

const ERROR_CODES: &[(&str, ErrorCode)] = &[
    ("invalid", ErrorCode::Invalid),
    ("unsupported", ErrorCode::Unsupported),
    ("denied", ErrorCode::Denied),
    ("unavailable", ErrorCode::Unavailable),
    ("provider_failed", ErrorCode::ProviderFailed),
    ("pending", ErrorCode::Pending),
    ("unknown", ErrorCode::Unknown),
];

/// The effect an error declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectState {
    /// No effect happened.
    None,
    /// The effect happened.
    Applied,
    /// The effect is under way.
    Pending,
    /// Nobody can tell.
    Unknown,
}

impl EffectState {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            EffectState::None => "none",
            EffectState::Applied => "applied",
            EffectState::Pending => "pending",
            EffectState::Unknown => "unknown",
        }
    }
}

const EFFECTS: &[(&str, EffectState)] = &[
    ("none", EffectState::None),
    ("applied", EffectState::Applied),
    ("pending", EffectState::Pending),
    ("unknown", EffectState::Unknown),
];

/// The `error` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorBody {
    /// The code.
    pub code: ErrorCode,
    /// The reason, one the code allows.
    pub reason: String,
    /// A safe diagnostic; never compared.
    pub message: String,
    /// The effect, one the code allows.
    pub effect: EffectState,
    /// The locator; required for `pending` and for `provider_failed` with `applied`.
    pub resource: Option<Locator>,
    /// The last observed native state.
    pub state: Option<String>,
    /// How many observations were made.
    pub observations: Option<u64>,
    /// The bound that passed.
    pub deadline_ms: Option<u64>,
    /// What cleanup did after an applied failure, as the worker wrote it.
    pub cleanup: Option<Value>,
}

/// An `error`: the request did not complete, with the effect it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    /// The request it answers.
    pub request_id: String,
    /// Its operation.
    pub operation_id: OperationId,
    /// The error.
    pub error: ErrorBody,
}

/// How a `cancel` was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelState {
    /// The worker is stopping its wait.
    Requested,
    /// No such request is in flight.
    NotFound,
    /// The request was already answered.
    AlreadyAnswered,
    /// The action has `cancel: false`.
    Unsupported,
}

impl CancelState {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            CancelState::Requested => "requested",
            CancelState::NotFound => "not_found",
            CancelState::AlreadyAnswered => "already_answered",
            CancelState::Unsupported => "unsupported",
        }
    }
}

const CANCEL_STATES: &[(&str, CancelState)] = &[
    ("requested", CancelState::Requested),
    ("not_found", CancelState::NotFound),
    ("already_answered", CancelState::AlreadyAnswered),
    ("unsupported", CancelState::Unsupported),
];

/// One request left open at shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The request.
    pub request_id: String,
    /// Its operation.
    pub operation_id: OperationId,
    /// `pending` or `unknown`.
    pub state: EffectState,
    /// The locator.
    pub resource: Locator,
}

/// A message the worker sends Fabric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToFabric {
    /// The handshake answer.
    HelloAck {
        /// The version picked.
        protocol: u64,
        /// The worker's name and version.
        worker: String,
        /// The profile it enforces.
        profile: Profile,
        /// Its limits, none above Fabric's.
        limits: Limits,
    },
    /// The handshake refusal, whose shape never changes.
    HelloRefused {
        /// The worker's own range.
        versions: Versions,
        /// `unsupported_version`, `unsupported_profile` or `invalid`.
        code: String,
        /// A diagnostic.
        message: String,
    },
    /// A result.
    Result(ResultMessage),
    /// An error.
    Error(ErrorMessage),
    /// Progress of a request.
    Progress {
        /// The request.
        request_id: String,
        /// Its operation.
        operation_id: OperationId,
        /// Its sequence number, from 1.
        seq: u64,
        /// The native state.
        state: String,
        /// The locator as soon as it is known.
        resource: Locator,
    },
    /// A cancel was received.
    CancelAck {
        /// The cancel.
        request_id: String,
        /// The request it cancels.
        cancels: String,
        /// How it was taken.
        state: CancelState,
    },
    /// The shutdown drained.
    ShutdownAck {
        /// The shutdown.
        request_id: String,
        /// What was still open.
        unresolved: Vec<Unresolved>,
    },
    /// The worker's last frame.
    ProtocolError {
        /// `oversize`, `truncated`, `empty`, `malformed` or `unexpected`.
        code: String,
        /// A diagnostic.
        message: String,
    },
}

/// What Fabric knows when it reads a worker's frame.
#[derive(Debug, Clone, Copy)]
pub enum Expect<'a> {
    /// The handshake answer to this hello.
    Handshake(&'a Hello),
    /// A session message under this profile.
    Session(&'a Profile),
}

impl ToFabric {
    /// Parses a message from the worker. A message that breaks the
    /// protocol is refused with `protocol_violation`.
    pub fn parse(value: &Value, expect: Expect<'_>) -> Result<ToFabric, Refusal> {
        parse_to_fabric(value, expect).map_err(|e| e.code("protocol_violation"))
    }

    /// The JSON value of the message.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per message, each the message's field table"
    )]
    pub fn to_value(&self) -> Value {
        match self {
            ToFabric::HelloAck {
                protocol,
                worker,
                profile,
                limits,
            } => object(vec![
                ("type", "hello_ack".into()),
                ("protocol", (*protocol).into()),
                ("worker", worker.clone().into()),
                ("profile", profile.to_value()),
                ("limits", limits.to_value()),
            ]),
            ToFabric::HelloRefused {
                versions,
                code,
                message,
            } => object(vec![
                ("type", "hello_refused".into()),
                ("versions", versions.to_value()),
                (
                    "reason",
                    object(vec![
                        ("code", code.clone().into()),
                        ("message", message.clone().into()),
                    ]),
                ),
            ]),
            ToFabric::Result(r) => {
                let mut fields = vec![
                    ("type", "result".into()),
                    ("request_id", r.request_id.clone().into()),
                    ("operation_id", r.operation_id.to_string().into()),
                    (
                        "state",
                        RESULT_STATES
                            .iter()
                            .find(|(_, s)| *s == r.state)
                            .map_or("completed", |(n, _)| n)
                            .into(),
                    ),
                    ("resource", r.resource.to_value()),
                    ("output", Value::Object(r.output.clone())),
                    ("replayed", r.replayed.into()),
                    (
                        "receipts",
                        Value::Array(
                            r.receipts
                                .iter()
                                .map(|x| {
                                    object(vec![
                                        ("algorithm", "sha256".into()),
                                        ("digest", x.digest.to_string().into()),
                                        ("ref", x.reference.clone().into()),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ];
                if let Some(message) = &r.evidence_error {
                    fields.push((
                        "evidence_error",
                        object(vec![
                            ("reason", "receipt_write_failed".into()),
                            ("message", message.clone().into()),
                        ]),
                    ));
                }
                object(fields)
            }
            ToFabric::Error(e) => {
                let b = &e.error;
                let mut body = vec![
                    ("code", b.code.name().into()),
                    ("reason", b.reason.clone().into()),
                    ("message", b.message.clone().into()),
                    ("effect", b.effect.name().into()),
                ];
                if let Some(r) = &b.resource {
                    body.push(("resource", r.to_value()));
                }
                if let Some(s) = &b.state {
                    body.push(("state", s.clone().into()));
                }
                if let Some(n) = b.observations {
                    body.push(("observations", n.into()));
                }
                if let Some(n) = b.deadline_ms {
                    body.push(("deadline_ms", n.into()));
                }
                if let Some(c) = &b.cleanup {
                    body.push(("cleanup", c.clone()));
                }
                object(vec![
                    ("type", "error".into()),
                    ("request_id", e.request_id.clone().into()),
                    ("operation_id", e.operation_id.to_string().into()),
                    ("error", object(body)),
                ])
            }
            ToFabric::Progress {
                request_id,
                operation_id,
                seq,
                state,
                resource,
            } => object(vec![
                ("type", "progress".into()),
                ("request_id", request_id.clone().into()),
                ("operation_id", operation_id.to_string().into()),
                ("seq", (*seq).into()),
                ("state", state.clone().into()),
                ("resource", resource.to_value()),
            ]),
            ToFabric::CancelAck {
                request_id,
                cancels,
                state,
            } => object(vec![
                ("type", "cancel_ack".into()),
                ("request_id", request_id.clone().into()),
                ("cancels", cancels.clone().into()),
                ("state", state.name().into()),
            ]),
            ToFabric::ShutdownAck {
                request_id,
                unresolved,
            } => object(vec![
                ("type", "shutdown_ack".into()),
                ("request_id", request_id.clone().into()),
                (
                    "unresolved",
                    Value::Array(
                        unresolved
                            .iter()
                            .map(|u| {
                                object(vec![
                                    ("request_id", u.request_id.clone().into()),
                                    ("operation_id", u.operation_id.to_string().into()),
                                    ("state", u.state.name().into()),
                                    ("resource", u.resource.to_value()),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
            ToFabric::ProtocolError { code, message } => object(vec![
                ("type", "protocol_error".into()),
                ("code", code.clone().into()),
                ("message", message.clone().into()),
            ]),
        }
    }
}

fn message_text(value: &Value, what: &str) -> Walk<String> {
    let s = string(value, what)?;
    if s.chars().count() > MAX_MESSAGE_CHARS {
        return Err(bad(
            "limit",
            format!("{what} is over {MAX_MESSAGE_CHARS} characters"),
        ));
    }
    Ok(s.to_owned())
}

fn parse_to_fabric(value: &Value, expect: Expect<'_>) -> Walk<ToFabric> {
    let mut o = Obj::new(value, "message")?;
    let kind = string(o.req("type")?, "type")?;
    let message = match (expect, kind) {
        (Expect::Handshake(hello), "hello_ack") => parse_ack(&mut o, hello)?,
        (Expect::Handshake(_), "hello_refused") => {
            let versions = Versions::parse(o.req("versions")?)?;
            let mut r = Obj::new(o.req("reason")?, "reason")?;
            let code = variant(
                r.req("code")?,
                "reason.code",
                &[
                    ("unsupported_version", "unsupported_version"),
                    ("unsupported_profile", "unsupported_profile"),
                    ("invalid", "invalid"),
                ],
            )?
            .to_owned();
            let message = message_text(r.req("message")?, "reason.message")?;
            r.finish()?;
            ToFabric::HelloRefused {
                versions,
                code,
                message,
            }
        }
        (Expect::Session(profile), "result") => parse_result(&mut o, profile)?,
        (Expect::Session(profile), "error") => parse_error(&mut o, profile)?,
        (Expect::Session(profile), "progress") => {
            let resource = Locator::parse(o.req("resource")?, "resource")?;
            resource.fits(profile)?;
            let seq = int(o.req("seq")?, "seq")?;
            if !(1..=MAX_PROGRESS).contains(&seq) {
                return Err(bad("limit", "progress seq is not 1 to 16"));
            }
            ToFabric::Progress {
                request_id: token(o.req("request_id")?, "request_id")?,
                operation_id: id(o.req("operation_id")?, "operation_id")?,
                seq,
                state: owned(o.req("state")?, "state", 128)?,
                resource,
            }
        }
        (Expect::Session(_), "cancel_ack") => ToFabric::CancelAck {
            request_id: token(o.req("request_id")?, "request_id")?,
            cancels: token(o.req("cancels")?, "cancels")?,
            state: variant(o.req("state")?, "state", CANCEL_STATES)?,
        },
        (Expect::Session(profile), "shutdown_ack") => {
            let request_id = token(o.req("request_id")?, "request_id")?;
            let unresolved = array(o.req("unresolved")?, "unresolved")?
                .iter()
                .map(|v| {
                    let mut u = Obj::new(v, "unresolved[]")?;
                    let item = Unresolved {
                        request_id: token(u.req("request_id")?, "request_id")?,
                        operation_id: id(u.req("operation_id")?, "operation_id")?,
                        state: variant(
                            u.req("state")?,
                            "state",
                            &[
                                ("pending", EffectState::Pending),
                                ("unknown", EffectState::Unknown),
                            ],
                        )?,
                        resource: Locator::parse(u.req("resource")?, "resource")?,
                    };
                    item.resource.fits(profile)?;
                    u.finish()?;
                    Ok(item)
                })
                .collect::<Walk<Vec<_>>>()?;
            ToFabric::ShutdownAck {
                request_id,
                unresolved,
            }
        }
        (Expect::Session(_), "protocol_error") => ToFabric::ProtocolError {
            code: variant(
                o.req("code")?,
                "code",
                &[
                    ("oversize", "oversize"),
                    ("truncated", "truncated"),
                    ("empty", "empty"),
                    ("malformed", "malformed"),
                    ("unexpected", "unexpected"),
                ],
            )?
            .to_owned(),
            message: message_text(o.req("message")?, "message")?,
        },
        (_, other) => return Err(bad("unexpected", format!("a {other} message here"))),
    };
    o.finish()?;
    Ok(message)
}

fn parse_ack(o: &mut Obj<'_>, hello: &Hello) -> Walk<ToFabric> {
    let protocol = int(o.req("protocol")?, "protocol")?;
    let worker = owned(o.req("worker")?, "worker", 64)?;
    let profile = Profile::parse(o.req("profile")?)?;
    let limits = Limits::parse(o.req("limits")?)?;
    if protocol < hello.versions.min || protocol > hello.versions.max {
        return Err(bad("unsupported_version", format!("protocol {protocol}")));
    }
    if profile.profile != hello.profile {
        return Err(bad(
            "profile_mismatch",
            format!("asked for {}, got {}", hello.profile, profile.profile),
        ));
    }
    if !limits.at_most(&hello.limits) || !limits.within_caps() {
        return Err(bad("limit_raised", "the ack raises a limit"));
    }
    Ok(ToFabric::HelloAck {
        protocol,
        worker,
        profile,
        limits,
    })
}

fn parse_result(o: &mut Obj<'_>, profile: &Profile) -> Walk<ToFabric> {
    let request_id = token(o.req("request_id")?, "request_id")?;
    let operation_id = id(o.req("operation_id")?, "operation_id")?;
    let state = variant(o.req("state")?, "state", RESULT_STATES)?;
    let resource = Locator::parse(o.req("resource")?, "resource")?;
    resource.fits(profile)?;
    let Value::Object(output) = o.req("output")? else {
        return Err(bad("missing_field", "output is not an object"));
    };
    let replayed = boolean(o.req("replayed")?, "replayed")?;
    let receipts = array(o.req("receipts")?, "receipts")?;
    if u64::try_from(receipts.len()).unwrap_or(u64::MAX) > profile.limits.max_receipts {
        return Err(bad("limit", "more receipts than the profile allows"));
    }
    let receipts = receipts
        .iter()
        .map(|v| {
            let mut r = Obj::new(v, "receipts[]")?;
            variant(r.req("algorithm")?, "algorithm", &[("sha256", ())])?;
            let receipt = Receipt {
                digest: Digest::parse(r.req("digest")?, "receipt digest")?,
                reference: owned(r.req("ref")?, "receipt ref", 1024)?,
            };
            r.finish()?;
            Ok(receipt)
        })
        .collect::<Walk<Vec<_>>>()?;
    let evidence_error = o
        .opt("evidence_error")?
        .map(|v| {
            let mut e = Obj::new(v, "evidence_error")?;
            variant(
                e.req("reason")?,
                "evidence_error.reason",
                &[("receipt_write_failed", ())],
            )?;
            let message = message_text(e.req("message")?, "evidence_error.message")?;
            e.finish()?;
            Ok(message)
        })
        .transpose()?;
    Ok(ToFabric::Result(ResultMessage {
        request_id,
        operation_id,
        state,
        resource,
        output: output.clone(),
        replayed,
        receipts,
        evidence_error,
    }))
}

fn parse_error(o: &mut Obj<'_>, profile: &Profile) -> Walk<ToFabric> {
    let request_id = token(o.req("request_id")?, "request_id")?;
    let operation_id = id(o.req("operation_id")?, "operation_id")?;
    let mut b = Obj::new(o.req("error")?, "error")?;
    let code = variant(b.req("code")?, "error.code", ERROR_CODES)?;
    let reason = string(b.req("reason")?, "error.reason")?;
    if !code.reasons().contains(&reason) {
        return Err(bad(
            "unknown_variant",
            format!("reason {reason} for {}", code.name()),
        ));
    }
    let message = message_text(b.req("message")?, "error.message")?;
    let effect = variant(b.req("effect")?, "error.effect", EFFECTS)?;
    let resource = b
        .opt("resource")?
        .map(|v| Locator::parse(v, "error.resource"))
        .transpose()?;
    if let Some(r) = &resource {
        r.fits(profile)?;
    }
    let state = b
        .opt("state")?
        .map(|v| owned(v, "error.state", 128))
        .transpose()?;
    let observations = b
        .opt("observations")?
        .map(|v| int(v, "error.observations"))
        .transpose()?;
    let deadline_ms = b
        .opt("deadline_ms")?
        .map(|v| int(v, "error.deadline_ms"))
        .transpose()?;
    let cleanup = b.opt("cleanup")?.cloned();
    b.finish()?;
    let allowed = match code {
        ErrorCode::ProviderFailed => {
            matches!(effect, EffectState::None | EffectState::Applied)
        }
        ErrorCode::Pending => effect == EffectState::Pending,
        ErrorCode::Unknown => effect == EffectState::Unknown,
        _ => effect == EffectState::None,
    };
    if !allowed {
        return Err(bad(
            "effect_mismatch",
            format!("{} with effect {}", code.name(), effect.name()),
        ));
    }
    let locator_required = code == ErrorCode::Pending
        || (code == ErrorCode::ProviderFailed && effect == EffectState::Applied);
    if locator_required && resource.is_none() {
        return Err(bad("missing_field", "error.resource"));
    }
    if code.is_refusal() && resource.is_some() {
        return Err(bad("unknown_field", "a refusal carries no resource"));
    }
    if cleanup.is_some() && !(code == ErrorCode::ProviderFailed && effect == EffectState::Applied) {
        return Err(bad(
            "unknown_field",
            "cleanup belongs to an applied provider failure",
        ));
    }
    Ok(ToFabric::Error(ErrorMessage {
        request_id,
        operation_id,
        error: ErrorBody {
            code,
            reason: reason.to_owned(),
            message,
            effect,
            resource,
            state,
            observations,
            deadline_ms,
            cleanup,
        },
    }))
}

/// The digest Fabric computes for a request: blake3 under the
/// `fabric.bridge.request.v1` domain over the canonical JSON of what the
/// request asks (`Request::digest_content`), so the same effect under
/// the same key has the same digest and another effect another one.
#[must_use]
pub fn request_digest(request: &Request) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabric.bridge.request.v1\0");
    hasher.update(canonical(&request.digest_content()).as_bytes());
    Digest(*hasher.finalize().as_bytes())
}

/// JSON text with object keys sorted at every depth, whatever order the map keeps.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}
