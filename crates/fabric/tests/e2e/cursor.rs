//! Cursor Agent CLI: a job under `--harness cursor` is suspended on one
//! node and resumed on the other with a fake `cursor-agent` that keeps the
//! agent's real on-disk layout, `~/.cursor/chats/<md5 of the workspace>/<chat>/`
//! with a `store.db` that holds its key beside its blobs, and a
//! `cli-config.json` with a login. Nothing of the chat travels (decision
//! of 2026-11-08): the target starts a new chat in the restored
//! workspace, at `workspace_only`, and answers a question about the first
//! turn from what that turn left in the workspace. The fake is installed
//! from here through `Env::install`, so `world.rs` stays untouched.

use std::fs;
use std::process::{Command, Stdio};

use crate::world::{Env, Fallible, Wire};

/// The id of the n-th chat the fake makes, from 0.
fn chat(n: u32) -> String {
    format!("3c9e1f2a-7b4d-4e8f-9a1b-{n:012}")
}

/// The fake `cursor-agent`, from the layout of 2026.10.01-e373342. Each
/// chat it makes takes the next id from `~/cursor-chats`. A first turn
/// leaves `NOTES.md` in the workspace; a question about the first turn is
/// answered from that file, since no chat travels. With
/// `FAKE_CURSOR_HOLD` set it holds the chat's store open and waits.
const FAKE: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo 2026.10.01-e373342; exit 0; fi
printf '%s\n' "$@" > "$HOME/cursor-agent-args"
resume=; prompt=; ended=
while [ $# -gt 0 ]; do
  if [ -n "$ended" ]; then prompt=$1
  else case "$1" in --resume) resume=$2; shift ;; --) ended=1 ;; esac
  fi
  shift
done
root=$(pwd -P)
if command -v md5sum >/dev/null 2>&1; then hash=$(printf %s "$root" | md5sum | cut -c1-32)
elif [ -x /sbin/md5 ]; then hash=$(printf %s "$root" | /sbin/md5 -q)
else hash=$(printf %s "$root" | md5 -q); fi
[ ${#hash} -eq 32 ] || { echo "cursor-agent fake: no md5" >&2; exit 2; }
if [ -n "$resume" ]; then id=$resume
else
  n=0; [ ! -f "$HOME/cursor-chats" ] || n=$(cat "$HOME/cursor-chats")
  echo $((n + 1)) > "$HOME/cursor-chats"
  id=$(printf '3c9e1f2a-7b4d-4e8f-9a1b-%012d' "$n")
fi
store="$HOME/.cursor/chats/$hash/$id"
mkdir -p "$store"
printf 'blobs and meta: blobEncryptionKey sk-ant-api03-FAKEFAKEFAKEFAKEFAKEFAKEFAKE0123\n' >> "$store/store.db"
printf '{"schemaVersion":1,"cwd":"%s","hasConversation":true}\n' "$root" > "$store/meta.json"
[ -f "$HOME/.cursor/cli-config.json" ] || printf '{"authInfo":{"accessToken":"FAKE-not-a-token"}}\n' > "$HOME/.cursor/cli-config.json"
if [ -n "$FAKE_CURSOR_HOLD" ]; then
  exec 3<"$store/store.db"
  : > "$HOME/cursor-holding"
  while :; do sleep 0.1; done
fi
case "$prompt" in
  *"first turn"*)
    if [ -f NOTES.md ]; then answer="from the workspace: $(cat NOTES.md)"; else answer="nothing to go on"; fi ;;
  *)
    [ -f NOTES.md ] || printf 'the first turn asked: %s\n' "$prompt" > NOTES.md
    answer="cursor did: $prompt" ;;
esac
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$id"
printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]},"session_id":"%s"}\n' "$answer" "$id"
printf '{"type":"result","subtype":"success","is_error":false,"result":"%s","session_id":"%s"}\n' "$answer" "$id"
"#;

fn install(env: &Env) -> Fallible {
    env.install("cursor-agent", FAKE)
}

/// A `cursor-agent` that holds a chat open, killed when dropped, so a
/// failed assertion never leaves it running.
struct Holder(std::process::Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the fake outside any node, in the job's workspace, continuing
/// `id` and holding its store open, as a `cursor-agent` the user left
/// running would; returns once it holds it.
fn hold_chat(env: &Env, job: &str, id: &str) -> Fallible<Holder> {
    let child = Command::new(env.bin().join("cursor-agent"))
        .args(["--trust", "--resume", id])
        .current_dir(env.work(job))
        .env_clear()
        .env("HOME", &env.home)
        .env("PATH", env.path())
        .env("FAKE_CURSOR_HOLD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let holder = Holder(child);
    crate::world::wait_for(
        &env.home.join("cursor-holding"),
        "the fake to hold the chat",
    )?;
    Ok(holder)
}

/// Suspend, move, resume: the snapshot carries the workspace and no byte
/// of the chat or the login, the target starts a new chat and says the
/// old one stayed behind, the question about the first turn is answered
/// there from the workspace, and the restore reached `workspace_only`. A
/// suspend while a `cursor-agent` still holds the chat open is refused
/// first, and goes through once it has exited.
#[test]
fn e2e_a_cursor_job_moves_as_its_workspace_and_a_new_chat_answers_from_it() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    install(&p.env)?;
    let job = crate::new_job(&p.env, &p.a, "cursor")?;
    let first = chat(0);
    let ran = p.a.ok(&[
        "job",
        "run",
        &job,
        "--headless",
        "--prompt",
        "remember the word teal",
    ])?;
    assert!(ran.contains("cursor did: remember the word teal"), "{ran}");
    assert!(
        ran.contains(&format!("recorded cursor-agent session {first}")),
        "{ran}"
    );

    let holder = hold_chat(&p.env, &job, &first)?;
    let refused = p.a.fails(&["job", "suspend", &job])?;
    assert!(refused.contains("cursor-agent is running"), "{refused}");
    drop(holder);

    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured cursor-agent session {first} (0 files)")),
        "{suspended}"
    );
    let manifest = fs::read_to_string(
        p.a.state()
            .join("jobs")
            .join(&job)
            .join("harness-state/manifest.json"),
    )?;
    assert!(manifest.contains("cursor-store-not-carried"), "{manifest}");
    for absent in ["store.db", "cli-config", "FAKE"] {
        assert!(
            !manifest.contains(absent),
            "{absent} travelled:\n{manifest}"
        );
    }

    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    // As on a second machine, the target holds no Cursor chat.
    fs::remove_dir_all(p.env.home.join(".cursor/chats"))?;
    let resumed = p.b.ok(&[
        "job",
        "resume",
        &job,
        "--headless",
        "--prompt",
        "what did the first turn ask?",
    ])?;
    assert!(
        resumed.contains("from the workspace: the first turn asked: remember the word teal"),
        "the question about the first turn was answered on the resumed side:\n{resumed}"
    );
    assert!(
        resumed.contains(&format!("chat {first} is not carried ("))
            && resumed.contains("workspace_only"),
        "the resume says the conversation stayed behind:\n{resumed}"
    );
    assert!(
        resumed.contains(&format!("recorded cursor-agent session {}", chat(1))),
        "{resumed}"
    );
    let args = p.env.fake_args("cursor-agent")?;
    assert_eq!(
        args,
        [
            "--print",
            "--output-format",
            "stream-json",
            "--trust",
            "--",
            "what did the first turn ask?"
        ]
    );
    let log = p.b.log();
    assert!(
        log.contains(&format!(
            "restored cursor-agent session {first} at fidelity workspace_only"
        )),
        "{log}"
    );
    Ok(())
}
