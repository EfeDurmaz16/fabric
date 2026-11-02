# Legacy state fixture, commit 176622e

A state directory made by the Fabric binary of commit 176622e (main before
managed jobs), with `e2e_make_legacy_fixture` in
`crates/fabric/tests/e2e/legacy.rs`: one job under the fake ACP agent, one
turn, work of every kind in its root, an approval, then a suspend. The
daemon was stopped with `SIGKILL`, as a crash would stop it, so the store's
catalog still has its WAL.

- `state/`: the state directory, without `node.key` (a private key is never
  committed), the daemon's log, lock, socket and client environment.
- `fake-agent/`: the fake agent's session files from the shared home.
- `job`: the job's id.

Never edit these files. A test that needs other legacy data adds a new
fixture beside this one.
