#!/bin/sh
# A fake opencode 1.18.34 for the adapter's tests. It keeps OpenCode's
# data directory, ${XDG_DATA_HOME:-$HOME/.local/share}/opencode, with an
# opencode.db that holds a fake login token and an auth.json, which must
# never travel, and keeps its session rows as text beside them under
# fake-rows/<id>.info (the session row as JSON) and fake-rows/<id>.msgs (one
# message with its parts per line). It answers export, import and run as
# 1.18.34 does: export prints the JSON with "Exporting session: <id>" on
# its error output, or "Error: Session not found: <id>" and exit 1; import
# adds the rows the store lacks and prints "Imported session: <id>", a new
# id under FAKE_OPENCODE_NEW_ID; run --format json prints step_start, text
# and step_finish events. A prompt that asks for the first turn is
# answered from the store, so an answer proves the transcript travelled.
version=1.18.34
[ -z "$FAKE_OPENCODE_VERSION" ] || version=$FAKE_OPENCODE_VERSION
[ -z "$FABRIC_HOME" ] || [ ! -f "$FABRIC_HOME/opencode-version" ] || version=$(cat "$FABRIC_HOME/opencode-version")
if [ "$1" = --version ]; then echo "$version"; exit 0; fi
data="${XDG_DATA_HOME:-$HOME/.local/share}/opencode"
rows="$data/fake-rows"
mkdir -p "$rows"
[ -f "$data/opencode.db" ] || printf 'SQLite format 3\naccount|acc_1|access_token|sk-ant-FAKEdatabaseTOKEN0123456789\n' > "$data/opencode.db"
[ -f "$data/auth.json" ] || { printf '{"anthropic":{"type":"api","key":"sk-ant-FAKEauthJSON0123456789abcd"}}\n' > "$data/auth.json"; chmod 600 "$data/auth.json"; }
case "$1" in
  export)
    id=$2
    if [ ! -f "$rows/$id.info" ]; then echo "Error: Session not found: $id" >&2; exit 1; fi
    echo "Exporting session: $id" >&2
    printf '{"info":%s,\n"messages":[\n' "$(cat "$rows/$id.info")"
    sed '$!s/$/,/' "$rows/$id.msgs"
    printf ']}\n'
    exit 0 ;;
  import)
    file=$2
    id=$(sed -n '1s/^{"info":{"id":"\([^"]*\)".*/\1/p' "$file")
    [ -n "$id" ] || { echo "Error: not an export" >&2; exit 1; }
    new=$id
    [ ! -f "$data/fake-new-id" ] || new=ses_0fabric0minted0by0import1
    [ -f "$rows/$new.info" ] || sed -n '1s/^{"info":\(.*\),$/\1/p' "$file" | sed "s/$id/$new/g" > "$rows/$new.info"
    touch "$rows/$new.msgs"
    sed '1,2d;$d' "$file" | sed 's/,$//' | sed "s/$id/$new/g" | while IFS= read -r line; do
      grep -qxF -e "$line" "$rows/$new.msgs" || printf '%s\n' "$line" >> "$rows/$new.msgs"
    done
    echo "Imported session: $new"
    exit 0 ;;
esac
printf '%s\n' "$@" > "$HOME/opencode-args"
env > "$HOME/opencode-env"
[ "$1" = run ] || exit 0
shift
session= model= agent= variant= prompt=
while [ $# -gt 0 ]; do
  case "$1" in
    -s) session=$2; shift ;;
    -m) model=$2; shift ;;
    --agent) agent=$2; shift ;;
    --variant) variant=$2; shift ;;
    --format) shift ;;
    --) prompt=$2; shift ;;
  esac
  shift
done
[ -n "$model" ] || model=anthropic/claude-fake-5
[ -n "$agent" ] || agent=build
[ -n "$variant" ] || variant=high
provider=${model%%/*}
name=${model#*/}
if [ -z "$session" ]; then
  session=ses_0fabric0opencode0session001
  printf '{"id":"%s","slug":"quiet-otter","projectID":"global","directory":"%s","title":"t","agent":"%s","model":{"id":"%s","providerID":"%s","variant":"%s"},"version":"%s","time":{"created":1,"updated":1}}\n' \
    "$session" "$(pwd -P)" "$agent" "$name" "$provider" "$variant" "$version" > "$rows/$session.info"
  : > "$rows/$session.msgs"
elif [ ! -f "$rows/$session.info" ]; then
  printf '{"type":"error","timestamp":1,"sessionID":"%s","error":{"name":"NotFound","data":{"message":"Session not found"}}}\n' "$session"
  exit 1
fi
n=$(wc -l < "$rows/$session.msgs" | tr -d ' ')
first=$(sed -n '1s/.*"text":"\([^"]*\)".*/\1/p' "$rows/$session.msgs")
case "$prompt" in
  *"first turn"*) answer="the first turn said: $first" ;;
  *) answer="opencode did: $prompt" ;;
esac
printf '{"info":{"role":"user","agent":"%s","model":{"providerID":"%s","modelID":"%s"},"id":"msg_u%s","sessionID":"%s"},"parts":[{"type":"text","text":"%s","id":"prt_u%s"}]}\n' \
  "$agent" "$provider" "$name" "$n" "$session" "$prompt" "$n" >> "$rows/$session.msgs"
printf '{"info":{"role":"assistant","agent":"%s","providerID":"%s","modelID":"%s","id":"msg_a%s","sessionID":"%s"},"parts":[{"type":"text","text":"%s","id":"prt_a%s"}]}\n' \
  "$agent" "$provider" "$name" "$n" "$session" "$answer" "$n" >> "$rows/$session.msgs"
printf '{"type":"step_start","timestamp":1,"sessionID":"%s","part":{"type":"step-start"}}\n' "$session"
printf '{"type":"text","timestamp":2,"sessionID":"%s","part":{"type":"text","text":"%s"}}\n' "$session" "$answer"
printf '{"type":"step_finish","timestamp":3,"sessionID":"%s","part":{"type":"step-finish","reason":"stop"}}\n' "$session"
