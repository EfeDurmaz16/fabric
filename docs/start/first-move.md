# First move

This is the walk a fresh checkout of `main` was taken through with Codex on one Mac on 2026-11-03, a real turn and its resume. On 2026-11-08, after M4 merged, the same commands ran again under the fake `codex` of the end-to-end tests, so no real turn ran that day; it checked that the merge broke none of them. [The evidence](../evidence/usable-from-main/walk.md) has every command and what each run printed. It needs a node running (see [Install](install.md)) and Codex logged in, as `fabric doctor` shows. Claude Code takes the same commands with `--harness claude`; that walk is not recorded on `main`.

## On one Mac

```sh
fabric job new --repo <a git repository> --goal "say hello and name one file" --harness codex
fabric job run <job> --headless --prompt "Say hello and name one file here."
fabric job suspend <job>
fabric job resume <job> --headless --prompt "Which file did you name before?"
fabric job status <job>
fabric job log <job>
```

`job new` clones the repository into `~/fabric/jobs/<job>/work`; it does not adopt your checkout in place (`fabric job init` is not built). The headless turn prints the answer, then a last line naming the session (`codex session <thread>` or `claude-code session <id>`). The resume continues that session, and its `continuing ... session` line names the model, effort and sandbox it carried.

Drop `--headless --prompt ...` to run the harness in your terminal instead; exit it before you suspend, or let `suspend` stop it.

## To a second Mac

Both Macs need the same user name and home path. Start `fabric serve` on each from the desktop session, not over SSH, so the node can read the Claude Code login in the Keychain. Pair them, then:

```sh
fabric job suspend <job>                    # on the first Mac
fabric job move <job> --to fab1<other-id>   # the other Mac owns the job from here
fabric job resume <job>                     # on the other Mac
```

`move` prints what it carries, rebuilds or leaves for every ignored path before anything changes hands. To bring the job back, run `fabric job resume <job>` on the first Mac; it pulls the job. The whole walk, pairing included, is the [two Macs guide](../guides/two-macs.md).

## What you should see

- On the target, `git status` and both diffs equal the source's, and the caches the matrix carries are there.
- `.env` is not.
- The harness answers a question about the turn it took on the other machine.
- On the source, `fabric job status <job>` says another node owns the job, and `fabric job run <job>` there exits 81.
