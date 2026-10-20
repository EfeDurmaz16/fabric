//! Redacting JSON text without touching signed blocks.
//!
//! Invariant: an object that has a `signature` key is copied byte for byte,
//! or dropped whole when it is an array element that holds a secret; it is
//! never edited. Every other string value is decoded, redacted and
//! re-encoded, and the result is valid JSON whenever the input was. The
//! scanner reads one JSON text once and never builds a tree; a transcript
//! is fed to it one line at a time.

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

struct Frame {
    kind: Container,
    start: usize,
    signed: bool,
    expect: Expect,
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
            signed: false,
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
        if frame.signed {
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
        match self.stack.last_mut() {
            Some(frame) if matches!(frame.expect, Expect::FirstKey | Expect::Key) => {
                frame.expect = Expect::Colon;
                if &self.text[at..end] == "\"signature\"" {
                    frame.signed = true;
                }
            }
            _ => self.begin_value(at)?,
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
    let between = &text[end..start];
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

    let mut dropped = Vec::new();
    for block in &scan.signed {
        let raw = &text[block.span.clone()];
        if redactor.matches(raw) {
            if !block.in_array {
                return Err(TransformError::SecretInUndroppableSignedBlock);
            }
            dropped.push(block.span.clone());
            outcome.dropped_signed_blocks += 1;
        }
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
mod tests {
    use super::*;

    const GITHUB: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ab";
    const ANTHROPIC: &str = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";

    fn redact(text: &str) -> Result<Outcome, TransformError> {
        transform(text, Redactor::standard())
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
            r#"{{"content":[{{"type":"text","text":"a"}},{{"thinking":"{ANTHROPIC}","signature":"s"}}]}}"#
        );
        assert_eq!(
            redact(&last).unwrap().text.unwrap(),
            r#"{"content":[{"type":"text","text":"a"}]}"#
        );

        let only = format!(r#"{{"content":[{{"thinking":"{ANTHROPIC}","signature":"s"}}]}}"#);
        assert_eq!(redact(&only).unwrap().text.unwrap(), r#"{"content":[]}"#);
    }

    #[test]
    fn neighbouring_signed_elements_with_secrets_are_all_dropped() {
        let adjacent = format!(
            r#"[{{"thinking":"{ANTHROPIC}","signature":"a"}},{{"thinking":"{GITHUB}","signature":"b"}}]"#
        );
        let out = redact(&adjacent).unwrap();
        assert_eq!(out.dropped_signed_blocks, 2);
        assert_eq!(out.text.unwrap(), "[]");
        let trailing_run = format!(
            r#"[1, {{"thinking":"{ANTHROPIC}","signature":"a"}}, {{"thinking":"{GITHUB}","signature":"b"}}]"#
        );
        assert_eq!(redact(&trailing_run).unwrap().text.unwrap(), "[1]");
    }

    #[test]
    fn secret_in_a_signed_object_value_is_refused() {
        let line = format!(r#"{{"block":{{"thinking":"{ANTHROPIC}","signature":"s"}}}}"#);
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
}
