### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 149.8 | 63.3 | 0.429 | 0.401 to 0.473 | faster |
| acp launch | 20 | 72.2 | 31.2 | 0.442 | 0.417 to 0.451 | faster |
| acp approve | 20 | 15.8 | 8.1 | 0.525 | 0.488 to 0.564 | faster |
| claude new | 20 | 142.1 | 59.3 | 0.413 | 0.402 to 0.444 | faster |
| claude launch | 20 | 92.9 | 91.2 | 0.987 | 0.916 to 1.025 | not shown |
| claude approve | 20 | 16.0 | 8.2 | 0.512 | 0.478 to 0.545 | faster |
| codex new | 20 | 143.9 | 58.7 | 0.414 | 0.389 to 0.431 | faster |
| codex launch | 20 | 104.6 | 98.0 | 0.928 | 0.886 to 0.970 | faster |
| codex approve | 20 | 15.6 | 8.4 | 0.550 | 0.514 to 0.608 | faster |
| ledger append | 20 | 16.2 | 7.9 | 0.511 | 0.494 to 0.561 | faster |
| cli version | 20 | 3.2 | 3.3 | 1.052 | 0.951 to 1.215 | not shown, median above 1 |
| cli ls | 20 | 4.1 | 3.5 | 0.816 | 0.782 to 0.870 | faster |
| acp suspend | 20 | 418.1 | 336.1 | 0.785 | 0.753 to 0.832 | faster |
| acp move | 20 | 169.3 | 103.9 | 0.620 | 0.598 to 0.643 | faster |
| acp resume | 20 | 734.6 | 278.1 | 0.414 | 0.348 to 0.490 | faster |
| acp pull | 20 | 1364.7 | 704.7 | 0.524 | 0.483 to 0.595 | faster |
| claude suspend | 20 | 477.3 | 321.6 | 0.675 | 0.654 to 0.705 | faster |
| claude move | 20 | 164.7 | 107.8 | 0.645 | 0.632 to 0.662 | faster |
| claude resume | 20 | 904.3 | 383.5 | 0.470 | 0.391 to 0.553 | faster |
| claude pull | 20 | 1524.5 | 810.8 | 0.550 | 0.488 to 0.587 | faster |
| codex suspend | 20 | 704.1 | 550.0 | 0.779 | 0.765 to 0.800 | faster |
| codex move | 20 | 163.3 | 108.4 | 0.659 | 0.630 to 0.695 | faster |
| codex resume | 20 | 1177.8 | 563.4 | 0.515 | 0.468 to 0.561 | faster |
| codex pull | 20 | 2062.0 | 1242.2 | 0.626 | 0.579 to 0.666 | faster |
| cold start | 20 | 57.1 | 56.8 | 0.992 | 0.959 to 1.066 | not shown |
| restart | 20 | 419.6 | 59.8 | 0.143 | 0.124 to 0.170 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38909 -> 38207 | 8016 -> 7360 | 10.4 -> 8.7 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1185 -> 295 | 4095 -> 4936 | 6880 -> 6400 | 10.9 -> 8.4 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1165 -> 1131 | 5104 -> 4592 | 9.3 -> 6.1 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1637 -> 1401 | 5152 -> 4608 | 9.5 -> 6.8 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1100 -> 786 | 3496 -> 3582 | 7088 -> 6064 | 12.0 -> 8.2 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1167 -> 1129 | 5072 -> 4592 | 8.9 -> 6.5 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1646 -> 1401 | 5120 -> 4624 | 8.3 -> 6.6 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3802 -> 3966 | 7040 -> 6144 | 13.1 -> 8.4 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1160 -> 1129 | 5072 -> 4560 | 9.3 -> 6.5 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1170 -> 1132 | 5040 -> 4592 | 9.4 -> 6.4 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 553 | 3168 -> 3312 | 5.9 -> 5.8 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1506 -> 1477 | 5168 -> 4640 | 8.9 -> 6.3 |
| acp suspend | 36 -> 35 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 40023 -> 41057 | 14963 -> 12644 | 5296 -> 4752 | 9.2 -> 6.7 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5230 -> 2503 | 8306 -> 5078 | 5664 -> 5056 | 9.6 -> 6.9 |
| acp resume | 32 -> 14 | 24 -> 13 | 51 -> 61 | 167 -> 167 | 16080 -> 21773 | 11124 -> 8573 | 6944 -> 6528 | 11.8 -> 8.7 |
| acp pull | 111 -> 56 | 81 -> 41 | 59 -> 77 | 1376 -> 1376 | 33347 -> 39937 | 29948 -> 21958 | 7088 -> 6720 | 11.6 -> 8.7 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13260 -> 14165 | 19192 -> 16162 | 5280 -> 4704 | 8.7 -> 6.9 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5249 -> 2566 | 8199 -> 5282 | 5632 -> 5056 | 9.8 -> 7.1 |
| claude resume | 34 -> 16 | 24 -> 13 | 55 -> 65 | 148 -> 148 | 21805 -> 27046 | 11063 -> 8625 | 7360 -> 6528 | 12.9 -> 9.1 |
| claude pull | 114 -> 61 | 83 -> 41 | 63 -> 83 | 1338 -> 1338 | 41562 -> 47634 | 36316 -> 27510 | 7408 -> 6672 | 12.9 -> 9.4 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 14450 -> 15317 | 16191 -> 12375 | 5232 -> 4704 | 8.4 -> 7.3 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5317 -> 2642 | 8209 -> 5300 | 5632 -> 5040 | 9.1 -> 6.9 |
| codex resume | 35 -> 17 | 24 -> 13 | 59 -> 69 | 148 -> 148 | 25788 -> 31079 | 11191 -> 8735 | 7408 -> 6512 | 12.6 -> 9.7 |
| codex pull | 117 -> 64 | 82 -> 40 | 67 -> 88 | 1338 -> 1338 | 47104 -> 52848 | 32607 -> 23215 | 7456 -> 6640 | 12.0 -> 9.4 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5758 -> 5853 | 5088 -> 4528 | 43.0 -> 26.6 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 0 | 463 -> 463 | 460425 -> 481999 | 14690 -> 10961 | 5216 -> 4688 | 72.1 -> 27.6 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 10390 -> 8702, cli_maxrss_kib 8016 -> 7360, cmd:defaults 2 -> 0, frees 28090 -> 27531, fullfsync 8 -> 3, malloc_bytes 13531786 -> 13498086, mallocs 38909 -> 38207, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 10899 -> 8432, cli_maxrss_kib 6880 -> 6400, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3338 -> 3806, fullfsync 15 -> 5, malloc_bytes 710338 -> 980002, mallocs 4095 -> 4936, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1011 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1033 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 9270 -> 6070, cli_maxrss_kib 5104 -> 4592, frees 1014 -> 980, fullfsync 3 -> 1, malloc_bytes 316610 -> 421531, mallocs 1165 -> 1131, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 9518 -> 6756, cli_maxrss_kib 5152 -> 4608, cmd:defaults 2 -> 0, frees 1381 -> 1174, fullfsync 8 -> 3, malloc_bytes 383663 -> 460858, mallocs 1637 -> 1401, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 12019 -> 8151, cli_maxrss_kib 7088 -> 6064, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3108 -> 3204, fullfsync 15 -> 7, malloc_bytes 596776 -> 712108, mallocs 3496 -> 3582, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 926 -> 612, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 929 -> 930, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 8858 -> 6518, cli_maxrss_kib 5072 -> 4592, frees 1016 -> 980, fullfsync 3 -> 1, malloc_bytes 316994 -> 420939, mallocs 1167 -> 1129, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 8346 -> 6648, cli_maxrss_kib 5120 -> 4624, cmd:defaults 2 -> 0, frees 1392 -> 1174, fullfsync 8 -> 3, malloc_bytes 384147 -> 460802, mallocs 1646 -> 1401, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 13113 -> 8412, cli_maxrss_kib 7040 -> 6144, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3395 -> 3567, fullfsync 17 -> 8, malloc_bytes 687582 -> 810908, mallocs 3802 -> 3966, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 14 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 9332 -> 6495, cli_maxrss_kib 5072 -> 4560, frees 1009 -> 980, fullfsync 3 -> 1, malloc_bytes 316546 -> 420939, mallocs 1160 -> 1129, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 9410 -> 6414, cli_maxrss_kib 5040 -> 4592, frees 1020 -> 980, fullfsync 3 -> 1, malloc_bytes 317826 -> 421451, mallocs 1170 -> 1132, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 5947 -> 5805, cli_maxrss_kib 3168 -> 3312, frees 176 -> 516, malloc_bytes 87612 -> 264640, mallocs 189 -> 553
- cli ls: cli_cpu_us 8940 -> 6336, cli_maxrss_kib 5168 -> 4640, frees 1314 -> 1285, malloc_bytes 362353 -> 466859, mallocs 1506 -> 1477, opens 13 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 9188 -> 6685, cli_maxrss_kib 5296 -> 4752, cmd:git bundle 3 -> 2, frees 10324 -> 9579, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3541313 -> 3614224, mallocs 14963 -> 12644, opens 142 -> 128, read_bytes.ipc 5269 -> 5199, read_bytes.jobs 2302 -> 2181, read_bytes.store 37634 -> 38789, read_calls.ipc 187 -> 177, read_calls.jobs 24 -> 22, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3587 -> 3582, write_bytes.store 95740 -> 95983
- acp move: barrierfsync 0 -> 4, cli_cpu_us 9600 -> 6882, cli_maxrss_kib 5664 -> 5056, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6818 -> 4091, fullfsync 15 -> 7, malloc_bytes 2004306 -> 1785007, mallocs 8306 -> 5078, opens 73 -> 15, read_bytes.ipc 468 -> 110, read_bytes.jobs 4099 -> 119, read_bytes.store 1131 -> 2384, read_calls.ipc 124 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2698 -> 1913, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 11841 -> 8700, cli_maxrss_kib 6944 -> 6528, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 8932 -> 6925, fsync 51 -> 61, fullfsync 24 -> 13, malloc_bytes 4344834 -> 3757959, mallocs 11124 -> 8573, opens 212 -> 217, read_bytes.ipc 4938 -> 2406, read_bytes.jobs 3557 -> 3363, read_bytes.outside 554 -> 691, read_bytes.store 12401 -> 18288, read_calls.ipc 172 -> 67, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.store 125 -> 111, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 32 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.jobs 3549 -> 3561, write_bytes.outside 483 -> 640, write_bytes.store 4535 -> 2478, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 11591 -> 8669, cli_maxrss_kib 7088 -> 6720, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23325 -> 17700, fsync 59 -> 77, fullfsync 81 -> 41, malloc_bytes 8696353 -> 7623072, mallocs 29948 -> 21958, opens 417 -> 366, read_bytes.ipc 11102 -> 7683, read_bytes.jobs 8541 -> 6229, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 24632 -> 33386, read_calls.ipc 498 -> 257, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 111 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4625 -> 4084, write_bytes.jobs 8390 -> 8384, write_bytes.outside 747 -> 904, write_bytes.store 60006 -> 57347, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 8736 -> 6924, cli_maxrss_kib 5280 -> 4704, cmd:git bundle 3 -> 2, frees 13961 -> 12888, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3660256 -> 3680018, mallocs 19192 -> 16162, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 3676 -> 3555, read_bytes.store 9497 -> 10523, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4746 -> 4766, write_bytes.store 54738 -> 54955
- claude move: barrierfsync 0 -> 4, cli_cpu_us 9818 -> 7082, cli_maxrss_kib 5632 -> 5056, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6747 -> 4266, fullfsync 15 -> 7, malloc_bytes 1994289 -> 1819303, mallocs 8199 -> 5282, opens 73 -> 15, read_bytes.ipc 468 -> 154, read_bytes.jobs 4046 -> 82, read_bytes.store 1203 -> 2484, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2362 -> 1896, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 12926 -> 9123, cli_maxrss_kib 7360 -> 6528, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9113 -> 7156, fsync 55 -> 65, fullfsync 24 -> 13, malloc_bytes 4363784 -> 3777468, mallocs 11063 -> 8625, opens 250 -> 252, read_bytes.ipc 4417 -> 1884, read_bytes.jobs 8586 -> 8389, read_bytes.outside 3026 -> 3163, read_bytes.state 270 -> 122, read_bytes.store 12949 -> 18535, read_calls.ipc 176 -> 71, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.state 8 -> 4, read_calls.store 125 -> 111, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 34 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 4694 -> 4709, write_bytes.outside 483 -> 640, write_bytes.store 4529 -> 2481, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 12895 -> 9374, cli_maxrss_kib 7408 -> 6672, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 27949 -> 22097, fsync 63 -> 83, fullfsync 83 -> 41, malloc_bytes 9170492 -> 8017135, mallocs 36316 -> 27510, opens 458 -> 407, read_bytes.ipc 10599 -> 7226, read_bytes.jobs 15728 -> 13424, read_bytes.outside 5735 -> 5872, read_bytes.store 25660 -> 34036, read_calls.ipc 507 -> 267, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 114 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4279 -> 3725, write_bytes.jobs 10895 -> 10897, write_bytes.outside 617 -> 774, write_bytes.store 66771 -> 82177, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 41 -> 49
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 8403 -> 7332, cli_maxrss_kib 5232 -> 4704, cmd:git bundle 3 -> 2, frees 11683 -> 10116, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3727835 -> 3747599, mallocs 16191 -> 12375, opens 152 -> 138, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 4494 -> 4373, read_bytes.store 9869 -> 10857, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5637 -> 5628, write_bytes.store 63368 -> 63593
- codex move: barrierfsync 0 -> 4, cli_cpu_us 9143 -> 6913, cli_maxrss_kib 5632 -> 5040, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6763 -> 4282, fullfsync 15 -> 7, malloc_bytes 1995048 -> 1819451, mallocs 8209 -> 5300, opens 73 -> 15, read_bytes.ipc 468 -> 146, read_bytes.jobs 4078 -> 106, read_bytes.store 1239 -> 2536, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2362 -> 1888, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 12625 -> 9749, cli_maxrss_kib 7408 -> 6512, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9265 -> 7288, fsync 59 -> 69, fullfsync 24 -> 13, malloc_bytes 4468752 -> 3882838, mallocs 11191 -> 8735, opens 292 -> 296, read_bytes.ipc 4567 -> 2034, read_bytes.jobs 12195 -> 12002, read_bytes.outside 9297 -> 9434, read_bytes.store 13471 -> 18955, read_calls.ipc 182 -> 77, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 127 -> 113, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 35 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 5623 -> 5635, write_bytes.outside 483 -> 640, write_bytes.store 4532 -> 2478, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 12011 -> 9417, cli_maxrss_kib 7456 -> 6640, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25183 -> 18824, fsync 67 -> 88, fullfsync 82 -> 40, malloc_bytes 9319581 -> 8154812, mallocs 32607 -> 23215, opens 511 -> 458, read_bytes.ipc 10766 -> 7383, read_bytes.jobs 20120 -> 17799, read_bytes.outside 18389 -> 18526, read_bytes.state 322 -> 174, read_bytes.store 26662 -> 34875, read_calls.ipc 516 -> 276, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.state 8 -> 4, read_calls.store 219 -> 207, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 117 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4213 -> 3651, write_bytes.jobs 12682 -> 12673, write_bytes.outside 504 -> 661, write_bytes.store 60530 -> 82567, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 39 -> 49
- cold start: cli_cpu_us 43020 -> 26550, cli_maxrss_kib 5088 -> 4528, frees 4325 -> 4378, malloc_bytes 1625667 -> 2212425, mallocs 5758 -> 5853
- restart: barrierfsync 0 -> 3, cli_cpu_us 72098 -> 27566, cli_maxrss_kib 5216 -> 4688, cli_processes 8 -> 4, frees 10806 -> 7756, fullfsync 38 -> 6, malloc_bytes 3649407 -> 3079730, mallocs 14690 -> 10961, opens 232 -> 87, read_bytes.jobs 6522 -> 3376, read_bytes.store 436756 -> 461476, read_calls.jobs 104 -> 52, read_calls.store 108 -> 114, renames 17 -> 4, syncs.jobs 32 -> 3, write_bytes.ipc 2732 -> 2072, write_bytes.jobs 2290 -> 487, write_calls.ipc 39 -> 31, write_calls.jobs 16 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 13832 | 14896 | 13096 | 14000 | 0.941 | 0.939 to 0.945 |
| b | 78 | 14376 | 15168 | 13384 | 13920 | 0.935 | 0.929 to 0.941 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5752 | 7072 | 5824 | 6784 | 0.995 | 0.988 to 0.998 |
| b | 78 | 7008 | 7648 | 6880 | 7152 | 0.961 | 0.960 to 0.965 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 11424 | 11936 | 10584 | 10832 | 0.925 | 0.919 to 0.934 |
| daemon b | 15024 | 16032 | 14128 | 14944 | 0.935 | 0.926 to 0.940 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 4304 | 4816 | 4128 | 4384 | 0.957 | 0.943 to 0.982 |
| daemon b | 8400 | 9824 | 7608 | 8640 | 0.906 | 0.883 to 0.932 |
