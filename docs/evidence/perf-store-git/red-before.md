# Each test against the code before its fix

Each regression test of this branch ran against the tree before its own
fix, and failed there; the output below is what it printed, with the
machine's temporary paths cut to `<tmp>`. The tests that log git run in a
child process with a `git` first on the `PATH` that appends each call to a
file (`common::run_with_logged_git`).

## Item 8: `git bundle unbundle` without a separate verify

`a_restore_runs_bundle_unbundle_alone`, on `8c28b1a`:

```
assertion `left == right` failed
  left: ["verify", "unbundle", "verify", "unbundle"]
 right: ["unbundle", "unbundle"]
```

`a_link_whose_prerequisites_the_chain_lacks_fails_the_restore` is the guard
that the check stays: on `8c28b1a` the same link failed in `git bundle
verify` ("Repository lacks these prerequisite commits"), after the fix in
`git bundle unbundle` with the same message.

## Item 18: the exclude file and the config without git

`a_restore_writes_its_exclude_file_and_config_without_git`, on `26a9475`;
the restore's calls included:

```
rev-parse --git-path info/exclude
config --local --replace-all remote.origin.url https://example.com/x.git
config --local --replace-all remote.origin.fetch +refs/heads/*:refs/remotes/origin/*
config --local --replace-all branch.main.remote origin
config --local --replace-all branch.main.merge refs/heads/main
config --local --replace-all push.default simple
```

While the writer was built, its property test, with git 2.54 as the
oracle, found that git quotes a value holding a carriage return (the fix
of CVE-2025-48384); the writer does the same, and
`a_carriage_return_in_a_config_value_comes_back` holds it on any git.

## Item 5: symbolic refs and `HEAD` in the ref transaction

`a_restore_sets_symbolic_refs_and_head_in_its_ref_transaction`, on
`26ec80b`:

```
assertion `left == right` failed
  left: ["update-ref --stdin", "symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main", "symbolic-ref HEAD refs/heads/main"]
 right: ["update-ref --no-deref --stdin"]
```

`a_git_without_symref_update_sets_them_one_by_one_and_is_not_asked_again`,
on `26ec80b`, with a `git` that refuses `symref-update`:

```
assertion `left == right` failed
  left: ["update-ref --stdin", "symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main", "symbolic-ref HEAD refs/heads/main"]
 right: ["update-ref --no-deref --stdin", "update-ref --stdin", "symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main", "symbolic-ref HEAD refs/heads/main"]
```

An experiment on git 2.54 first showed why the transaction needs
`--no-deref`: without it, `symref-update HEAD refs/heads/main` followed the
`HEAD` of `git init` and made `refs/heads/master` a symbolic ref. With it,
the refs and every reflog came out byte for byte as with `symbolic-ref`,
for a branch and for a detached `HEAD`; both cases used `main`.

The first test above is `a_restore_sets_head_and_symbolic_refs_without_a_process_each`
since `c0ece10`, which takes `HEAD` out of the transaction:
`a_repository_with_the_branch_git_init_names_restores`, on `228a6cd`
(the code of `a4a72e4`), failed at its first layout, a repository on
`master`:

```
scenario 0 ["checkout -q master"]: Err(Git(Failed { command: "update-ref --no-deref --stdin", code: Some(128), stderr: "fatal: multiple updates for 'HEAD' (including one via its referent 'refs/heads/master') are not allowed" }))
```

git refuses one transaction that updates `HEAD` and the branch `HEAD`
points at, and a restore's `git init` points `HEAD` at `master`, so every
snapshot with a `master` branch failed to restore on git 2.46 and later,
whatever its `HEAD` (checked on git 2.54 by hand for `master`, `main` with
a `master` branch, and a detached `HEAD` with a `master` branch). On
`c0ece10`, whose `git init` takes `--initial-branch`, all seven layouts of
the test restore.

## Item 10: the bundle's heads without `list-heads`

`a_capture_reads_the_bundle_heads_without_git`, on `a4a72e4`:

```
assertion `left == right` failed
  left: ["create", "verify", "list-heads", "create", "verify", "list-heads"]
 right: ["create", "verify", "create", "verify"]
```

## Item 9: a resume flat in the bundle chain

`a_resume_unpacks_only_the_links_the_node_lacks`, on `b961153` (the test
alone, without the tests that read the new `bundles_unpacked` field): after
a restore of the third link, a restore of the fourth unpacked the whole
chain again.

```
assertion `left == right` failed
  left: 4
 right: 1
```

`a_pack_rewritten_in_place_in_one_restore_leaves_the_cache_and_the_others_alone`,
on `02b7dc4` with the hard-link branch of `bundle_cache::place`, the one
every platform but macOS took, forced on macOS: the first restore's pack
files, the cached ones and the second restore's were one inode.

```
assertion `left == right` failed: <tmp>/first/.git/objects/pack/pack-67298ca598be56f9d65b1ff1eae1d0ccd8c71ede.idx
  left: 3
 right: 1
```

With that check taken out, the rewrite of the second restore's packs in
place broke the first restore: its `git fsck --no-dangling` failed
(`workspace.rs`, the `fsck` of `first`). On `7939faa`, which copies, the
test passes.
