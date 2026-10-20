//! Rewriting JSON text without touching signed blocks.
//!
//! Invariant: an object that has a `signature` key is copied byte for byte,
//! or dropped whole when it is an array element that holds a secret; it is
//! never edited. Every other string value is decoded, redacted, mapped and
//! re-encoded, and the result is valid JSON whenever the input was. The
//! scanner reads the text once and never builds a tree, so a 100 MB
//! transcript costs one pass.

use std::ops::Range;

use crate::rewrite::{PathMap, RedactionCounts, Redactor};

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

struct StringToken {
    span: Range<usize>,
    is_key: bool,
}

struct SignedBlock {
    span: Range<usize>,
    in_array: bool,
}

#[derive(Default)]
struct Scan {
    strings: Vec<StringToken>,
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
        let is_key = match self.stack.last_mut() {
            Some(frame) if matches!(frame.expect, Expect::FirstKey | Expect::Key) => {
                frame.expect = Expect::Colon;
                if &self.text[at..end] == "\"signature\"" {
                    frame.signed = true;
                }
                true
            }
            _ => {
                self.begin_value(at)?;
                false
            }
        };
        self.out.strings.push(StringToken {
            span: at..end,
            is_key,
        });
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

/// Redacts and path-maps every string outside signed blocks of one JSON
/// text, dropping a signed array element that holds a secret.
pub fn transform(
    text: &str,
    map: &PathMap,
    redactor: Option<&Redactor>,
) -> Result<Outcome, TransformError> {
    let scan = scan(text)?;
    let mut outcome = Outcome::default();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    let mut dropped = Vec::new();
    for block in &scan.signed {
        let raw = &text[block.span.clone()];
        if redactor.is_some_and(|r| r.matches(raw)) {
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

    for token in &scan.strings {
        if token.is_key || inside_signed(&token.span) {
            continue;
        }
        let raw = &text[token.span.clone()];
        let Ok(decoded) = serde_json::from_str::<String>(raw) else {
            return Err(TransformError::NotJson {
                offset: token.span.start,
            });
        };
        let mut current: Option<String> = None;
        if let Some(r) = redactor
            && let Some(redacted) = r.apply(&decoded, &mut outcome.redactions)
        {
            current = Some(redacted);
        }
        if let Some(mapped) = map.apply(current.as_deref().unwrap_or(&decoded)) {
            current = Some(mapped);
        }
        if let Some(new) = current {
            let Ok(encoded) = serde_json::to_string(&new) else {
                return Err(TransformError::NotJson {
                    offset: token.span.start,
                });
            };
            edits.push((token.span.clone(), encoded));
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

    fn map(from: &str, to: &str) -> PathMap {
        PathMap::new([(from.to_owned(), to.to_owned())])
    }

    #[test]
    fn maps_strings_outside_signed_blocks_and_keeps_signed_bytes() {
        let line = r#"{"cwd":"/Users/x/w","message":{"content":[{"type":"thinking","thinking":"look at /Users/x/w/a.rs","signature":"abc"},{"type":"text","text":"edit /Users/x/w/a.rs"}]}}"#;
        let out = transform(line, &map("/Users/x/w", "/Users/y/w"), None).unwrap();
        let text = out.text.unwrap();
        assert!(text.contains(r#""cwd":"/Users/y/w""#));
        assert!(text.contains(r#""text":"edit /Users/y/w/a.rs""#));
        assert!(text.contains(
            r#"{"type":"thinking","thinking":"look at /Users/x/w/a.rs","signature":"abc"}"#
        ));
        serde_json::from_str::<serde_json::Value>(&text).unwrap();
    }

    #[test]
    fn unchanged_text_is_reported_as_unchanged() {
        let line = r#"{"a":"b","n":1.5e3,"t":true,"z":null,"arr":[]}"#;
        let out = transform(line, &map("/nope", "/x"), Some(Redactor::standard())).unwrap();
        assert_eq!(out.text, None);
    }

    #[test]
    fn keys_are_never_rewritten() {
        let line = r#"{"/Users/x":"/Users/x"}"#;
        let out = transform(line, &map("/Users/x", "/Users/y"), None).unwrap();
        assert_eq!(out.text.unwrap(), r#"{"/Users/x":"/Users/y"}"#);
    }

    #[test]
    fn escaped_paths_are_decoded_before_matching() {
        let line = r#"{"p":"\/Users\/x\/w"}"#;
        let out = transform(line, &map("/Users/x/w", "/Users/y/w"), None).unwrap();
        assert_eq!(out.text.unwrap(), r#"{"p":"/Users/y/w"}"#);
    }

    #[test]
    fn signed_element_with_a_secret_is_dropped_whole() {
        let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        let line = format!(
            r#"{{"content":[{{"type":"text","text":"a"}},{{"type":"thinking","thinking":"{secret}","signature":"s"}},{{"type":"text","text":"b"}}]}}"#
        );
        let out = transform(&line, &PathMap::default(), Some(Redactor::standard())).unwrap();
        assert_eq!(out.dropped_signed_blocks, 1);
        let text = out.text.unwrap();
        assert_eq!(
            text,
            r#"{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}"#
        );

        let last = format!(
            r#"{{"content":[{{"type":"text","text":"a"}},{{"thinking":"{secret}","signature":"s"}}]}}"#
        );
        let out = transform(&last, &PathMap::default(), Some(Redactor::standard())).unwrap();
        assert_eq!(
            out.text.unwrap(),
            r#"{"content":[{"type":"text","text":"a"}]}"#
        );

        let adjacent = format!(
            r#"[{{"thinking":"{secret}","signature":"a"}},{{"thinking":"{secret}","signature":"b"}}]"#
        );
        let out = transform(&adjacent, &PathMap::default(), Some(Redactor::standard())).unwrap();
        assert_eq!(out.dropped_signed_blocks, 2);
        assert_eq!(out.text.unwrap(), "[]");
        let trailing_run = format!(
            r#"[1, {{"thinking":"{secret}","signature":"a"}}, {{"thinking":"{secret}","signature":"b"}}]"#
        );
        let out = transform(
            &trailing_run,
            &PathMap::default(),
            Some(Redactor::standard()),
        )
        .unwrap();
        assert_eq!(out.text.unwrap(), "[1]");

        let only = format!(r#"{{"content":[{{"thinking":"{secret}","signature":"s"}}]}}"#);
        let out = transform(&only, &PathMap::default(), Some(Redactor::standard())).unwrap();
        assert_eq!(out.text.unwrap(), r#"{"content":[]}"#);
    }

    #[test]
    fn secret_in_a_signed_object_value_is_refused() {
        let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        let line = format!(r#"{{"block":{{"thinking":"{secret}","signature":"s"}}}}"#);
        assert_eq!(
            transform(&line, &PathMap::default(), Some(Redactor::standard())),
            Err(TransformError::SecretInUndroppableSignedBlock)
        );
    }

    #[test]
    fn truncated_and_trailing_text_are_not_json() {
        assert!(matches!(
            transform(r#"{"a":"b"#, &PathMap::default(), None),
            Err(TransformError::NotJson { .. })
        ));
        assert!(matches!(
            transform(r#"{"a":"b"} {"c":1}"#, &PathMap::default(), None),
            Err(TransformError::NotJson { .. })
        ));
        for bad in [
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
                matches!(
                    transform(bad, &PathMap::default(), None),
                    Err(TransformError::NotJson { .. })
                ),
                "{bad:?} must be refused"
            );
        }
        for good in [
            "{}",
            "[]",
            r#"{"a":[]}"#,
            r#"[{},[],""]"#,
            r#"{"a":{"b":null}}"#,
        ] {
            assert!(
                transform(good, &PathMap::default(), None).is_ok(),
                "{good:?} must be accepted"
            );
        }
        assert!(transform(r#""just a string""#, &PathMap::default(), None).is_ok());
        assert!(transform("42", &PathMap::default(), None).is_ok());
    }
}
