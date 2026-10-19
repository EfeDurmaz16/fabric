//! A step's output reaches the caller's scoped log, line by line.
//!
//! This test is alone in its own binary on purpose. `tracing` computes a
//! callsite's interest the first time any thread hits it; while exactly one
//! dispatcher is registered in the process, it asks that thread's default,
//! and a thread without the scoped subscriber answers "never", which is
//! then cached for every thread. Another test running a step at the same
//! time can therefore make this one see no lines. A process of its own
//! removes that.

use std::fs;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use fabric_env::detect::detect;
use fabric_env::materialize::plan;

/// A buffer the log writes into, shared with the test.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self
            .0
            .lock()
            .map_err(|_| io::Error::other("log buffer poisoned"))?;
        bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Output is logged from reader threads, which would drop it if they did
/// not carry the caller's subscriber.
#[test]
fn step_output_reaches_the_callers_log_line_by_line() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".fabric")).unwrap();
    fs::write(
        dir.path().join(".fabric/env.toml"),
        r#"
[[setup]]
program = "sh"
args = ["-c", "echo first-out; echo first-err >&2; echo second-out"]
when = "always"
"#,
    )
    .unwrap();
    let detected = detect(dir.path()).unwrap();
    let planned = plan(dir.path(), &detected).unwrap();
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || planned.execute()).unwrap();
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("fabric_env::setup") && line.contains("stream="))
        .collect();
    assert_eq!(lines.len(), 3, "{log}");
    let first_out = lines.iter().position(|l| l.contains("first-out")).unwrap();
    let second_out = lines.iter().position(|l| l.contains("second-out")).unwrap();
    assert!(first_out < second_out);
    assert!(lines[first_out].contains("stream=\"stdout\""));
    let err = lines.iter().find(|l| l.contains("first-err")).unwrap();
    assert!(err.contains("stream=\"stderr\""));
}
