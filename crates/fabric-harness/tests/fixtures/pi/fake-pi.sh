#!/bin/sh
[ -z "$FABRIC_TOOL_WARM" ] || exit 0
# A fake pi 0.80.3 for the adapter's tests. It keeps pi's agent directory,
# ${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}, with an auth.json that holds a
# fake key and must never travel, and one v3 JSONL file per session under
# sessions/--<cwd with / as ->--/<timestamp>_<uuid>.jsonl, as 0.80.3 does.
# With -p --mode json it prints the session header and a message_end of
# the answer. --session <path> continues that file; without it a new
# session is made. A prompt that asks for the first turn is answered from
# the file, so an answer proves the transcript travelled.
version=0.80.3
[ -z "$FABRIC_HOME" ] || [ ! -f "$FABRIC_HOME/pi-version" ] || version=$(cat "$FABRIC_HOME/pi-version")
if [ "$1" = --version ]; then echo "$version"; exit 0; fi
printf '%s\n' "$@" > "$HOME/pi-args"
env > "$HOME/pi-env"
agent="${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}"
mkdir -p "$agent"
[ -f "$agent/auth.json" ] || { printf '{"anthropic":{"type":"api_key","key":"sk-ant-FAKEpiAUTH0123456789abcdef"}}\n' > "$agent/auth.json"; chmod 600 "$agent/auth.json"; }
file= provider= model= thinking= prompt= print=
while [ $# -gt 0 ]; do
  case "$1" in
    -p) print=1 ;;
    --mode) shift ;;
    --session) file=$2; shift ;;
    --provider) provider=$2; shift ;;
    --model) model=$2; shift ;;
    --thinking) thinking=$2; shift ;;
    *) prompt=$1 ;;
  esac
  shift
done
[ -n "$print" ] || exit 0
cwd=$(pwd -P)
if [ -n "$file" ]; then
  [ -f "$file" ] || { echo "No session found matching '$file'" >&2; exit 1; }
  id=$(sed -n '1s/.*"id":"\([^"]*\)".*/\1/p' "$file")
else
  dir="$agent/sessions/--$(printf '%s' "$cwd" | sed 's#^/##; s#[/\\:]#-#g')--"
  mkdir -p "$dir"
  n=$(ls "$dir" | wc -l | tr -d ' ')
  id=0199fab0-0000-7000-8000-00000000000$((n + 1))
  file="$dir/2026-11-08T10-00-0$n-000Z_$id.jsonl"
  printf '{"type":"session","version":3,"id":"%s","timestamp":"2026-11-08T10:00:00.000Z","cwd":"%s"}\n' "$id" "$cwd" > "$file"
  [ -n "$provider" ] || provider=anthropic
  [ -n "$model" ] || model=claude-fake-5
  [ -n "$thinking" ] || thinking=medium
fi
[ -z "$model" ] || printf '{"type":"model_change","id":"m1","parentId":null,"timestamp":"t","provider":"%s","modelId":"%s"}\n' "$provider" "$model" >> "$file"
[ -z "$thinking" ] || printf '{"type":"thinking_level_change","id":"t1","parentId":null,"timestamp":"t","thinkingLevel":"%s"}\n' "$thinking" >> "$file"
first=$(sed -n 's/.*"role":"user","content":"\([^"]*\)".*/\1/p' "$file" | head -n 1)
case "$prompt" in
  *"first turn"*) answer="the first turn said: $first" ;;
  *) answer="pi did: $prompt" ;;
esac
printf '{"type":"message","id":"u1","parentId":null,"timestamp":"t","message":{"role":"user","content":"%s","timestamp":1}}\n' "$prompt" >> "$file"
printf '{"type":"message","id":"r1","parentId":"u1","timestamp":"t","message":{"role":"assistant","content":[{"type":"thinking","thinking":"t","thinkingSignature":"c2ln"},{"type":"text","text":"%s"}],"provider":"anthropic","model":"claude-fake-5","stopReason":"stop","timestamp":2}}\n' "$answer" >> "$file"
head -n 1 "$file"
echo '{"type":"agent_start"}'
printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"t"},{"type":"text","text":"%s"}],"stopReason":"stop"}}\n' "$answer"
echo '{"type":"agent_end","messages":[]}'
