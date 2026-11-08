//! Gemini CLI: a job under `--harness gemini` is suspended on one node and
//! resumed on the other with a fake `gemini` that keeps the agent's real
//! on-disk layout, and the refusals of docs/harness-adapters.md hold. The
//! fake files its chat as `~/.gemini/tmp/<slug>/chats/session-<ts>-<id8>.jsonl`
//! beside a second session that shares the 8-character prefix, keeps the
//! slug in `projects.json` and a `.project_root` marker, writes a fake
//! `oauth_creds.json`, and answers a question about the first turn from
//! the chat it resumes. It is installed from here through `Env::install`,
//! so `world.rs` stays untouched.

use std::fs;
use std::path::PathBuf;

use crate::world::{Env, Fallible, Wire, text};

/// The session the fake makes.
const SESSION: &str = "6f1d2c3b-4a5e-4b6c-8d7e-9f0a1b2c3d4e";
/// Another session of the same workspace that shares its 8-character prefix.
const DECOY: &str = "6f1d2c3b-0000-4000-8000-000000000001";

/// The fake `gemini`, from the format of Gemini CLI 0.67.0. It answers
/// `--version` with 0.67.0, or what `gemini-version` in the node's state
/// directory says, and writes its arguments to `~/gemini-args`.
const FAKE: &str = r#"#!/bin/sh
version=0.67.0
[ ! -f "$FABRIC_HOME/gemini-version" ] || version=$(cat "$FABRIC_HOME/gemini-version")
if [ "$1" = --version ]; then echo "$version"; exit 0; fi
printf '%s\n' "$@" > "$HOME/gemini-args"
resume=; model=gemini-3-pro; prompt=
while [ $# -gt 0 ]; do
  case "$1" in
    --resume) resume=$2; shift ;;
    --model) model=$2; shift ;;
    --prompt=*) prompt=${1#--prompt=} ;;
  esac
  shift
done
root=$(pwd -P)
base="$HOME/.gemini"
slug=
for marker in "$base"/tmp/*/.project_root; do
  if [ -f "$marker" ] && [ "$(cat "$marker")" = "$root" ]; then slug=$(basename "$(dirname "$marker")"); fi
done
[ -n "$slug" ] || slug=$(basename "$root" | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9]/-/g')
dir="$base/tmp/$slug"
mkdir -p "$dir/chats"
printf '%s' "$root" > "$dir/.project_root"
printf '{"projects":{"%s":"%s"}}' "$root" "$slug" > "$base/projects.json"
[ -f "$base/oauth_creds.json" ] || printf '{"access_token":"ya29.FAKE-not-a-token"}\n' > "$base/oauth_creds.json"
if [ -n "$resume" ]; then
  id=$resume
  file=
  for f in "$dir"/chats/session-*-$(printf %s "$id" | cut -c1-8).jsonl; do
    if [ -f "$f" ] && head -1 "$f" | grep -q "\"sessionId\":\"$id\""; then file=$f; fi
  done
  if [ -z "$file" ]; then echo "Error resuming session: no session $id" >&2; exit 42; fi
else
  id=@SESSION@
  short=$(printf %s "$id" | cut -c1-8)
  file="$dir/chats/session-2026-11-08T10-00-$short.jsonl"
  printf '{"sessionId":"%s","projectHash":"%s","startTime":"t","lastUpdated":"t","kind":"main"}\n' "$id" "$slug" > "$file"
  printf '{"sessionId":"%s","projectHash":"%s","startTime":"t","lastUpdated":"t","kind":"main"}\n' "@DECOY@" "$slug" > "$dir/chats/session-2026-11-07T09-00-$short.jsonl"
fi
printf '{"id":"u","timestamp":"t","type":"user","content":[{"text":"%s"}]}\n' "$prompt" >> "$file"
first=$(sed -n 2p "$file" | sed 's/.*"text":"\([^"]*\)".*/\1/')
case "$prompt" in
  *"first turn"*) answer="the first turn asked: $first" ;;
  *) answer="gemini did: $prompt" ;;
esac
printf '{"id":"g","timestamp":"t","type":"gemini","content":[{"text":"%s"}],"model":"%s"}\n' "$answer" "$model" >> "$file"
mkdir -p "$dir/$id/plans"
printf '%s\n' "$prompt" >> "$dir/$id/plans/plan.md"
printf '{"type":"init","timestamp":"t","session_id":"%s","model":"%s"}\n' "$id" "$model"
printf '{"type":"message","timestamp":"t","role":"assistant","content":"%s","delta":true}\n' "$answer"
printf '{"type":"result","timestamp":"t","status":"success"}\n'
"#;

fn install(env: &Env) -> Fallible {
    env.install(
        "gemini",
        &FAKE.replace("@SESSION@", SESSION).replace("@DECOY@", DECOY),
    )
}

/// Removes the Gemini chats from the home both nodes share, as on a
/// second machine that never ran the session; the login stays.
fn forget_chats(env: &Env) -> Fallible {
    fs::remove_dir_all(env.home.join(".gemini/tmp"))?;
    fs::remove_file(env.home.join(".gemini/projects.json"))?;
    Ok(())
}

fn chats(env: &Env) -> PathBuf {
    env.home.join(".gemini/tmp/work/chats")
}

/// The capture takes the session found by its short id and not the one
/// that shares the prefix, nor the login; the target, which never ran
/// Gemini, gets the chat and the marker its registry finds the slug by,
/// and the resumed session answers about the first turn from the chat
/// that travelled, under the same id and model, at `native_session`.
#[test]
fn e2e_a_gemini_session_found_by_its_short_id_moves_and_answers_about_its_first_turn() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    install(&p.env)?;
    let job = crate::new_job(&p.env, &p.a, "gemini")?;
    let ran = p.a.ok(&[
        "job",
        "run",
        &job,
        "--headless",
        "--prompt",
        "remember the word teal",
    ])?;
    assert!(ran.contains("gemini did: remember the word teal"), "{ran}");
    assert!(
        ran.contains(&format!("recorded gemini-cli session {SESSION}")),
        "{ran}"
    );

    let suspended = p.a.ok(&["job", "suspend", &job])?;
    assert!(
        suspended.contains(&format!("captured gemini-cli session {SESSION} (2 files)")),
        "the chat and its plan, not the decoy:\n{suspended}"
    );
    let state = p.a.state().join("jobs").join(&job).join("harness-state");
    let manifest = fs::read_to_string(state.join("manifest.json"))?;
    assert!(
        manifest.contains("session-2026-11-08T10-00-6f1d2c3b.jsonl"),
        "{manifest}"
    );
    for absent in [
        "session-2026-11-07T09-00",
        "oauth_creds",
        "projects.json",
        "ya29",
    ] {
        assert!(
            !manifest.contains(absent),
            "{absent} travelled:\n{manifest}"
        );
    }

    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    forget_chats(&p.env)?;
    let resumed = p.b.ok(&[
        "job",
        "resume",
        &job,
        "--headless",
        "--prompt",
        "what did the first turn ask?",
    ])?;
    assert!(
        resumed.contains("the first turn asked: remember the word teal"),
        "the question about the first turn was answered on the resumed side:\n{resumed}"
    );
    assert!(
        resumed.contains(&format!(
            "continuing gemini-cli session {SESSION} with model gemini-3-pro (recorded in the session)"
        )),
        "{resumed}"
    );
    let args = p.env.fake_args("gemini")?;
    assert_eq!(
        args,
        [
            "--output-format",
            "stream-json",
            "--resume",
            SESSION,
            "--model",
            "gemini-3-pro",
            "--prompt=what did the first turn ask?"
        ]
    );
    assert_eq!(
        fs::read_to_string(p.env.home.join(".gemini/tmp/work/.project_root"))?,
        p.env.work(&job).to_string_lossy()
    );
    assert!(
        !chats(&p.env)
            .join("session-2026-11-07T09-00-6f1d2c3b.jsonl")
            .exists(),
        "the session that shares the prefix travelled"
    );
    let log = p.b.log();
    assert!(
        log.contains(&format!(
            "restored gemini-cli session {SESSION} at fidelity native_session"
        )),
        "{log}"
    );
    Ok(())
}

/// A chat records no version, so the newest Gemini that wrote it travels
/// with the job: a session continued by 0.68.0 on one node is refused to
/// 0.67.0 on the other, which never starts on it.
#[test]
fn e2e_an_older_gemini_is_refused_a_session_a_newer_one_continued() -> Fallible {
    let p = crate::setup(Wire::TcpTls, None, None)?;
    install(&p.env)?;
    fs::write(p.b.state().join("gemini-version"), "0.68.0\n")?;
    let job = crate::new_job(&p.env, &p.a, "gemini")?;
    p.a.ok(&["job", "run", &job, "--headless", "--prompt", "one"])?;
    p.a.ok(&["job", "suspend", &job])?;
    p.a.ok(&["job", "move", &job, "--to", &p.b.id])?;
    p.b.ok(&["job", "resume", &job, "--headless", "--prompt", "two"])?;

    fs::write(p.env.home.join("gemini-args"), "")?;
    let back =
        p.a.run(&["job", "resume", &job, "--headless", "--prompt", "three"])?;
    let said = text(&back);
    assert!(!back.status.success(), "{said}");
    assert!(
        said.contains(&format!(
            "gemini 0.67.0 is older than gemini 0.68.0, which wrote session {SESSION}"
        )),
        "{said}"
    );
    let args = fs::read_to_string(p.env.home.join("gemini-args"))?;
    assert!(!args.contains("three"), "the older gemini started: {args}");
    Ok(())
}
