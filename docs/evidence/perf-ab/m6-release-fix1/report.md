### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 147.1 | 76.3 | 0.478 | 0.460 to 0.541 | faster |
| acp launch | 20 | 74.2 | 31.5 | 0.424 | 0.406 to 0.446 | faster |
| acp approve | 20 | 15.9 | 8.5 | 0.528 | 0.484 to 0.548 | faster |
| claude new | 20 | 144.3 | 65.4 | 0.446 | 0.430 to 0.492 | faster |
| claude launch | 20 | 94.7 | 90.8 | 0.970 | 0.943 to 0.993 | faster |
| claude approve | 20 | 16.6 | 8.4 | 0.519 | 0.492 to 0.543 | faster |
| codex new | 20 | 144.4 | 65.5 | 0.445 | 0.406 to 0.534 | faster |
| codex launch | 20 | 106.9 | 96.4 | 0.921 | 0.892 to 0.959 | faster |
| codex approve | 20 | 15.8 | 8.4 | 0.543 | 0.504 to 0.550 | faster |
| ledger append | 20 | 16.0 | 8.2 | 0.526 | 0.500 to 0.535 | faster |
| cli version | 20 | 3.0 | 3.0 | 1.024 | 0.877 to 1.073 | not shown, median above 1 |
| cli ls | 20 | 4.2 | 3.9 | 0.881 | 0.789 to 0.956 | faster |
| acp suspend | 20 | 462.4 | 369.2 | 0.774 | 0.730 to 0.870 | faster |
| acp move | 20 | 181.5 | 105.7 | 0.597 | 0.578 to 0.633 | faster |
| acp resume | 20 | 824.2 | 300.0 | 0.356 | 0.320 to 0.441 | faster |
| acp pull | 20 | 1449.7 | 760.0 | 0.540 | 0.480 to 0.604 | faster |
| claude suspend | 20 | 547.0 | 378.1 | 0.708 | 0.675 to 0.771 | faster |
| claude move | 20 | 175.0 | 107.6 | 0.617 | 0.578 to 0.643 | faster |
| claude resume | 20 | 973.9 | 400.5 | 0.419 | 0.370 to 0.515 | faster |
| claude pull | 20 | 1638.8 | 846.3 | 0.532 | 0.479 to 0.583 | faster |
| codex suspend | 20 | 787.4 | 661.8 | 0.835 | 0.802 to 0.865 | faster |
| codex move | 20 | 173.2 | 107.4 | 0.619 | 0.609 to 0.652 | faster |
| codex resume | 20 | 1260.5 | 618.1 | 0.541 | 0.484 to 0.590 | faster |
| codex pull | 20 | 2426.2 | 1403.3 | 0.616 | 0.577 to 0.686 | faster |
| cold start | 20 | 61.6 | 57.4 | 0.918 | 0.887 to 0.938 | faster |
| restart | 20 | 448.9 | 66.9 | 0.192 | 0.133 to 0.223 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38909 -> 38218 | 8080 -> 7264 | 9.8 -> 7.4 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1184 -> 295 | 4104 -> 4563 | 6960 -> 6416 | 10.2 -> 9.3 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1160 -> 1137 | 5088 -> 4592 | 8.2 -> 6.7 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1636 -> 1413 | 5120 -> 4656 | 8.4 -> 6.4 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1103 -> 786 | 3504 -> 3592 | 6992 -> 6064 | 10.1 -> 8.7 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1171 -> 1136 | 5088 -> 4576 | 8.5 -> 6.2 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1634 -> 1411 | 5120 -> 4656 | 8.6 -> 6.4 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3820 -> 3974 | 7056 -> 6192 | 10.4 -> 7.7 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1160 -> 1136 | 5056 -> 4560 | 8.1 -> 6.6 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1162 -> 1137 | 5104 -> 4576 | 8.4 -> 5.7 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 10 | 3168 -> 2320 | 5.9 -> 5.5 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1507 -> 1481 | 5200 -> 4688 | 8.6 -> 6.0 |
| acp suspend | 36 -> 35 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 40054 -> 40989 | 14968 -> 12635 | 5280 -> 4752 | 8.2 -> 5.9 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5212 -> 2459 | 8302 -> 5085 | 5600 -> 5040 | 8.2 -> 8.8 |
| acp resume | 32 -> 14 | 24 -> 13 | 51 -> 61 | 167 -> 167 | 16060 -> 21704 | 11161 -> 8670 | 7056 -> 6496 | 10.6 -> 7.7 |
| acp pull | 111 -> 56 | 81 -> 41 | 59 -> 77 | 1376 -> 1376 | 33255 -> 39739 | 30344 -> 21666 | 7136 -> 6640 | 9.8 -> 9.5 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13274 -> 14162 | 19190 -> 16196 | 5312 -> 4768 | 8.8 -> 6.8 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5265 -> 2568 | 8198 -> 5289 | 5616 -> 5040 | 8.7 -> 6.1 |
| claude resume | 34 -> 16 | 24 -> 13 | 55 -> 65 | 148 -> 148 | 21662 -> 27029 | 11059 -> 8634 | 7312 -> 6544 | 10.5 -> 8.6 |
| claude pull | 114 -> 61 | 83 -> 41 | 63 -> 83 | 1338 -> 1338 | 41631 -> 47544 | 36721 -> 27502 | 7424 -> 6656 | 13.0 -> 8.9 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 14405 -> 15435 | 16227 -> 12408 | 5248 -> 4752 | 9.0 -> 6.5 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5328 -> 2642 | 8212 -> 5304 | 5616 -> 5056 | 8.9 -> 6.5 |
| codex resume | 35 -> 17 | 24 -> 13 | 59 -> 69 | 148 -> 148 | 25762 -> 31029 | 11192 -> 8743 | 7392 -> 6528 | 11.2 -> 8.5 |
| codex pull | 117 -> 64 | 82 -> 40 | 67 -> 88 | 1338 -> 1338 | 47005 -> 52753 | 33060 -> 22982 | 7440 -> 6672 | 11.7 -> 9.7 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5754 -> 5877 | 5120 -> 4528 | 36.9 -> 25.7 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 0 | 463 -> 463 | 501625 -> 498479 | 16190 -> 10991 | 5296 -> 4688 | 72.8 -> 25.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 9846 -> 7432, cli_maxrss_kib 8080 -> 7264, cmd:defaults 2 -> 0, frees 28092 -> 27541, fullfsync 8 -> 3, malloc_bytes 13531066 -> 13498818, mallocs 38909 -> 38218, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 10186 -> 9296, cli_maxrss_kib 6960 -> 6416, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3345 -> 3793, fullfsync 15 -> 5, malloc_bytes 711125 -> 879288, mallocs 4104 -> 4563, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1010 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1032 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 8237 -> 6691, cli_maxrss_kib 5088 -> 4592, frees 1009 -> 987, fullfsync 3 -> 1, malloc_bytes 316290 -> 421466, mallocs 1160 -> 1137, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 8390 -> 6357, cli_maxrss_kib 5120 -> 4656, cmd:defaults 2 -> 0, frees 1381 -> 1184, fullfsync 8 -> 3, malloc_bytes 383835 -> 461579, mallocs 1636 -> 1413, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 10117 -> 8653, cli_maxrss_kib 6992 -> 6064, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3117 -> 3213, fullfsync 15 -> 7, malloc_bytes 597078 -> 712534, mallocs 3504 -> 3592, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 929 -> 612, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 932 -> 930, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 8486 -> 6215, cli_maxrss_kib 5088 -> 4576, frees 1021 -> 987, fullfsync 3 -> 1, malloc_bytes 316802 -> 421210, mallocs 1171 -> 1136, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 8601 -> 6367, cli_maxrss_kib 5120 -> 4656, cmd:defaults 2 -> 0, frees 1381 -> 1184, fullfsync 8 -> 3, malloc_bytes 382931 -> 461182, mallocs 1634 -> 1411, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 10395 -> 7674, cli_maxrss_kib 7056 -> 6192, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3413 -> 3575, fullfsync 17 -> 8, malloc_bytes 688734 -> 811209, mallocs 3820 -> 3974, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 13 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 8145 -> 6574, cli_maxrss_kib 5056 -> 4560, frees 1010 -> 987, fullfsync 3 -> 1, malloc_bytes 316098 -> 421210, mallocs 1160 -> 1136, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 8397 -> 5711, cli_maxrss_kib 5104 -> 4576, frees 1009 -> 987, fullfsync 3 -> 1, malloc_bytes 317014 -> 421466, mallocs 1162 -> 1137, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 5921 -> 5499, cli_maxrss_kib 3168 -> 2320, frees 176 -> 6, malloc_bytes 87612 -> 2298, mallocs 189 -> 10
- cli ls: cli_cpu_us 8610 -> 5999, cli_maxrss_kib 5200 -> 4688, frees 1316 -> 1289, malloc_bytes 361969 -> 467016, mallocs 1507 -> 1481, opens 13 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 8206 -> 5875, cli_maxrss_kib 5280 -> 4752, cmd:git bundle 3 -> 2, frees 10325 -> 9586, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3558320 -> 3597854, mallocs 14968 -> 12635, opens 142 -> 128, read_bytes.ipc 5269 -> 5199, read_bytes.jobs 2302 -> 2181, read_bytes.store 37665 -> 38721, read_calls.ipc 187 -> 177, read_calls.jobs 24 -> 22, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3587 -> 3585, write_bytes.store 95704 -> 95919
- acp move: barrierfsync 0 -> 4, cli_cpu_us 8150 -> 8753, cli_maxrss_kib 5600 -> 5040, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6816 -> 4098, fullfsync 15 -> 7, malloc_bytes 2003813 -> 1785334, mallocs 8302 -> 5085, opens 73 -> 15, read_bytes.ipc 468 -> 110, read_bytes.jobs 4099 -> 119, read_bytes.store 1113 -> 2340, read_calls.ipc 124 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2697 -> 1913, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 10635 -> 7710, cli_maxrss_kib 7056 -> 6496, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 8962 -> 6961, fsync 51 -> 61, fullfsync 24 -> 13, malloc_bytes 4347832 -> 3762754, mallocs 11161 -> 8670, opens 212 -> 217, read_bytes.ipc 4937 -> 2406, read_bytes.jobs 3558 -> 3366, read_bytes.outside 554 -> 691, read_bytes.store 12380 -> 18216, read_calls.ipc 171 -> 66, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.store 125 -> 111, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 32 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.outside 483 -> 640, write_bytes.store 4528 -> 2477, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 9817 -> 9531, cli_maxrss_kib 7136 -> 6640, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23351 -> 17682, fsync 59 -> 77, fullfsync 81 -> 41, malloc_bytes 8801061 -> 7526071, mallocs 30344 -> 21666, opens 417 -> 366, read_bytes.ipc 11101 -> 7683, read_bytes.jobs 8543 -> 6234, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 24538 -> 33183, read_calls.ipc 496 -> 255, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 111 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4624 -> 4084, write_bytes.jobs 8391 -> 8381, write_bytes.outside 747 -> 904, write_bytes.store 59995 -> 57310, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 8815 -> 6754, cli_maxrss_kib 5312 -> 4768, cmd:git bundle 3 -> 2, frees 13959 -> 12921, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3660238 -> 3684980, mallocs 19190 -> 16196, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 3676 -> 3555, read_bytes.store 9511 -> 10520, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4773 -> 4768, write_bytes.store 79474 -> 54941, write_calls.store 45 -> 33
- claude move: barrierfsync 0 -> 4, cli_cpu_us 8715 -> 6104, cli_maxrss_kib 5616 -> 5040, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6745 -> 4273, fullfsync 15 -> 7, malloc_bytes 1995141 -> 1819318, mallocs 8198 -> 5289, opens 73 -> 15, read_bytes.ipc 468 -> 154, read_bytes.jobs 4062 -> 82, read_bytes.store 1203 -> 2486, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1896, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 10533 -> 8567, cli_maxrss_kib 7312 -> 6544, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9109 -> 7165, fsync 55 -> 65, fullfsync 24 -> 13, malloc_bytes 4363124 -> 3777867, mallocs 11059 -> 8634, opens 248 -> 252, read_bytes.ipc 4417 -> 1884, read_bytes.jobs 8581 -> 8391, read_bytes.outside 3026 -> 3163, read_bytes.store 12959 -> 18516, read_calls.ipc 178 -> 71, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.store 125 -> 111, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 34 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 4698 -> 4710, write_bytes.outside 483 -> 640, write_bytes.store 4534 -> 2477, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 12989 -> 8902, cli_maxrss_kib 7424 -> 6656, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 27935 -> 22090, fsync 63 -> 83, fullfsync 83 -> 41, malloc_bytes 9276039 -> 7977955, mallocs 36721 -> 27502, opens 458 -> 407, read_bytes.ipc 10600 -> 7227, read_bytes.jobs 15722 -> 13416, read_bytes.outside 5735 -> 5872, read_bytes.store 25735 -> 33954, read_calls.ipc 506 -> 266, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 114 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4278 -> 3725, write_bytes.jobs 10884 -> 10899, write_bytes.outside 617 -> 774, write_bytes.store 84875 -> 82176, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 51 -> 49
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 8992 -> 6525, cli_maxrss_kib 5248 -> 4752, cmd:git bundle 3 -> 2, frees 11711 -> 10125, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3730685 -> 3766805, mallocs 16227 -> 12408, opens 152 -> 140, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 4494 -> 4373, read_bytes.state 87 -> 235, read_bytes.store 9824 -> 10827, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.state 2 -> 6, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5644 -> 5632, write_bytes.store 88084 -> 88261
- codex move: barrierfsync 0 -> 4, cli_cpu_us 8934 -> 6516, cli_maxrss_kib 5616 -> 5056, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6768 -> 4286, fullfsync 15 -> 7, malloc_bytes 1994840 -> 1819706, mallocs 8212 -> 5304, opens 73 -> 15, read_bytes.ipc 468 -> 146, read_bytes.jobs 4094 -> 106, read_bytes.store 1234 -> 2536, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1888, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 11198 -> 8501, cli_maxrss_kib 7392 -> 6528, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9265 -> 7296, fsync 59 -> 69, fullfsync 24 -> 13, malloc_bytes 4469271 -> 3883148, mallocs 11192 -> 8743, opens 292 -> 296, read_bytes.ipc 4567 -> 2034, read_bytes.jobs 12201 -> 12007, read_bytes.outside 9297 -> 9434, read_bytes.store 13439 -> 18900, read_calls.ipc 182 -> 76, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 127 -> 113, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 35 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 5635 -> 5632, write_bytes.outside 483 -> 640, write_bytes.store 4533 -> 2479, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 11747 -> 9724, cli_maxrss_kib 7440 -> 6672, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25197 -> 18822, fsync 67 -> 88, fullfsync 82 -> 40, malloc_bytes 9442366 -> 8125101, mallocs 33060 -> 22982, opens 511 -> 458, read_bytes.ipc 10766 -> 7383, read_bytes.jobs 20114 -> 17801, read_bytes.outside 18389 -> 18526, read_bytes.state 322 -> 174, read_bytes.store 26569 -> 34778, read_calls.ipc 516 -> 276, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.state 8 -> 4, read_calls.store 219 -> 207, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 117 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4212 -> 3651, write_bytes.jobs 12670 -> 12686, write_bytes.outside 504 -> 661, write_bytes.store 75365 -> 90795, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 45 -> 53
- cold start: cli_cpu_us 36930 -> 25717, cli_maxrss_kib 5120 -> 4528, frees 4318 -> 4401, malloc_bytes 1626499 -> 2213830, mallocs 5754 -> 5877
- restart: barrierfsync 0 -> 3, cli_cpu_us 72814 -> 25042, cli_maxrss_kib 5296 -> 4688, cli_processes 9 -> 4, frees 12123 -> 7787, fullfsync 38 -> 6, malloc_bytes 4010891 -> 3080473, mallocs 16190 -> 10991, opens 245 -> 87, read_bytes.jobs 6522 -> 3376, read_calls.jobs 104 -> 52, renames 17 -> 4, syncs.jobs 32 -> 3, write_bytes.ipc 3695 -> 2072, write_bytes.jobs 2290 -> 487, write_calls.ipc 53 -> 31, write_calls.jobs 16 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 13856 | 16032 | 13184 | 14960 | 0.938 | 0.934 to 0.944 |
| b | 78 | 14504 | 14992 | 13696 | 14080 | 0.944 | 0.942 to 0.945 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5768 | 7024 | 6040 | 6928 | 1.000 | 0.997 to 1.003 |
| b | 78 | 6976 | 7664 | 6992 | 7296 | 1.000 | 0.980 to 1.000 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 11536 | 11952 | 10680 | 10976 | 0.933 | 0.921 to 0.934 |
| daemon b | 15056 | 15984 | 13944 | 14832 | 0.927 | 0.918 to 0.941 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 4424 | 4816 | 4280 | 4592 | 0.987 | 0.953 to 0.993 |
| daemon b | 8768 | 10192 | 7592 | 8672 | 0.888 | 0.866 to 0.901 |
