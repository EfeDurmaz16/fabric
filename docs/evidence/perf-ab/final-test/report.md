### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 153.3 | 68.4 | 0.447 | 0.442 to 0.454 | faster |
| acp launch | 20 | 69.1 | 31.5 | 0.459 | 0.448 to 0.465 | faster |
| acp approve | 20 | 15.7 | 8.3 | 0.532 | 0.514 to 0.556 | faster |
| claude new | 20 | 138.3 | 51.6 | 0.374 | 0.367 to 0.380 | faster |
| claude launch | 20 | 93.6 | 91.0 | 0.978 | 0.945 to 1.002 | not shown |
| claude approve | 20 | 16.9 | 9.2 | 0.535 | 0.528 to 0.558 | faster |
| codex new | 20 | 136.9 | 52.3 | 0.386 | 0.373 to 0.395 | faster |
| codex launch | 20 | 104.7 | 98.2 | 0.940 | 0.917 to 0.960 | faster |
| codex approve | 20 | 16.7 | 8.7 | 0.533 | 0.519 to 0.555 | faster |
| ledger append | 20 | 16.4 | 8.6 | 0.529 | 0.522 to 0.553 | faster |
| cli version | 20 | 3.0 | 3.1 | 1.015 | 0.966 to 1.087 | not shown, median above 1 |
| cli ls | 20 | 5.1 | 4.7 | 0.888 | 0.876 to 0.907 | faster |
| acp suspend | 20 | 357.5 | 267.0 | 0.741 | 0.734 to 0.749 | faster |
| acp move | 20 | 166.4 | 108.6 | 0.651 | 0.634 to 0.682 | faster |
| acp resume | 20 | 607.5 | 263.3 | 0.436 | 0.372 to 0.501 | faster |
| acp pull | 20 | 1128.2 | 626.0 | 0.560 | 0.521 to 0.595 | faster |
| claude suspend | 20 | 436.1 | 276.1 | 0.630 | 0.623 to 0.639 | faster |
| claude move | 20 | 166.3 | 112.0 | 0.665 | 0.635 to 0.679 | faster |
| claude resume | 20 | 779.1 | 373.8 | 0.486 | 0.416 to 0.542 | faster |
| claude pull | 20 | 1410.9 | 752.0 | 0.531 | 0.507 to 0.578 | faster |
| codex suspend | 20 | 649.3 | 496.6 | 0.764 | 0.752 to 0.773 | faster |
| codex move | 20 | 171.1 | 111.7 | 0.656 | 0.628 to 0.669 | faster |
| codex resume | 20 | 998.7 | 536.2 | 0.554 | 0.478 to 0.585 | faster |
| codex pull | 20 | 1832.5 | 1124.8 | 0.621 | 0.598 to 0.656 | faster |
| cold start | 20 | 60.3 | 60.0 | 0.991 | 0.960 to 1.038 | not shown |
| restart | 20 | 423.5 | 77.2 | 0.185 | 0.151 to 0.204 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38962 -> 37906 | 14288 -> 14432 | 20.0 -> 18.7 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1183 -> 295 | 3741 -> 4488 | 14432 -> 15056 | 12.1 -> 9.9 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1199 -> 817 | 10560 -> 10480 | 8.8 -> 6.9 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1681 -> 1094 | 10800 -> 10720 | 9.3 -> 6.8 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1102 -> 788 | 3537 -> 3169 | 14224 -> 14400 | 11.8 -> 9.3 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1202 -> 816 | 10560 -> 10464 | 9.2 -> 6.6 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1677 -> 1091 | 10800 -> 10704 | 9.4 -> 6.7 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3844 -> 3518 | 14144 -> 14256 | 12.1 -> 10.5 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1207 -> 817 | 10528 -> 10496 | 8.9 -> 7.4 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1204 -> 816 | 10576 -> 10496 | 9.2 -> 7.2 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 256 | 4928 -> 5088 | 5.5 -> 5.4 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1554 -> 1161 | 10768 -> 10688 | 8.4 -> 6.6 |
| acp suspend | 36 -> 35 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 39965 -> 41001 | 15000 -> 12333 | 11280 -> 11312 | 9.0 -> 7.2 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5211 -> 2457 | 8334 -> 4764 | 11904 -> 11920 | 10.2 -> 7.7 |
| acp resume | 32 -> 14 | 24 -> 13 | 51 -> 61 | 167 -> 167 | 16000 -> 21718 | 11180 -> 8249 | 14608 -> 15264 | 12.4 -> 9.8 |
| acp pull | 111 -> 56 | 81 -> 41 | 59 -> 77 | 1376 -> 1376 | 33115 -> 39768 | 30391 -> 21349 | 14816 -> 15360 | 13.2 -> 10.4 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13260 -> 14171 | 19235 -> 15864 | 11296 -> 11248 | 10.2 -> 7.1 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5275 -> 2572 | 8253 -> 4969 | 11888 -> 11936 | 10.5 -> 7.8 |
| claude resume | 34 -> 16 | 24 -> 13 | 55 -> 65 | 148 -> 148 | 21650 -> 27030 | 11112 -> 8211 | 14736 -> 15232 | 15.5 -> 11.0 |
| claude pull | 114 -> 61 | 83 -> 41 | 63 -> 83 | 1338 -> 1338 | 41576 -> 47616 | 36327 -> 27095 | 14960 -> 15328 | 15.6 -> 11.5 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 14400 -> 15338 | 16240 -> 12185 | 11280 -> 11264 | 9.7 -> 7.6 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5315 -> 2650 | 8237 -> 4988 | 11888 -> 11920 | 10.9 -> 9.0 |
| codex resume | 35 -> 17 | 24 -> 13 | 59 -> 69 | 148 -> 148 | 25767 -> 31042 | 11246 -> 8325 | 14784 -> 15200 | 13.5 -> 10.1 |
| codex pull | 117 -> 64 | 82 -> 40 | 67 -> 88 | 1338 -> 1338 | 46843 -> 52779 | 32642 -> 22582 | 14992 -> 15328 | 14.1 -> 11.1 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5878 -> 4371 | 10432 -> 10384 | 32.3 -> 25.8 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 0 | 463 -> 463 | 485145 -> 477879 | 16455 -> 9451 | 11008 -> 10880 | 81.5 -> 26.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 20040 -> 18726, cli_maxrss_kib 14288 -> 14432, cmd:defaults 2 -> 0, frees 28135 -> 27244, fullfsync 8 -> 3, malloc_bytes 13540056 -> 13340055, mallocs 38962 -> 37906, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 12067 -> 9908, cli_maxrss_kib 14432 -> 15056, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3338 -> 3378, fullfsync 15 -> 5, malloc_bytes 615978 -> 787531, mallocs 3741 -> 4488, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1009 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1031 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 8809 -> 6886, cli_maxrss_kib 10560 -> 10480, frees 1040 -> 684, fullfsync 3 -> 1, malloc_bytes 324087 -> 262083, mallocs 1199 -> 817, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 9291 -> 6842, cli_maxrss_kib 10800 -> 10720, cmd:defaults 2 -> 0, frees 1413 -> 879, fullfsync 8 -> 3, malloc_bytes 392409 -> 302255, mallocs 1681 -> 1094, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 11816 -> 9318, cli_maxrss_kib 14224 -> 14400, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3133 -> 2802, fullfsync 15 -> 7, malloc_bytes 605132 -> 527759, mallocs 3537 -> 3169, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 928 -> 614, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 931 -> 932, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 9248 -> 6593, cli_maxrss_kib 10560 -> 10464, frees 1044 -> 684, fullfsync 3 -> 1, malloc_bytes 324087 -> 261827, mallocs 1202 -> 816, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 9436 -> 6699, cli_maxrss_kib 10800 -> 10704, cmd:defaults 2 -> 0, frees 1413 -> 879, fullfsync 8 -> 3, malloc_bytes 391425 -> 301795, mallocs 1677 -> 1091, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 12110 -> 10471, cli_maxrss_kib 14144 -> 14256, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3425 -> 3130, fullfsync 17 -> 8, malloc_bytes 696626 -> 618694, mallocs 3844 -> 3518, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 13 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 8932 -> 7441, cli_maxrss_kib 10528 -> 10496, frees 1048 -> 685, fullfsync 3 -> 1, malloc_bytes 324855 -> 261891, mallocs 1207 -> 817, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 9230 -> 7186, cli_maxrss_kib 10576 -> 10496, frees 1043 -> 683, fullfsync 3 -> 1, malloc_bytes 324855 -> 262019, mallocs 1204 -> 816, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 5471 -> 5414, cli_maxrss_kib 4928 -> 5088, frees 185 -> 240, malloc_bytes 88361 -> 106774, mallocs 201 -> 256
- cli ls: cli_cpu_us 8358 -> 6611, cli_maxrss_kib 10768 -> 10688, frees 1356 -> 987, malloc_bytes 370058 -> 307335, mallocs 1554 -> 1161, opens 13 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 8998 -> 7180, cli_maxrss_kib 11280 -> 11312, cmd:git bundle 3 -> 2, frees 10345 -> 9283, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3566381 -> 3453457, mallocs 15000 -> 12333, opens 142 -> 128, read_bytes.ipc 5269 -> 5199, read_bytes.jobs 2302 -> 2181, read_bytes.store 37576 -> 38733, read_calls.ipc 187 -> 177, read_calls.jobs 24 -> 22, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3586 -> 3579, write_bytes.store 95696 -> 95933
- acp move: barrierfsync 0 -> 4, cli_cpu_us 10190 -> 7704, cli_maxrss_kib 11904 -> 11920, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6835 -> 3795, fullfsync 15 -> 7, malloc_bytes 2017708 -> 1622975, mallocs 8334 -> 4764, opens 73 -> 15, read_bytes.ipc 468 -> 110, read_bytes.jobs 4099 -> 119, read_bytes.store 1112 -> 2338, read_calls.ipc 124 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2698 -> 1913, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 12422 -> 9751, cli_maxrss_kib 14608 -> 15264, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 8976 -> 6556, fsync 51 -> 61, fullfsync 24 -> 13, malloc_bytes 4355227 -> 3578498, mallocs 11180 -> 8249, opens 212 -> 219, read_bytes.ipc 4938 -> 2406, read_bytes.jobs 3557 -> 3361, read_bytes.outside 554 -> 691, read_bytes.store 12321 -> 18235, read_calls.ipc 173 -> 67, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.store 125 -> 111, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 32 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.jobs 3563 -> 3554, write_bytes.outside 483 -> 640, write_bytes.store 4529 -> 2479, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 13185 -> 10429, cli_maxrss_kib 14816 -> 15360, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23390 -> 17274, fsync 59 -> 77, fullfsync 81 -> 41, malloc_bytes 8815685 -> 7348097, mallocs 30391 -> 21349, opens 417 -> 366, read_bytes.ipc 11100 -> 7683, read_bytes.jobs 8535 -> 6230, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 24406 -> 33216, read_calls.ipc 499 -> 258, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 111 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4625 -> 4084, write_bytes.jobs 8381 -> 8412, write_bytes.outside 747 -> 904, write_bytes.store 59970 -> 57330, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 10158 -> 7099, cli_maxrss_kib 11296 -> 11248, cmd:git bundle 3 -> 2, frees 13997 -> 12602, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3669140 -> 3519866, mallocs 19235 -> 15864, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 3676 -> 3555, read_bytes.store 9497 -> 10529, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4772 -> 4775, write_bytes.store 54748 -> 54953
- claude move: barrierfsync 0 -> 4, cli_cpu_us 10492 -> 7784, cli_maxrss_kib 11888 -> 11936, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6792 -> 3969, fullfsync 15 -> 7, malloc_bytes 2008754 -> 1657469, mallocs 8253 -> 4969, opens 73 -> 15, read_bytes.ipc 468 -> 154, read_bytes.jobs 4070 -> 82, read_bytes.store 1205 -> 2490, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2362 -> 1896, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 15544 -> 11036, cli_maxrss_kib 14736 -> 15232, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9148 -> 6757, fsync 55 -> 65, fullfsync 24 -> 13, malloc_bytes 4373869 -> 3593947, mallocs 11112 -> 8211, opens 248 -> 252, read_bytes.ipc 4417 -> 1884, read_bytes.jobs 8584 -> 8389, read_bytes.outside 3026 -> 3163, read_bytes.store 12944 -> 18519, read_calls.ipc 176 -> 73, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.store 125 -> 111, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 34 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 4710 -> 4714, write_bytes.outside 483 -> 640, write_bytes.store 4533 -> 2478, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 15569 -> 11542, cli_maxrss_kib 14960 -> 15328, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 27947 -> 21698, fsync 63 -> 83, fullfsync 83 -> 41, malloc_bytes 9183775 -> 7789187, mallocs 36327 -> 27095, opens 458 -> 407, read_bytes.ipc 10600 -> 7226, read_bytes.jobs 15729 -> 13422, read_bytes.outside 5735 -> 5872, read_bytes.store 25673 -> 34020, read_calls.ipc 507 -> 268, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 114 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4279 -> 3725, write_bytes.jobs 10895 -> 10896, write_bytes.outside 617 -> 774, write_bytes.store 70008 -> 57466, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 45 -> 37
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 9675 -> 7565, cli_maxrss_kib 11280 -> 11264, cmd:git bundle 3 -> 2, frees 11718 -> 9822, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3737975 -> 3653010, mallocs 16240 -> 12185, opens 152 -> 138, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 4494 -> 4373, read_bytes.store 9819 -> 10878, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5637 -> 5649, write_bytes.store 55122 -> 71807, write_calls.store 33 -> 41
- codex move: barrierfsync 0 -> 4, cli_cpu_us 10863 -> 9011, cli_maxrss_kib 11888 -> 11920, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6782 -> 3985, fullfsync 15 -> 7, malloc_bytes 2007606 -> 1657836, mallocs 8237 -> 4988, opens 73 -> 15, read_bytes.ipc 468 -> 146, read_bytes.jobs 4078 -> 106, read_bytes.store 1237 -> 2544, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2362 -> 1888, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 13465 -> 10087, cli_maxrss_kib 14784 -> 15200, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9308 -> 6899, fsync 59 -> 69, fullfsync 24 -> 13, malloc_bytes 4478881 -> 3699732, mallocs 11246 -> 8325, opens 292 -> 296, read_bytes.ipc 4567 -> 2034, read_bytes.jobs 12204 -> 11993, read_bytes.outside 9297 -> 9434, read_bytes.store 13441 -> 18927, read_calls.ipc 182 -> 77, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 127 -> 113, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 35 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 5641 -> 5622, write_bytes.outside 483 -> 640, write_bytes.store 4533 -> 2478, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 14081 -> 11055, cli_maxrss_kib 14992 -> 15328, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25218 -> 18428, fsync 67 -> 88, fullfsync 82 -> 40, malloc_bytes 9331310 -> 7936042, mallocs 32642 -> 22582, opens 509 -> 458, read_bytes.ipc 10766 -> 7383, read_bytes.jobs 20130 -> 17805, read_bytes.outside 18389 -> 18526, read_bytes.store 26539 -> 34800, read_calls.ipc 516 -> 276, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 219 -> 207, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 117 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4213 -> 3651, write_bytes.jobs 12699 -> 12671, write_bytes.outside 504 -> 661, write_bytes.store 93466 -> 90799, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 55 -> 53
- cold start: cli_cpu_us 32289 -> 25783, cli_maxrss_kib 10432 -> 10384, frees 4406 -> 2993, malloc_bytes 1658178 -> 1432433, mallocs 5878 -> 4371
- restart: barrierfsync 0 -> 3, cli_cpu_us 81538 -> 26749, cli_maxrss_kib 11008 -> 10880, cli_processes 9 -> 4, frees 12312 -> 6344, fullfsync 38 -> 6, malloc_bytes 4075335 -> 2259094, mallocs 16455 -> 9451, opens 245 -> 87, read_bytes.jobs 6522 -> 3376, read_bytes.store 461476 -> 457356, read_calls.jobs 104 -> 52, read_calls.store 114 -> 113, renames 17 -> 4, syncs.jobs 32 -> 3, write_bytes.ipc 3695 -> 2072, write_bytes.jobs 2290 -> 487, write_calls.ipc 53 -> 31, write_calls.jobs 16 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 25816 | 29648 | 26760 | 30128 | 1.028 | 1.022 to 1.042 |
| b | 78 | 27008 | 27712 | 27584 | 27776 | 1.015 | 1.012 to 1.016 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 6856 | 10512 | 6992 | 8976 | 0.981 | 0.945 to 0.999 |
| b | 78 | 9824 | 10928 | 8240 | 8512 | 0.833 | 0.818 to 0.833 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20048 | 20512 | 21008 | 21280 | 1.047 | 1.043 to 1.051 |
| daemon b | 28784 | 30352 | 28584 | 29216 | 0.985 | 0.980 to 0.992 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5080 | 5568 | 5056 | 5344 | 0.994 | 0.980 to 1.007 |
| daemon b | 12152 | 13840 | 9320 | 10160 | 0.772 | 0.766 to 0.780 |
