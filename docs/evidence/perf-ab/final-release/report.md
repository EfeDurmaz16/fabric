### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 135.1 | 51.5 | 0.379 | 0.368 to 0.387 | faster |
| acp launch | 20 | 64.3 | 27.4 | 0.426 | 0.416 to 0.435 | faster |
| acp approve | 20 | 15.0 | 7.4 | 0.494 | 0.469 to 0.514 | faster |
| claude new | 20 | 133.8 | 50.5 | 0.380 | 0.361 to 0.392 | faster |
| claude launch | 20 | 90.9 | 88.8 | 0.988 | 0.918 to 1.010 | not shown |
| claude approve | 20 | 15.7 | 7.6 | 0.483 | 0.466 to 0.509 | faster |
| codex new | 20 | 135.0 | 50.9 | 0.384 | 0.359 to 0.400 | faster |
| codex launch | 20 | 103.1 | 94.3 | 0.939 | 0.918 to 0.955 | faster |
| codex approve | 20 | 15.7 | 8.3 | 0.509 | 0.495 to 0.528 | faster |
| ledger append | 20 | 15.5 | 7.7 | 0.499 | 0.488 to 0.526 | faster |
| cli version | 20 | 2.9 | 3.0 | 1.006 | 0.953 to 1.080 | not shown, median above 1 |
| cli ls | 20 | 3.9 | 3.3 | 0.805 | 0.764 to 0.883 | faster |
| acp suspend | 20 | 351.1 | 260.0 | 0.743 | 0.739 to 0.750 | faster |
| acp move | 20 | 167.2 | 105.1 | 0.632 | 0.613 to 0.656 | faster |
| acp resume | 20 | 599.7 | 259.2 | 0.431 | 0.370 to 0.492 | faster |
| acp pull | 20 | 1140.5 | 613.2 | 0.539 | 0.503 to 0.582 | faster |
| claude suspend | 20 | 423.6 | 264.1 | 0.625 | 0.618 to 0.642 | faster |
| claude move | 20 | 164.5 | 108.7 | 0.666 | 0.644 to 0.678 | faster |
| claude resume | 20 | 767.3 | 370.6 | 0.484 | 0.414 to 0.548 | faster |
| claude pull | 20 | 1375.4 | 740.1 | 0.524 | 0.509 to 0.572 | faster |
| codex suspend | 20 | 637.0 | 482.4 | 0.758 | 0.747 to 0.770 | faster |
| codex move | 20 | 166.9 | 107.9 | 0.651 | 0.640 to 0.676 | faster |
| codex resume | 20 | 979.2 | 526.5 | 0.545 | 0.491 to 0.594 | faster |
| codex pull | 20 | 1822.8 | 1114.5 | 0.612 | 0.586 to 0.652 | faster |
| cold start | 20 | 56.0 | 42.2 | 0.934 | 0.743 to 0.995 | faster |
| restart | 20 | 414.4 | 55.4 | 0.129 | 0.118 to 0.162 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38920 -> 37861 | 8064 -> 6848 | 10.7 -> 9.0 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1191 -> 295 | 4099 -> 4441 | 6848 -> 6016 | 9.9 -> 8.1 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1176 -> 784 | 5072 -> 4176 | 7.4 -> 5.9 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1636 -> 1057 | 5120 -> 4224 | 8.7 -> 6.0 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1102 -> 785 | 3493 -> 3128 | 6992 -> 5632 | 10.9 -> 7.5 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1163 -> 783 | 5056 -> 4144 | 8.8 -> 6.3 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1634 -> 1056 | 5088 -> 4224 | 8.8 -> 6.1 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3803 -> 3471 | 7056 -> 5680 | 10.7 -> 7.9 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1163 -> 783 | 5104 -> 4160 | 8.8 -> 6.0 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1169 -> 785 | 5088 -> 4160 | 8.1 -> 5.9 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 244 | 3168 -> 3040 | 7.0 -> 5.6 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1505 -> 1131 | 5264 -> 4288 | 9.4 -> 6.0 |
| acp suspend | 36 -> 35 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 39955 -> 40967 | 14976 -> 12302 | 5280 -> 4368 | 8.5 -> 6.8 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5195 -> 2449 | 8304 -> 4731 | 5664 -> 4704 | 8.9 -> 6.8 |
| acp resume | 32 -> 14 | 24 -> 13 | 51 -> 61 | 167 -> 167 | 15979 -> 21698 | 11134 -> 8117 | 6944 -> 6112 | 10.0 -> 7.2 |
| acp pull | 111 -> 56 | 81 -> 41 | 59 -> 77 | 1376 -> 1376 | 33050 -> 39693 | 30343 -> 21591 | 7168 -> 6256 | 10.8 -> 8.4 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13268 -> 14161 | 19190 -> 15822 | 5312 -> 4352 | 8.6 -> 6.4 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5273 -> 2570 | 8211 -> 4936 | 5600 -> 4688 | 8.9 -> 6.2 |
| claude resume | 34 -> 16 | 24 -> 13 | 55 -> 65 | 148 -> 148 | 21664 -> 27022 | 11071 -> 8169 | 7296 -> 6144 | 11.8 -> 8.3 |
| claude pull | 114 -> 61 | 83 -> 41 | 63 -> 83 | 1338 -> 1338 | 41609 -> 47534 | 36280 -> 27027 | 7536 -> 6304 | 13.9 -> 8.8 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 14421 -> 15276 | 16227 -> 12157 | 5264 -> 4352 | 8.6 -> 6.0 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5325 -> 2638 | 8217 -> 4954 | 5648 -> 4656 | 8.7 -> 6.2 |
| codex resume | 35 -> 17 | 24 -> 13 | 59 -> 69 | 148 -> 148 | 25768 -> 31029 | 11195 -> 8288 | 7344 -> 6160 | 11.1 -> 7.8 |
| codex pull | 117 -> 64 | 82 -> 40 | 67 -> 88 | 1338 -> 1338 | 46995 -> 52744 | 32574 -> 22756 | 7488 -> 6320 | 11.3 -> 8.3 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5813 -> 4224 | 5104 -> 4144 | 30.0 -> 23.0 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 0 | 463 -> 463 | 497505 -> 502599 | 16199 -> 9295 | 5264 -> 4336 | 71.8 -> 22.2 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 10671 -> 9046, cli_maxrss_kib 8064 -> 6848, cmd:defaults 2 -> 0, frees 28103 -> 27210, fullfsync 8 -> 3, malloc_bytes 13531770 -> 13331129, mallocs 38920 -> 37861, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 9875 -> 8111, cli_maxrss_kib 6848 -> 6016, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3342 -> 3340, fullfsync 15 -> 5, malloc_bytes 710679 -> 778349, mallocs 4099 -> 4441, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1017 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1039 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 7448 -> 5935, cli_maxrss_kib 5072 -> 4176, frees 1025 -> 659, fullfsync 3 -> 1, malloc_bytes 317314 -> 254238, mallocs 1176 -> 784, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 8728 -> 6014, cli_maxrss_kib 5120 -> 4224, cmd:defaults 2 -> 0, frees 1381 -> 853, fullfsync 8 -> 3, malloc_bytes 383327 -> 294029, mallocs 1636 -> 1057, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 10922 -> 7479, cli_maxrss_kib 6992 -> 5632, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3106 -> 2776, fullfsync 15 -> 7, malloc_bytes 596309 -> 518792, mallocs 3493 -> 3128, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 928 -> 611, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 931 -> 929, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 8838 -> 6323, cli_maxrss_kib 5056 -> 4144, frees 1012 -> 659, fullfsync 3 -> 1, malloc_bytes 316562 -> 253982, mallocs 1163 -> 783, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 8806 -> 6073, cli_maxrss_kib 5088 -> 4224, cmd:defaults 2 -> 0, frees 1381 -> 853, fullfsync 8 -> 3, malloc_bytes 382931 -> 293849, mallocs 1634 -> 1056, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 10664 -> 7908, cli_maxrss_kib 7056 -> 5680, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3395 -> 3102, fullfsync 17 -> 8, malloc_bytes 688094 -> 609299, mallocs 3803 -> 3471, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 13 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 8810 -> 6031, cli_maxrss_kib 5104 -> 4160, frees 1013 -> 659, fullfsync 3 -> 1, malloc_bytes 316674 -> 253982, mallocs 1163 -> 783, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 8137 -> 5925, cli_maxrss_kib 5088 -> 4160, frees 1015 -> 659, fullfsync 3 -> 1, malloc_bytes 317570 -> 254574, mallocs 1169 -> 785, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 7024 -> 5602, cli_maxrss_kib 3168 -> 3040, frees 176 -> 231, malloc_bytes 87612 -> 105993, mallocs 189 -> 244
- cli ls: cli_cpu_us 9401 -> 6025, cli_maxrss_kib 5264 -> 4288, frees 1314 -> 964, malloc_bytes 361841 -> 299902, mallocs 1505 -> 1131, opens 13 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 8501 -> 6807, cli_maxrss_kib 5280 -> 4368, cmd:git bundle 3 -> 2, frees 10323 -> 9260, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3558923 -> 3444885, mallocs 14976 -> 12302, opens 142 -> 128, read_bytes.ipc 5269 -> 5199, read_bytes.jobs 2302 -> 2181, read_bytes.store 37566 -> 38699, read_calls.ipc 187 -> 177, read_calls.jobs 24 -> 22, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3577 -> 3587, write_bytes.store 95684 -> 95917
- acp move: barrierfsync 0 -> 4, cli_cpu_us 8891 -> 6805, cli_maxrss_kib 5664 -> 4704, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6817 -> 3771, fullfsync 15 -> 7, malloc_bytes 2004661 -> 1613896, mallocs 8304 -> 4731, opens 73 -> 15, read_bytes.ipc 468 -> 110, read_bytes.jobs 4087 -> 119, read_bytes.store 1108 -> 2330, read_calls.ipc 124 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2697 -> 1913, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 9970 -> 7217, cli_maxrss_kib 6944 -> 6112, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 8942 -> 6523, fsync 51 -> 61, fullfsync 24 -> 13, malloc_bytes 4345636 -> 3564535, mallocs 11134 -> 8117, opens 212 -> 217, read_bytes.ipc 4938 -> 2406, read_bytes.jobs 3558 -> 3364, read_bytes.outside 554 -> 691, read_bytes.store 12299 -> 18212, read_calls.ipc 173 -> 67, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.store 125 -> 111, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 32 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.outside 483 -> 640, write_bytes.store 4529 -> 2478, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 10842 -> 8409, cli_maxrss_kib 7168 -> 6256, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23349 -> 17264, fsync 59 -> 77, fullfsync 81 -> 41, malloc_bytes 8816838 -> 7425595, mallocs 30343 -> 21591, opens 417 -> 366, read_bytes.ipc 11101 -> 7682, read_bytes.jobs 8538 -> 6233, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 24338 -> 33138, read_calls.ipc 498 -> 257, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 111 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4624 -> 4084, write_bytes.jobs 8385 -> 8404, write_bytes.outside 747 -> 904, write_bytes.store 59948 -> 57306, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 8637 -> 6388, cli_maxrss_kib 5312 -> 4352, cmd:git bundle 3 -> 2, frees 13962 -> 12573, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3660651 -> 3510122, mallocs 19190 -> 15822, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 3676 -> 3555, read_bytes.store 9505 -> 10519, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4755 -> 4748, write_bytes.store 54746 -> 79653, write_calls.store 33 -> 45
- claude move: barrierfsync 0 -> 4, cli_cpu_us 8942 -> 6205, cli_maxrss_kib 5600 -> 4688, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6754 -> 3945, fullfsync 15 -> 7, malloc_bytes 1995073 -> 1647900, mallocs 8211 -> 4936, opens 73 -> 15, read_bytes.ipc 468 -> 154, read_bytes.jobs 4066 -> 82, read_bytes.store 1207 -> 2488, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1896, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 11834 -> 8348, cli_maxrss_kib 7296 -> 6144, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9120 -> 6728, fsync 55 -> 65, fullfsync 24 -> 13, malloc_bytes 4364481 -> 3584029, mallocs 11071 -> 8169, opens 248 -> 252, read_bytes.ipc 4417 -> 1884, read_bytes.jobs 8589 -> 8393, read_bytes.outside 3026 -> 3163, read_bytes.store 12953 -> 18507, read_calls.ipc 179 -> 73, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.store 125 -> 111, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 34 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 4707 -> 4702, write_bytes.outside 483 -> 640, write_bytes.store 4532 -> 2478, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 13876 -> 8843, cli_maxrss_kib 7536 -> 6304, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 27900 -> 21650, fsync 63 -> 83, fullfsync 83 -> 41, malloc_bytes 9168603 -> 7775109, mallocs 36280 -> 27027, opens 458 -> 407, read_bytes.ipc 10600 -> 7227, read_bytes.jobs 15735 -> 13412, read_bytes.outside 5735 -> 5872, read_bytes.store 25700 -> 33948, read_calls.ipc 507 -> 268, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 114 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4278 -> 3725, write_bytes.jobs 10902 -> 10877, write_bytes.outside 617 -> 774, write_bytes.store 84858 -> 57449, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 51 -> 37
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 8566 -> 6036, cli_maxrss_kib 5264 -> 4352, cmd:git bundle 3 -> 2, frees 11718 -> 9793, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3730521 -> 3643794, mallocs 16227 -> 12157, opens 152 -> 138, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 4494 -> 4373, read_bytes.store 9840 -> 10816, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5636 -> 5648, write_bytes.store 88082 -> 88267
- codex move: barrierfsync 0 -> 4, cli_cpu_us 8696 -> 6200, cli_maxrss_kib 5648 -> 4656, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6772 -> 3960, fullfsync 15 -> 7, malloc_bytes 1995376 -> 1648278, mallocs 8217 -> 4954, opens 73 -> 15, read_bytes.ipc 468 -> 146, read_bytes.jobs 4090 -> 106, read_bytes.store 1235 -> 2532, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1888, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 11145 -> 7841, cli_maxrss_kib 7344 -> 6160, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9276 -> 6869, fsync 59 -> 69, fullfsync 24 -> 13, malloc_bytes 4468869 -> 3690141, mallocs 11195 -> 8288, opens 292 -> 296, read_bytes.ipc 4567 -> 2034, read_bytes.jobs 12201 -> 12006, read_bytes.outside 9297 -> 9434, read_bytes.store 13445 -> 18901, read_calls.ipc 183 -> 77, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 127 -> 113, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 35 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 5637 -> 5634, write_bytes.outside 483 -> 640, write_bytes.store 4528 -> 2479, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 11326 -> 8336, cli_maxrss_kib 7488 -> 6320, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25163 -> 18381, fsync 67 -> 88, fullfsync 82 -> 40, malloc_bytes 9314271 -> 7951763, mallocs 32574 -> 22756, opens 509 -> 460, read_bytes.ipc 10766 -> 7383, read_bytes.jobs 20107 -> 17804, read_bytes.outside 18389 -> 18526, read_bytes.state 322 -> 174, read_bytes.store 26566 -> 34766, read_calls.ipc 517 -> 277, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.state 8 -> 4, read_calls.store 219 -> 207, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 117 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4212 -> 3651, write_bytes.jobs 12676 -> 12693, write_bytes.outside 504 -> 661, write_bytes.store 93465 -> 90804, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 55 -> 53
- cold start: cli_cpu_us 30032 -> 22971, cli_maxrss_kib 5104 -> 4144, frees 4378 -> 2881, malloc_bytes 1630114 -> 1396802, mallocs 5813 -> 4224
- restart: barrierfsync 0 -> 3, cli_cpu_us 71841 -> 22201, cli_maxrss_kib 5264 -> 4336, cli_processes 9 -> 4, frees 12129 -> 6229, fullfsync 38 -> 6, malloc_bytes 4011801 -> 2221851, mallocs 16199 -> 9295, opens 245 -> 87, read_bytes.jobs 6522 -> 3376, read_bytes.store 473836 -> 482076, read_calls.jobs 104 -> 52, read_calls.store 117 -> 119, renames 17 -> 4, syncs.jobs 32 -> 3, write_bytes.ipc 3695 -> 2072, write_bytes.jobs 2290 -> 487, write_calls.ipc 53 -> 31, write_calls.jobs 16 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 14128 | 16048 | 12792 | 14560 | 0.891 | 0.889 to 0.894 |
| b | 78 | 15064 | 15408 | 13248 | 13456 | 0.874 | 0.867 to 0.882 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5520 | 6704 | 5480 | 6336 | 0.969 | 0.963 to 0.972 |
| b | 78 | 6784 | 7168 | 6304 | 6576 | 0.933 | 0.917 to 0.933 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 11344 | 11808 | 9832 | 10128 | 0.867 | 0.864 to 0.870 |
| daemon b | 15968 | 16352 | 13808 | 14304 | 0.860 | 0.855 to 0.871 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 4216 | 4672 | 3920 | 4208 | 0.930 | 0.922 to 0.939 |
| daemon b | 8296 | 9344 | 6904 | 7680 | 0.838 | 0.821 to 0.851 |
