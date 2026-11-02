# The flush and memory tests, red on the old code

Each test below was run on 2026-10-28 against the head of this branch with
one change put back as it was before its fix, in the same tree, and
failed; with the fix it passes (`cargo test -p fabricd -p fabric-harness`).
Temporary directories are shown as `<tmp>` and the pid as `<pid>`. `Full`
is `F_FULLFSYNC` on macOS, `Barrier` is `F_BARRIERFSYNC` and `Plain` is
`fsync`; on Linux all three are `fsync`, and the tests check the kind of
flush asked for, so they hold on both.

## Item 3, fabricd: `durable::tests`

The temporary flushed `Full` before the rename, as before `41526fc`:

```text
test durable::tests::an_ordered_replace_has_the_barrier_and_no_full_flush ... FAILED
test durable::tests::an_atomic_write_orders_with_a_barrier_and_flushes_the_drive_once ... FAILED
  left: [(Full, "<tmp>/manifest.<pid>.tmp")]
 right: [(Barrier, "<tmp>/manifest.<pid>.tmp")]
  left: [(Full, "<tmp>/marker.<pid>.tmp"), (Full, "<tmp>")]
 right: [(Barrier, "<tmp>/marker.<pid>.tmp"), (Full, "<tmp>")]
test result: FAILED. 2 passed; 2 failed
```

## Item 12, fabricd: `ledger_file::tests`

`write_manifest` through `write_atomic`, as before `9f47c43` (item 3 in):

```text
test ledger_file::tests::a_commit_flushes_the_drive_once_and_orders_its_manifest ... FAILED
  left: [(Full, "<tmp>/ledger"), (Barrier, "<tmp>/manifest.<pid>.tmp"), (Full, "<tmp>")]
 right: [(Full, "<tmp>/ledger"), (Barrier, "<tmp>/manifest.<pid>.tmp")]
test result: FAILED. 0 passed; 1 failed
```

Before item 3 the left side was three full flushes.

## Ledger memory, fabricd: `ledger_file::tests`

The ledger file keeping a copy of every record it scanned, created or
appended, as before `3904a60`:

```text
test ledger_file::tests::an_open_ledger_hands_its_records_over_and_keeps_no_copy ... FAILED
  left: 3
 right: 0
test result: FAILED. 0 passed; 1 failed
```

## Item 3, fabric-harness: `durable::tests`

Every flush asked for as `Full`, as `sync_all` is on macOS, before
`923aac7`:

```text
test durable::tests::an_atomic_write_orders_with_a_barrier_and_flushes_the_drive_once ... FAILED
  left: [(Full, ""), (Full, "a"), (Full, "tmp:a/b"), (Full, "a/b")]
 right: [(Plain, ""), (Plain, "a"), (Barrier, "tmp:a/b"), (Full, "a/b")]
test result: FAILED. 0 passed; 1 failed
```

## Item 16, fabric-harness: `pipeline::tests`

Staging as before `3771de8` (item 3 in): every staged file a durable
atomic write with its own full flush, and the old manifest's removal a
full flush:

```text
test pipeline::tests::a_capture_flushes_the_drive_once_at_its_manifest ... FAILED
test pipeline::tests::a_capture_into_a_used_directory_orders_the_old_manifest_away_first ... FAILED
  left: [(Plain, "out"), (Plain, "out/home"), (Barrier, "tmp:out/home/x"), (Full, "out/home/x"),
         (Plain, "out/home"), (Barrier, "tmp:out/home/y"), (Full, "out/home/y"),
         (Barrier, "tmp:out"), (Full, "out")]
 right: [(Plain, "out"), (Plain, "out/home"), (Plain, "tmp:out/home/x"), (Plain, "out/home/x"),
         (Plain, "out/home"), (Plain, "tmp:out/home/y"), (Plain, "out/home/y"),
         (Barrier, "tmp:out"), (Full, "out")]
  left: [(Full, "out"), (Barrier, "tmp:out/home/x"), (Full, "out/home/x"), (Plain, "out/home"),
         (Barrier, "tmp:out"), (Full, "out")]
 right: [(Barrier, "out"), (Plain, "tmp:out/home/x"), (Plain, "out/home/x"), (Plain, "out/home"),
         (Barrier, "tmp:out"), (Full, "out")]
test result: FAILED. 0 passed; 2 failed
```

## Restore placement, fabric-harness: `place::tests`

`place_all` as before `90d79de`: each file its own atomic write with a full
flush of its directory. A restore of one file makes the same two flushes
either way, so its test passes on both and guards that case:

```text
test place::tests::a_restore_of_one_file_flushes_as_an_atomic_write_does ... ok
test place::tests::a_restore_stopped_part_way_flushes_what_it_placed ... FAILED
test place::tests::a_restore_orders_each_file_and_flushes_the_drive_once ... FAILED
  left: [(Barrier, "tmp:"), (Full, ""), (Barrier, "tmp:")]
 right: [(Barrier, "tmp:"), (Barrier, "tmp:"), (Full, "")]
  left: [(Barrier, "tmp:home/p"), (Full, "home/p"), (Barrier, "tmp:home/p"), (Full, "home/p"),
         (Plain, "home/p"), (Barrier, "tmp:home/p/q"), (Full, "home/p/q")]
 right: [(Barrier, "tmp:home/p"), (Barrier, "tmp:home/p"), (Plain, "home/p"),
         (Barrier, "tmp:home/p/q"), (Plain, "home/p"), (Full, "home/p/q")]
test result: FAILED. 1 passed; 2 failed
```
