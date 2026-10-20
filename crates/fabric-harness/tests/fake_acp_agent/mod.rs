//! A fake ACP agent for the integration tests. `acp_roundtrip` starts its
//! own test binary as the agent, which then runs `serve` instead of a test.
//!
//! It speaks the ACP v1 baseline over stdio, keeps one JSON file per
//! session under `FAKE_STATE_DIR`, and lets the tests shape its
//! capabilities through environment variables:
//! `FAKE_LOAD=0` drops `loadSession`, `FAKE_RESUME=1` advertises
//! `session/resume`, `FAKE_ASK_PERMISSION=1` asks for permission on every
//! prompt, `FAKE_PID_FILE` names a file it writes its pid to, and
//! `FAKE_LINGER=1` keeps it alive after its stdin closes, as a hung agent
//! would.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

type Lines<'a> = std::io::Lines<std::io::StdinLock<'a>>;
type Reply = Result<Value, (i64, String)>;

struct Agent {
    state_dir: PathBuf,
    counter: u64,
    next_request_id: u64,
    out: std::io::Stdout,
}

fn env_flag(name: &str, default: bool) -> bool {
    std::env::var(name).map_or(default, |v| v == "1" || (default && v != "0"))
}

impl Agent {
    fn send(&mut self, value: &Value) -> std::io::Result<()> {
        let mut text = serde_json::to_string(value).map_err(std::io::Error::other)?;
        text.push('\n');
        let mut lock = self.out.lock();
        lock.write_all(text.as_bytes())?;
        lock.flush()
    }

    fn update(&mut self, session: &str, text: &str) -> std::io::Result<()> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text }
                }
            }
        }))
    }

    fn state_path(&self, session: &str) -> PathBuf {
        self.state_dir.join(format!("{session}.json"))
    }

    fn load_state(&self, session: &str) -> Option<Value> {
        let bytes = std::fs::read(self.state_path(session)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_state(&self, session: &str, state: &Value) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.state_path(session), serde_json::to_vec_pretty(state)?)
    }

    fn initialize() -> Value {
        let mut session_caps = serde_json::Map::new();
        if env_flag("FAKE_RESUME", false) {
            session_caps.insert("resume".to_owned(), json!({}));
        }
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": env_flag("FAKE_LOAD", true),
                "sessionCapabilities": session_caps
            },
            "agentInfo": { "name": "fake-acp-agent", "version": "0.1.0" }
        })
    }

    fn new_session(&mut self, params: &Value) -> std::io::Result<Reply> {
        self.counter += 1;
        let session = format!("fake-{}", self.counter);
        let cwd = params.get("cwd").cloned().unwrap_or(Value::Null);
        self.save_state(
            &session,
            &json!({ "cwd": cwd, "prompts": [], "loaded_from": [] }),
        )?;
        Ok(Ok(json!({ "sessionId": session })))
    }

    fn continue_session(&mut self, method: &str, params: &Value) -> std::io::Result<Reply> {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let Some(mut state) = self.load_state(&session) else {
            return Ok(Err((-32602, format!("unknown session {session}"))));
        };
        let cwd = params.get("cwd").cloned().unwrap_or(Value::Null);
        if let Some(list) = state.get_mut("loaded_from").and_then(Value::as_array_mut) {
            list.push(cwd.clone());
        }
        self.save_state(&session, &state)?;
        if method == "session/load" {
            self.update(&session, &format!("replayed; cwd now {cwd}"))?;
        }
        Ok(Ok(json!({})))
    }

    fn ask_permission(&mut self, session: &str, lines: &mut Lines<'_>) -> std::io::Result<Value> {
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/request_permission",
            "params": {
                "sessionId": session,
                "toolCall": { "toolCallId": "call-1", "title": "run a command" },
                "options": [
                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                    { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
                ]
            }
        }))?;
        for line in lines.by_ref() {
            let line = line?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) == Some(request_id) {
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        Ok(Value::Null)
    }

    fn prompt(&mut self, params: &Value, lines: &mut Lines<'_>) -> std::io::Result<Reply> {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let text = params
            .pointer("/prompt/0/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let Some(mut state) = self.load_state(&session) else {
            return Ok(Err((-32602, "unknown session".to_owned())));
        };
        let permission = if env_flag("FAKE_ASK_PERMISSION", false) {
            self.ask_permission(&session, lines)?
        } else {
            Value::Null
        };
        if let Some(list) = state.get_mut("prompts").and_then(Value::as_array_mut) {
            list.push(json!({ "text": text, "permission": permission }));
        }
        self.save_state(&session, &state)?;
        self.update(&session, &format!("echo: {text}"))?;
        Ok(Ok(json!({ "stopReason": "end_turn" })))
    }

    fn handle(
        &mut self,
        id: &Value,
        method: &str,
        params: &Value,
        lines: &mut Lines<'_>,
    ) -> std::io::Result<()> {
        let reply = match method {
            "initialize" => Ok(Self::initialize()),
            "session/new" => self.new_session(params)?,
            "session/load" | "session/resume" => self.continue_session(method, params)?,
            "session/prompt" => self.prompt(params, lines)?,
            other => Err((-32601, format!("method not found: {other}"))),
        };
        let response = match reply {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        };
        self.send(&response)
    }

    fn cancel(&mut self, params: &Value) -> std::io::Result<()> {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(mut state) = self.load_state(session) {
            state["cancelled"] = Value::Bool(true);
            self.save_state(session, &state)?;
        }
        Ok(())
    }
}

fn run() -> std::io::Result<()> {
    let state_dir =
        std::env::var_os("FAKE_STATE_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let mut agent = Agent {
        state_dir,
        counter: 0,
        next_request_id: 1000,
        out: std::io::stdout(),
    };
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (message.get("id").cloned(), method) {
            (Some(id), Some(method)) => agent.handle(&id, &method, &params, &mut lines)?,
            (None, Some(method)) if method == "session/cancel" => agent.cancel(&params)?,
            _ => {}
        }
    }
    Ok(())
}

/// Runs the agent until its stdin closes; with `FAKE_LINGER=1` it then
/// sleeps forever, as a hung agent would.
pub fn serve() -> std::io::Result<()> {
    if let Some(path) = std::env::var_os("FAKE_PID_FILE") {
        std::fs::write(path, std::process::id().to_string())?;
    }
    let result = run();
    if env_flag("FAKE_LINGER", false) {
        loop {
            std::thread::sleep(std::time::Duration::from_mins(1));
        }
    }
    result
}
