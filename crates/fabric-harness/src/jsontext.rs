//! Redacting JSON text without touching signed blocks.
//!
//! Invariant: a signed block is copied byte for byte, or dropped whole when
//! it is an array element that holds a secret; it is never edited. A signed
//! block is a Claude content block whose signature the API checks: an
//! object whose `type` is `thinking` and that has a `signature` key, or an
//! object whose `type` is `redacted_thinking`. Any other object with a
//! `signature` key (a webhook payload, a JWS in a tool result) is ordinary
//! JSON. A signed block nested in another is covered by the outer one.
//! Every other string value is decoded, redacted and re-encoded, and the
//! result is valid JSON whenever the input was. The scanner reads one JSON
//! text once and never builds a tree; a transcript is fed to it one line at
//! a time.

use std::ops::Range;

use crate::redact::{RedactionCounts, Redactor};

/// Why a text could not be transformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransformError {
    /// The text is not one JSON value.
    #[error("not JSON at byte {offset}")]
    NotJson {
        /// Byte offset of the first error.
        offset: usize,
    },
    /// A secret sits inside a signed block that is not an array element.
    #[error("a secret sits inside a signed block that cannot be dropped")]
    SecretInUndroppableSignedBlock,
}

/// What one transform did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    /// The text after the transform, or `None` when nothing changed.
    pub text: Option<String>,
    /// Redaction matches per rule.
    pub redactions: RedactionCounts,
    /// Signed blocks dropped because they held a secret.
    pub dropped_signed_blocks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Object,
    Array,
}

/// What the grammar allows next inside the innermost container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A key, or the closing brace of an empty object.
    FirstKey,
    /// A key after a comma.
    Key,
    /// The colon after a key.
    Colon,
    /// A value, or the closing bracket of an empty array.
    FirstValue,
    /// A value after a colon or a comma.
    Value,
    /// A comma or the closing bracket or brace.
    CommaOrEnd,
}

/// The `type` of an object, as far as signing is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    /// Any other type, or none.
    Other,
    /// `"type": "thinking"`.
    Thinking,
    /// `"type": "redacted_thinking"`.
    RedactedThinking,
}

struct Frame {
    kind: Container,
    start: usize,
    has_signature: bool,
    block: BlockType,
    /// True between a `"type"` key and its value.
    awaiting_type: bool,
    expect: Expect,
}

impl Frame {
    fn is_signed(&self) -> bool {
        match self.block {
            BlockType::Thinking => self.has_signature,
            BlockType::RedactedThinking => true,
            BlockType::Other => false,
        }
    }
}

struct SignedBlock {
    span: Range<usize>,
    in_array: bool,
}

#[derive(Default)]
struct Scan {
    strings: Vec<Range<usize>>,
    signed: Vec<SignedBlock>,
}

fn scan_string(bytes: &[u8], start: usize) -> Result<usize, TransformError> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Ok(i + 1),
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
    Err(TransformError::NotJson { offset: start })
}

/// Scans a number or a literal starting at `start`; returns the end offset.
fn scan_scalar(text: &str, start: usize) -> Result<usize, TransformError> {
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len()
        && !matches!(
            bytes[end],
            b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'
        )
    {
        end += 1;
    }
    let token = &text[start..end];
    let ok = matches!(token, "true" | "false" | "null") || token.parse::<f64>().is_ok();
    if ok {
        Ok(end)
    } else {
        Err(TransformError::NotJson { offset: start })
    }
}

/// The scanner state: the open containers plus whether the top-level value
/// has been seen.
struct Scanner<'a> {
    text: &'a str,
    stack: Vec<Frame>,
    seen_value: bool,
    out: Scan,
}

fn not_json<T>(offset: usize) -> Result<T, TransformError> {
    Err(TransformError::NotJson { offset })
}

impl Scanner<'_> {
    /// Checks that a value may start here and marks the slot taken.
    fn begin_value(&mut self, at: usize) -> Result<(), TransformError> {
        match self.stack.last_mut() {
            Some(frame) => match frame.expect {
                Expect::FirstValue | Expect::Value => {
                    frame.expect = Expect::CommaOrEnd;
                    Ok(())
                }
                _ => not_json(at),
            },
            None if self.seen_value => not_json(at),
            None => {
                self.seen_value = true;
                Ok(())
            }
        }
    }

    fn open(&mut self, byte: u8, at: usize) -> Result<(), TransformError> {
        self.begin_value(at)?;
        let (kind, expect) = if byte == b'{' {
            (Container::Object, Expect::FirstKey)
        } else {
            (Container::Array, Expect::FirstValue)
        };
        self.stack.push(Frame {
            kind,
            start: at,
            has_signature: false,
            block: BlockType::Other,
            awaiting_type: false,
            expect,
        });
        Ok(())
    }

    fn close(&mut self, byte: u8, at: usize) -> Result<(), TransformError> {
        let Some(frame) = self.stack.pop() else {
            return not_json(at);
        };
        let closes = if byte == b'}' {
            Container::Object
        } else {
            Container::Array
        };
        let may_close = match frame.expect {
            Expect::CommaOrEnd => true,
            Expect::FirstKey => frame.kind == Container::Object,
            Expect::FirstValue => frame.kind == Container::Array,
            Expect::Key | Expect::Colon | Expect::Value => false,
        };
        if frame.kind != closes || !may_close {
            return not_json(at);
        }
        if frame.is_signed() {
            let in_array = self
                .stack
                .last()
                .is_some_and(|f| f.kind == Container::Array);
            self.out.signed.push(SignedBlock {
                span: frame.start..at + 1,
                in_array,
            });
        }
        Ok(())
    }

    fn string(&mut self, at: usize) -> Result<usize, TransformError> {
        let end = scan_string(self.text.as_bytes(), at)?;
        let raw = &self.text[at..end];
        match self.stack.last_mut() {
            Some(frame) if matches!(frame.expect, Expect::FirstKey | Expect::Key) => {
                frame.expect = Expect::Colon;
                frame.awaiting_type = raw == "\"type\"";
                if raw == "\"signature\"" {
                    frame.has_signature = true;
                }
            }
            _ => {
                self.begin_value(at)?;
                if let Some(frame) = self.stack.last_mut()
                    && frame.kind == Container::Object
                    && frame.awaiting_type
                {
                    frame.block = match serde_json::from_str::<String>(raw).as_deref() {
                        Ok("thinking") => BlockType::Thinking,
                        Ok("redacted_thinking") => BlockType::RedactedThinking,
                        _ => BlockType::Other,
                    };
                }
            }
        }
        if let Some(frame) = self.stack.last_mut()
            && frame.expect != Expect::Colon
        {
            frame.awaiting_type = false;
        }
        self.out.strings.push(at..end);
        Ok(end)
    }

    fn colon(&mut self, at: usize) -> Result<(), TransformError> {
        match self.stack.last_mut() {
            Some(frame) if frame.expect == Expect::Colon => {
                frame.expect = Expect::Value;
                Ok(())
            }
            _ => not_json(at),
        }
    }

    fn comma(&mut self, at: usize) -> Result<(), TransformError> {
        match self.stack.last_mut() {
            Some(frame) if frame.expect == Expect::CommaOrEnd => {
                frame.expect = if frame.kind == Container::Object {
                    Expect::Key
                } else {
                    Expect::Value
                };
                Ok(())
            }
            _ => not_json(at),
        }
    }

    fn run(mut self) -> Result<Scan, TransformError> {
        let bytes = self.text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                b'{' | b'[' => {
                    self.open(b, i)?;
                    i += 1;
                }
                b'}' | b']' => {
                    self.close(b, i)?;
                    i += 1;
                }
                b'"' => i = self.string(i)?,
                b':' => {
                    self.colon(i)?;
                    i += 1;
                }
                b',' => {
                    self.comma(i)?;
                    i += 1;
                }
                _ => {
                    self.begin_value(i)?;
                    i = scan_scalar(self.text, i)?;
                }
            }
        }
        if !self.stack.is_empty() || !self.seen_value {
            return not_json(self.text.len());
        }
        Ok(self.out)
    }
}

fn scan(text: &str) -> Result<Scan, TransformError> {
    Scanner {
        text,
        stack: Vec::new(),
        seen_value: false,
        out: Scan::default(),
    }
    .run()
}

/// True when only whitespace and one comma separate `end` from `start`,
/// which makes two array elements neighbours.
fn only_a_comma_between(text: &str, end: usize, start: usize) -> bool {
    let Some(between) = text.get(end..start) else {
        return false;
    };
    between.matches(',').count() == 1
        && between
            .chars()
            .all(|c| c == ',' || matches!(c, ' ' | '\t' | '\n' | '\r'))
}

/// Joins runs of neighbouring dropped elements into one span each, so a
/// run is removed with one comma, whichever side it is on.
fn merge_neighbours(text: &str, mut spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    spans.sort_by_key(|span| span.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if only_a_comma_between(text, last.end, span.start) => {
                last.end = span.end;
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// The byte range to remove so that the array element at `span` disappears
/// and the array stays valid.
fn element_removal(text: &str, span: &Range<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut after = span.end;
    while after < bytes.len() && matches!(bytes[after], b' ' | b'\t' | b'\n' | b'\r') {
        after += 1;
    }
    if after < bytes.len() && bytes[after] == b',' {
        return span.start..after + 1;
    }
    let mut before = span.start;
    while before > 0 && matches!(bytes[before - 1], b' ' | b'\t' | b'\n' | b'\r') {
        before -= 1;
    }
    if before > 0 && bytes[before - 1] == b',' {
        return before - 1..span.end;
    }
    span.clone()
}

/// Redacts every string outside signed blocks of one JSON text, dropping
/// a signed array element that holds a secret.
pub fn transform(text: &str, redactor: &Redactor) -> Result<Outcome, TransformError> {
    let scan = scan(text)?;
    let mut outcome = Outcome::default();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    // A signed block nested in another one that is dropped goes with it,
    // so only the outermost blocks that hold a secret are considered.
    let mut holding: Vec<&SignedBlock> = scan
        .signed
        .iter()
        .filter(|block| redactor.matches(&text[block.span.clone()]))
        .collect();
    holding.sort_by_key(|block| (block.span.start, std::cmp::Reverse(block.span.end)));
    let mut dropped: Vec<Range<usize>> = Vec::new();
    for block in holding {
        if dropped
            .last()
            .is_some_and(|outer| block.span.end <= outer.end)
        {
            continue;
        }
        if !block.in_array {
            return Err(TransformError::SecretInUndroppableSignedBlock);
        }
        dropped.push(block.span.clone());
        outcome.dropped_signed_blocks += 1;
    }
    for run in merge_neighbours(text, dropped) {
        edits.push((element_removal(text, &run), String::new()));
    }

    let inside_signed = |span: &Range<usize>| {
        scan.signed
            .iter()
            .any(|b| b.span.start <= span.start && span.end <= b.span.end)
    };

    for span in &scan.strings {
        if inside_signed(span) {
            continue;
        }
        let raw = &text[span.clone()];
        let Ok(decoded) = serde_json::from_str::<String>(raw) else {
            return Err(TransformError::NotJson { offset: span.start });
        };
        if let Some(new) = redactor.apply(&decoded, &mut outcome.redactions) {
            let Ok(encoded) = serde_json::to_string(&new) else {
                return Err(TransformError::NotJson { offset: span.start });
            };
            edits.push((span.clone(), encoded));
        }
    }

    if edits.is_empty() {
        return Ok(outcome);
    }
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (span, replacement) in edits {
        if span.start < cursor {
            // Edits never overlap: a removal holds no string, and strings
            // inside signed blocks are not edited. An overlap means the scan
            // is wrong, so the text is refused rather than half edited; the
            // caller then redacts it as plain text.
            return Err(TransformError::NotJson { offset: span.start });
        }
        out.push_str(&text[cursor..span.start]);
        out.push_str(&replacement);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    outcome.text = Some(out);
    Ok(outcome)
}

#[cfg(test)]
mod props;

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
    const ANTHROPIC: &str = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";

    fn redact(text: &str) -> Result<Outcome, TransformError> {
        transform(text, Redactor::standard().unwrap())
    }

    #[test]
    fn redacts_strings_outside_signed_blocks_and_keeps_signed_bytes() {
        let line = format!(
            r#"{{"cwd":"/w","message":{{"content":[{{"type":"thinking","thinking":"plan","signature":"abc"}},{{"type":"text","text":"token {GITHUB}"}}]}}}}"#
        );
        let text = redact(&line).unwrap().text.unwrap();
        assert!(text.contains(r#""text":"token [REDACTED:github-token]""#));
        assert!(text.contains(r#"{"type":"thinking","thinking":"plan","signature":"abc"}"#));
        serde_json::from_str::<serde_json::Value>(&text).unwrap();
    }

    #[test]
    fn unchanged_text_is_reported_as_unchanged() {
        let line = r#"{"a":"b","n":1.5e3,"t":true,"z":null,"arr":[]}"#;
        assert_eq!(redact(line).unwrap().text, None);
    }

    #[test]
    fn keys_are_redacted_like_values() {
        let line = format!(r#"{{"{GITHUB}":"{GITHUB}"}}"#);
        assert_eq!(
            redact(&line).unwrap().text.unwrap(),
            r#"{"[REDACTED:github-token]":"[REDACTED:github-token]"}"#
        );
    }

    #[test]
    fn escaped_strings_are_decoded_before_matching() {
        let line = r#"{"p":"ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab"}"#;
        assert_eq!(
            redact(line).unwrap().text.unwrap(),
            r#"{"p":"[REDACTED:github-token]"}"#
        );
    }

    #[test]
    fn signed_element_with_a_secret_is_dropped_whole() {
        let line = format!(
            r#"{{"content":[{{"type":"text","text":"a"}},{{"type":"thinking","thinking":"{ANTHROPIC}","signature":"s"}},{{"type":"text","text":"b"}}]}}"#
        );
        let out = redact(&line).unwrap();
        assert_eq!(out.dropped_signed_blocks, 1);
        assert_eq!(
            out.text.unwrap(),
            r#"{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}"#
        );

        let last = format!(
            r#"{{"content":[{{"type":"text","text":"a"}},{{"type":"thinking","thinking":"{ANTHROPIC}","signature":"s"}}]}}"#
        );
        assert_eq!(
            redact(&last).unwrap().text.unwrap(),
            r#"{"content":[{"type":"text","text":"a"}]}"#
        );

        let only = format!(
            r#"{{"content":[{{"type":"thinking","thinking":"{ANTHROPIC}","signature":"s"}}]}}"#
        );
        assert_eq!(redact(&only).unwrap().text.unwrap(), r#"{"content":[]}"#);
    }

    #[test]
    fn neighbouring_signed_elements_with_secrets_are_all_dropped() {
        let adjacent = format!(
            r#"[{{"type":"thinking","thinking":"{ANTHROPIC}","signature":"a"}},{{"type":"thinking","thinking":"{GITHUB}","signature":"b"}}]"#
        );
        let out = redact(&adjacent).unwrap();
        assert_eq!(out.dropped_signed_blocks, 2);
        assert_eq!(out.text.unwrap(), "[]");
        let trailing_run = format!(
            r#"[1, {{"type":"thinking","thinking":"{ANTHROPIC}","signature":"a"}}, {{"type":"thinking","thinking":"{GITHUB}","signature":"b"}}]"#
        );
        assert_eq!(redact(&trailing_run).unwrap().text.unwrap(), "[1]");
    }

    #[test]
    fn secret_in_a_signed_object_value_is_refused() {
        let line = format!(
            r#"{{"block":{{"type":"thinking","thinking":"{ANTHROPIC}","signature":"s"}}}}"#
        );
        assert_eq!(
            redact(&line),
            Err(TransformError::SecretInUndroppableSignedBlock)
        );
    }

    #[test]
    fn truncated_and_trailing_text_are_not_json() {
        for bad in [
            r#"{"a":"b"#,
            r#"{"a":"b"} {"c":1}"#,
            r#"{"a" "b"}"#,
            r#"{"a":1,}"#,
            "[1,]",
            r#"{"a"}"#,
            "{:1}",
            "[,1]",
            r#"{"a":1 "b":2}"#,
            "",
            "   ",
        ] {
            assert!(
                matches!(redact(bad), Err(TransformError::NotJson { .. })),
                "{bad:?} must be refused"
            );
        }
        for good in [
            "{}",
            "[]",
            r#"{"a":[]}"#,
            r#"[{},[],""]"#,
            r#"{"a":{"b":null}}"#,
            r#""just a string""#,
            "42",
        ] {
            assert!(redact(good).is_ok(), "{good:?} must be accepted");
        }
    }

    #[test]
    fn an_object_with_a_signature_key_that_is_not_a_thinking_block_is_redacted() {
        let line = format!(
            r#"[{{"signature":"a","items":[{{"signature":"b","t":"{GITHUB}"}}]}},{{"payload":{{"signature":"c","k":"{GITHUB}"}}}}]"#
        );
        let out = redact(&line).unwrap();
        assert_eq!(out.dropped_signed_blocks, 0);
        assert_eq!(
            out.text.unwrap(),
            r#"[{"signature":"a","items":[{"signature":"b","t":"[REDACTED:github-token]"}]},{"payload":{"signature":"c","k":"[REDACTED:github-token]"}}]"#
        );
    }

    #[test]
    fn a_signed_block_nested_in_a_dropped_one_goes_with_it() {
        let inner = format!(r#"{{"type":"thinking","thinking":"{GITHUB}","signature":"b"}}"#);
        let line = format!(
            r#"{{"content":[{{"type":"text","text":"a"}},{{"type":"thinking","thinking":"x","signature":"a","items":[{inner},{inner}]}},{{"type":"redacted_thinking","data":"{GITHUB}"}}]}}"#
        );
        let out = redact(&line).unwrap();
        assert_eq!(out.dropped_signed_blocks, 2);
        assert_eq!(
            out.text.unwrap(),
            r#"{"content":[{"type":"text","text":"a"}]}"#
        );
    }
}
