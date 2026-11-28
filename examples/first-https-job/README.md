# Run your first Fabric job over HTTPS

Start a disposable Fabric node, run one ACP turn through the CLI, then read
the result from a separate Node.js application. The repository's
`fabric-fake-agent` executable answers the turn locally. Fabric performs real
placement, launches that process and journals its receipt. The fixture needs
no model credentials or cloud account.

For a team, the operator starts the authority and gateway, then gives callers
the endpoint, CA and scoped grants. An application only installs its chosen
client. This walkthrough plays both roles with isolated state.

Use a fresh Bash shell on macOS or Linux with Python 3.10+, uv, jq, OpenSSL
with `req -addext`, Node.js 22+ and npm. Allow local sockets and loopback
listening. For a source build, also install Git, Cargo with the repository's
pinned Rust toolchain, and Bun. Run the blocks in order in the same shell.

## Install the preview into an empty directory

With artifacts from a trusted build of this revision, set `FABRIC_PREVIEW_BIN`
to the absolute directory containing executable `fabric` and `fabric-fake-agent`,
and `FABRIC_PREVIEW_PACKAGES` to the absolute directory containing
`fabric_jobs-0.1.0-py3-none-any.whl` and `fabric-jobs-client-0.1.0.tgz`.
You can start anywhere. To build them yourself instead, leave both variables
unset and start at the root of the Fabric checkout.

```bash
set -euo pipefail
umask 077
work=$(mktemp -d /tmp/fabric-first.XXXXXX)
if [ -n "${FABRIC_PREVIEW_BIN:-}" ]; then
  bin="$FABRIC_PREVIEW_BIN"
  packages="${FABRIC_PREVIEW_PACKAGES:?set the preview packages directory}"
else
  repo=$(git rev-parse --show-toplevel)
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}"
  cargo build --locked -p fabric --features fixtures --bin fabric --bin fabric-fake-agent
  bin="$CARGO_TARGET_DIR/debug"
  packages="$work/packages"
  mkdir "$packages"
  uv build --wheel --out-dir "$packages" "$repo/sdk/python"
  (cd "$repo/sdk/typescript" && bun install --frozen-lockfile && npm pack --pack-destination "$packages")
fi
cd "$work"
mkdir consumer
uv venv --python '>=3.10' venv
uv pip install --python venv/bin/python "$packages"/fabric_jobs-0.1.0-py3-none-any.whl
(cd consumer && npm init -y >/dev/null && npm install --no-audit --no-fund "$packages/fabric-jobs-client-0.1.0.tgz")
```

## Start the authority and gateway

Both listeners pick an available loopback port. The node runs with a fresh
home and private state. This example's one-day certificate trusts only the
loopback endpoint when supplied to a client.

```bash
export FABRIC_HOME="$work/state"
mkdir "$FABRIC_HOME" home
printf '[acp.fabric-fake-agent]\nstate_dir = ".fake-agent"\n' > "$FABRIC_HOME/harness.toml"
env -i HOME="$work/home" PATH="$bin:/usr/bin:/bin" FABRIC_HOME="$FABRIC_HOME" \
  FAKE_STATE_DIR="$work/home/.fake-agent" "$bin/fabric" --json serve \
  --listen 127.0.0.1:0 --wire tcp-tls > node.json 2>node.log &
node_pid=$!
trap 'kill "$node_pid" 2>/dev/null || :; if [ -n "${gw_pid:-}" ]; then kill "$gw_pid" 2>/dev/null || :; fi' EXIT
wait_json() {
  for ((i=0; i<150; i++)); do
    if jq -es 'length > 0' "$1" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  echo "Timed out waiting for $1 in $work" >&2
  return 1
}
wait_json node.json
authority=$(jq -er '.node.node' node.json)
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=Fabric local demo' \
  -addext 'subjectAltName=IP:127.0.0.1' -keyout key.pem -out ca.pem
"$bin/fabric" gateway --authority "$authority" --listen 127.0.0.1:0 \
  --cert ca.pem --key key.pem >gateway.json &
gw_pid=$!
wait_json gateway.json
endpoint=$(jq -er .endpoint gateway.json)
```

## Submit and watch from the CLI

The owner mints a five-minute submit grant. Once the authority returns the
job ID, the owner issues an observer grant restricted to that job.

```bash
mint() { "$bin/fabric" --json facade mint --ttl-ms 300000 "$@" | jq -er '.token.bearer'; }
mint --actor first-job --node "$authority" --allow submit > submit.token
jq -n --arg pin "$authority" '{goal:"Say hello", harness:{kind:"acp",program:"fabric-fake-agent",args:[]}, source:{kind:"empty"}, policy:{pin:$pin}}' > job.json
remote() { "$work/venv/bin/fbrc" --endpoint "$endpoint" --ca-file "$work/ca.pem" "$@"; }
remote submit --key first-turn --spec job.json < submit.token | tee accepted.json
job=$(jq -er .job accepted.json)
mint --actor observer --job "$job" --allow inspect,watch > read.token
cursor=
status=
for ((i=0; i<30; i++)); do
  if [ -n "$cursor" ]; then
    remote watch "$job" --after "$cursor" --wait-ms 1000 <read.token >page.json
  else
    remote watch "$job" --wait-ms 1000 <read.token >page.json
  fi
  jq -c '.events[] | {seq,kind,status:.data.status}' page.json
  if next=$(jq -er '.next // empty' page.json); then cursor="$next"; fi
  status=$(jq -r '[.events[] | select(.kind == "attempt_observed") | .data.status] | last // empty' page.json)
  case "$status" in succeeded|failed|cancelled) break;; esac
  if jq -e '.closed != null' page.json >/dev/null; then break; fi
done
remote inspect "$job" <read.token >inspection.json
jq '{job:.job_id,state,attempt:.data.job.attempts[-1].status}' inspection.json
test "$status" = succeeded
```

The submit response includes a new job ID and `"replayed":false`. Retrying the
same spec with `first-turn` returns the original ID. The inspection shows
`{"job":"<job ID>","state":"active","attempt":"succeeded"}`.
A managed job can remain active after a successful turn. An empty watch
page means the wait elapsed. Use the attempt status to detect this turn's
outcome.

## Read the same job from Node.js

```bash
cd "$work/consumer"
cat > inspect.mjs <<'JS'
import { readFileSync } from "node:fs";
import { FabricJobs, JobId } from "@fabric-jobs/client";

const [endpoint, caFile, jobId] = process.argv.slice(2);
const jobs = new FabricJobs({
  endpoint,
  ca: readFileSync(caFile, "utf8"),
  token: readFileSync(0, "utf8").trim(),
});
const { job } = await jobs.inspect(JobId.parse(jobId));
console.log(JSON.stringify({ state: job.state, attempt: job.attempts.at(-1)?.status }));
JS
node inspect.mjs "$endpoint" "$work/ca.pem" "$job" < "$work/read.token"
```

Expected output is `{"state":"active","attempt":"succeeded"}`. Exit the
disposable shell to stop the node and gateway. The private demo directory at
`$work` contains the local node state, certificate key and expiring grants.
Delete it after inspecting your results.
