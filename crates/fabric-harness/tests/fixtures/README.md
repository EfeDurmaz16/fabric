# Adapter fixtures

One directory per adapter of PR 5 (`opencode/`, `pi/`, `gemini/`, `cursor/`), each holding the agent's real on-disk layout with synthetic content, which the adapter's roundtrip test (`tests/<adapter>_roundtrip.rs`) and its end-to-end fake copy into a temporary home. The contract is `docs/harness-adapters.md`; these are the rules a fixture keeps.

- Synthetic only. Nothing here was copied from a real session. The layout (directory names, file names, table and column names, JSON key names) is the agent's; the values are made up.
- Every secret is a fake value the standard redactor matches (`sk-ant-FAKE` followed by enough characters, a fake `ghp_` token, a fake PEM block), so the refusal tests prove the capture refuses it and the roundtrip tests prove it never reaches the snapshot. A fixture never holds a value that could be a real credential.
- No path from a real machine. A fixture is written with a placeholder home that the test replaces with its temporary home at run time, as the Codex roundtrip builds its `session_meta` from the test's own workspace; a layout keyed by the workspace path (pi's `--<path>--` directory, Cursor's md5 of the path) is derived by the test from that temporary path, never stored.
- `VERSION` beside the files names the agent version and the date the layout was read on, so a schema change in the agent is a fixture change with a date.
- A fixture that stands for a database (OpenCode's `opencode.db`, Cursor's `store.db`) is a real SQLite file only when a test needs to open it; otherwise a file with the right name is enough, because the adapters never read those files and the denylist refuses them by suffix.
