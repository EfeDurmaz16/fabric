### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 148.3 | 67.2 | 0.457 | 0.443 to 0.468 | faster |
| acp launch | 20 | 72.1 | 30.3 | 0.430 | 0.407 to 0.442 | faster |
| acp approve | 20 | 16.2 | 8.3 | 0.511 | 0.485 to 0.517 | faster |
| claude new | 20 | 143.7 | 62.7 | 0.433 | 0.423 to 0.447 | faster |
| claude launch | 20 | 93.5 | 89.9 | 0.968 | 0.938 to 1.011 | not shown |
| claude approve | 20 | 16.3 | 8.5 | 0.520 | 0.498 to 0.543 | faster |
| codex new | 20 | 141.7 | 62.4 | 0.440 | 0.427 to 0.463 | faster |
| codex launch | 20 | 102.9 | 97.2 | 0.949 | 0.913 to 0.970 | faster |
| codex approve | 20 | 15.8 | 8.2 | 0.521 | 0.494 to 0.542 | faster |
| ledger append | 20 | 16.3 | 8.5 | 0.519 | 0.496 to 0.542 | faster |
| cli version | 20 | 3.6 | 3.3 | 0.953 | 0.849 to 1.104 | not shown |
| cli ls | 20 | 4.7 | 3.9 | 0.869 | 0.822 to 0.927 | faster |
| acp suspend | 20 | 434.5 | 333.4 | 0.776 | 0.768 to 0.799 | faster |
| acp move | 20 | 166.5 | 103.2 | 0.623 | 0.589 to 0.647 | faster |
| acp resume | 20 | 723.7 | 290.9 | 0.391 | 0.321 to 0.483 | faster |
| acp pull | 20 | 1299.5 | 695.0 | 0.532 | 0.481 to 0.563 | faster |
| claude suspend | 20 | 493.8 | 331.6 | 0.680 | 0.660 to 0.695 | faster |
| claude move | 20 | 168.3 | 106.7 | 0.634 | 0.614 to 0.669 | faster |
| claude resume | 20 | 939.9 | 388.8 | 0.434 | 0.355 to 0.523 | faster |
| claude pull | 20 | 1599.8 | 835.2 | 0.513 | 0.479 to 0.582 | faster |
| codex suspend | 20 | 754.7 | 600.4 | 0.790 | 0.754 to 0.815 | faster |
| codex move | 20 | 168.7 | 107.9 | 0.633 | 0.596 to 0.657 | faster |
| codex resume | 20 | 1165.7 | 605.4 | 0.569 | 0.471 to 0.590 | faster |
| codex pull | 20 | 2117.0 | 1303.3 | 0.611 | 0.582 to 0.679 | faster |
| cold start | 20 | 58.5 | 54.7 | 0.945 | 0.924 to 0.962 | faster |
| restart | 20 | 410.7 | 56.4 | 0.142 | 0.127 to 0.184 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38921 -> 37863 | 8064 -> 6816 | 12.1 -> 9.6 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1185 -> 295 | 4096 -> 4067 | 6912 -> 5968 | 12.0 -> 9.1 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1173 -> 784 | 5088 -> 4096 | 10.6 -> 7.1 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1637 -> 1056 | 5120 -> 4176 | 11.8 -> 7.1 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1104 -> 786 | 3494 -> 3126 | 6960 -> 5616 | 12.3 -> 10.0 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1160 -> 783 | 5072 -> 4096 | 9.0 -> 7.5 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1634 -> 1056 | 5168 -> 4192 | 10.4 -> 9.3 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3821 -> 3472 | 7040 -> 5632 | 12.6 -> 9.7 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1187 -> 783 | 5104 -> 4096 | 10.2 -> 7.7 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1168 -> 784 | 5040 -> 4112 | 11.2 -> 7.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 244 | 3168 -> 2992 | 6.9 -> 6.7 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1505 -> 1131 | 5232 -> 4240 | 9.4 -> 7.0 |
| acp suspend | 36 -> 35 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 39983 -> 40962 | 14970 -> 12291 | 5264 -> 4272 | 9.6 -> 7.7 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5245 -> 2447 | 8321 -> 4732 | 5664 -> 4544 | 10.7 -> 7.7 |
| acp resume | 32 -> 14 | 24 -> 13 | 51 -> 61 | 167 -> 167 | 16142 -> 21702 | 11149 -> 8209 | 6960 -> 6048 | 13.0 -> 9.7 |
| acp pull | 111 -> 56 | 81 -> 41 | 59 -> 77 | 1376 -> 1376 | 33096 -> 39512 | 30353 -> 21495 | 7152 -> 6176 | 12.4 -> 10.1 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13255 -> 14175 | 19213 -> 15829 | 5328 -> 4240 | 11.7 -> 7.9 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5258 -> 2570 | 8205 -> 4938 | 5696 -> 4576 | 10.6 -> 10.2 |
| claude resume | 34 -> 16 | 24 -> 13 | 55 -> 65 | 148 -> 148 | 21647 -> 27042 | 11063 -> 8172 | 7296 -> 6064 | 13.6 -> 10.9 |
| claude pull | 114 -> 61 | 83 -> 41 | 63 -> 83 | 1338 -> 1338 | 41567 -> 47615 | 36776 -> 27045 | 7440 -> 6192 | 14.7 -> 12.1 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 14408 -> 15311 | 16205 -> 12038 | 5280 -> 4256 | 11.6 -> 7.8 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5320 -> 2648 | 8204 -> 4958 | 5632 -> 4528 | 11.9 -> 7.8 |
| codex resume | 35 -> 17 | 24 -> 13 | 59 -> 69 | 148 -> 148 | 25750 -> 31034 | 11190 -> 8295 | 7408 -> 6112 | 14.2 -> 10.6 |
| codex pull | 117 -> 64 | 82 -> 40 | 67 -> 88 | 1338 -> 1338 | 46834 -> 52756 | 32599 -> 22748 | 7536 -> 6256 | 16.5 -> 11.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5727 -> 4224 | 5088 -> 4128 | 42.0 -> 31.3 |
| restart | 0 -> 0 | 38 -> 6 | 0 -> 0 | 463 -> 463 | 497505 -> 510839 | 14626 -> 9282 | 5344 -> 4288 | 85.3 -> 29.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 12135 -> 9556, cli_maxrss_kib 8064 -> 6816, cmd:defaults 2 -> 0, frees 28103 -> 27211, fullfsync 8 -> 3, malloc_bytes 13532106 -> 13331465, mallocs 38921 -> 37863, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 12008 -> 9099, cli_maxrss_kib 6912 -> 5968, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3338 -> 3325, fullfsync 15 -> 5, malloc_bytes 710343 -> 678362, mallocs 4096 -> 4067, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1011 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1033 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 10605 -> 7073, cli_maxrss_kib 5088 -> 4096, frees 1021 -> 659, fullfsync 3 -> 1, malloc_bytes 317570 -> 254238, mallocs 1173 -> 784, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 11793 -> 7130, cli_maxrss_kib 5120 -> 4176, cmd:defaults 2 -> 0, frees 1381 -> 853, fullfsync 8 -> 3, malloc_bytes 383391 -> 293965, mallocs 1637 -> 1056, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 12271 -> 9990, cli_maxrss_kib 6960 -> 5616, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3106 -> 2776, fullfsync 15 -> 7, malloc_bytes 596887 -> 518877, mallocs 3494 -> 3126, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 930 -> 612, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 933 -> 930, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 9002 -> 7471, cli_maxrss_kib 5072 -> 4096, frees 1009 -> 659, fullfsync 3 -> 1, malloc_bytes 316546 -> 253982, mallocs 1160 -> 783, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 10392 -> 9310, cli_maxrss_kib 5168 -> 4192, cmd:defaults 2 -> 0, frees 1381 -> 853, fullfsync 8 -> 3, malloc_bytes 382931 -> 293849, mallocs 1634 -> 1056, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 12559 -> 9650, cli_maxrss_kib 7040 -> 5632, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 3414 -> 3103, fullfsync 17 -> 8, malloc_bytes 688798 -> 609363, mallocs 3821 -> 3472, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 13 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 10194 -> 7712, cli_maxrss_kib 5104 -> 4096, frees 1037 -> 659, fullfsync 3 -> 1, malloc_bytes 317826 -> 253982, mallocs 1187 -> 783, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 11243 -> 7498, cli_maxrss_kib 5040 -> 4112, frees 1017 -> 659, fullfsync 3 -> 1, malloc_bytes 316802 -> 254238, mallocs 1168 -> 784, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 6853 -> 6741, cli_maxrss_kib 3168 -> 2992, frees 176 -> 231, malloc_bytes 87612 -> 105993, mallocs 189 -> 244
- cli ls: cli_cpu_us 9370 -> 6981, cli_maxrss_kib 5232 -> 4240, frees 1314 -> 964, malloc_bytes 361841 -> 299902, mallocs 1505 -> 1131, opens 13 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 9626 -> 7653, cli_maxrss_kib 5264 -> 4272, cmd:git bundle 3 -> 2, frees 10326 -> 9259, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3558145 -> 3443837, mallocs 14970 -> 12291, opens 142 -> 128, read_bytes.ipc 5269 -> 5199, read_bytes.jobs 2302 -> 2181, read_bytes.store 37594 -> 38694, read_calls.ipc 187 -> 177, read_calls.jobs 24 -> 22, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3599 -> 3586, write_bytes.store 95684 -> 95917
- acp move: barrierfsync 0 -> 4, cli_cpu_us 10748 -> 7679, cli_maxrss_kib 5664 -> 4544, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6834 -> 3770, fullfsync 15 -> 7, malloc_bytes 2007765 -> 1613842, mallocs 8321 -> 4732, opens 73 -> 15, read_bytes.ipc 468 -> 110, read_bytes.jobs 4135 -> 119, read_bytes.store 1110 -> 2328, read_calls.ipc 124 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2697 -> 1913, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 12952 -> 9745, cli_maxrss_kib 6960 -> 6048, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 8957 -> 6530, fsync 51 -> 61, fullfsync 24 -> 13, malloc_bytes 4347010 -> 3569296, mallocs 11149 -> 8209, opens 212 -> 219, read_bytes.ipc 4937 -> 2406, read_bytes.jobs 3556 -> 3367, read_bytes.outside 554 -> 691, read_bytes.state 270 -> 122, read_bytes.store 12316 -> 18213, read_calls.ipc 173 -> 67, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.state 8 -> 4, read_calls.store 125 -> 111, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 32 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.jobs 3559 -> 3560, write_bytes.outside 483 -> 640, write_bytes.store 4534 -> 2477, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 12442 -> 10097, cli_maxrss_kib 7152 -> 6176, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23354 -> 17242, fsync 59 -> 77, fullfsync 81 -> 41, malloc_bytes 8801507 -> 7422007, mallocs 30353 -> 21495, opens 417 -> 366, read_bytes.ipc 11102 -> 7683, read_bytes.jobs 8530 -> 6231, read_bytes.outside 1632 -> 1769, read_bytes.store 24392 -> 33107, read_calls.ipc 498 -> 257, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 111 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4624 -> 4084, write_bytes.jobs 8385 -> 8398, write_bytes.outside 747 -> 904, write_bytes.store 59964 -> 57292, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 11740 -> 7921, cli_maxrss_kib 5328 -> 4240, cmd:git bundle 3 -> 2, frees 13976 -> 12579, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3661770 -> 3511072, mallocs 19213 -> 15829, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 3676 -> 3555, read_bytes.store 9492 -> 10533, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4753 -> 4765, write_bytes.store 54746 -> 63187, write_calls.store 33 -> 37
- claude move: barrierfsync 0 -> 4, cli_cpu_us 10573 -> 10165, cli_maxrss_kib 5696 -> 4576, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6750 -> 3947, fullfsync 15 -> 7, malloc_bytes 1994671 -> 1648236, mallocs 8205 -> 4938, opens 73 -> 15, read_bytes.ipc 468 -> 154, read_bytes.jobs 4054 -> 82, read_bytes.store 1204 -> 2488, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1896, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 13583 -> 10942, cli_maxrss_kib 7296 -> 6064, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9113 -> 6731, fsync 55 -> 65, fullfsync 24 -> 13, malloc_bytes 4363780 -> 3584168, mallocs 11063 -> 8172, opens 250 -> 252, read_bytes.ipc 4417 -> 1884, read_bytes.jobs 8582 -> 8392, read_bytes.outside 3026 -> 3163, read_bytes.store 12943 -> 18528, read_calls.ipc 178 -> 73, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.store 125 -> 111, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 34 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 4701 -> 4707, write_bytes.outside 483 -> 640, write_bytes.store 4529 -> 2479, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 14656 -> 12125, cli_maxrss_kib 7440 -> 6192, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 27977 -> 21666, fsync 63 -> 83, fullfsync 83 -> 41, malloc_bytes 9279824 -> 7777086, mallocs 36776 -> 27045, opens 458 -> 409, read_bytes.ipc 10602 -> 7227, read_bytes.jobs 15734 -> 13417, read_bytes.outside 5735 -> 5872, read_bytes.store 25659 -> 34024, read_calls.ipc 507 -> 268, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 217 -> 205, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 114 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4278 -> 3725, write_bytes.jobs 10906 -> 10890, write_bytes.outside 617 -> 774, write_bytes.store 84845 -> 65705, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 51 -> 41
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 11583 -> 7834, cli_maxrss_kib 5280 -> 4256, cmd:git bundle 3 -> 2, frees 11686 -> 9796, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3729127 -> 3578091, mallocs 16205 -> 12038, opens 152 -> 138, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 4494 -> 4373, read_bytes.store 9827 -> 10851, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5634 -> 5627, write_bytes.store 88070 -> 88267
- codex move: barrierfsync 0 -> 4, cli_cpu_us 11857 -> 7838, cli_maxrss_kib 5632 -> 4528, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6759 -> 3964, fullfsync 15 -> 7, malloc_bytes 1994387 -> 1648447, mallocs 8204 -> 4958, opens 73 -> 15, read_bytes.ipc 468 -> 146, read_bytes.jobs 4086 -> 106, read_bytes.store 1234 -> 2542, read_calls.ipc 124 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2361 -> 1888, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 14172 -> 10560, cli_maxrss_kib 7408 -> 6112, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9266 -> 6875, fsync 59 -> 69, fullfsync 24 -> 13, malloc_bytes 4468195 -> 3690936, mallocs 11190 -> 8295, opens 292 -> 298, read_bytes.ipc 4567 -> 2034, read_bytes.jobs 12199 -> 12003, read_bytes.outside 9297 -> 9434, read_bytes.store 13429 -> 18909, read_calls.ipc 182 -> 77, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 127 -> 113, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 24 -> 10, spawn:ps 3 -> 1, spawns 35 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 5626 -> 5623, write_bytes.outside 483 -> 640, write_bytes.store 4531 -> 2479, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 16464 -> 10967, cli_maxrss_kib 7536 -> 6256, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25176 -> 18394, fsync 67 -> 88, fullfsync 82 -> 40, malloc_bytes 9316394 -> 7951478, mallocs 32599 -> 22748, opens 509 -> 458, read_bytes.ipc 10765 -> 7383, read_bytes.jobs 20109 -> 17801, read_bytes.outside 18389 -> 18526, read_bytes.store 26551 -> 34781, read_calls.ipc 517 -> 277, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.store 219 -> 207, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 58 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 117 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4212 -> 3651, write_bytes.jobs 12668 -> 12674, write_bytes.outside 504 -> 661, write_bytes.store 85227 -> 90792, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 51 -> 53
- cold start: cli_cpu_us 42007 -> 31275, cli_maxrss_kib 5088 -> 4128, frees 4292 -> 2881, malloc_bytes 1625668 -> 1396847, mallocs 5727 -> 4224
- restart: barrierfsync 0 -> 3, cli_cpu_us 85302 -> 29666, cli_maxrss_kib 5344 -> 4288, cli_processes 8 -> 4, frees 10835 -> 6218, fullfsync 38 -> 6, malloc_bytes 3644353 -> 2222598, mallocs 14626 -> 9282, opens 232 -> 87, read_bytes.jobs 6522 -> 3376, read_bytes.store 473836 -> 490316, read_calls.jobs 104 -> 52, read_calls.store 117 -> 121, renames 17 -> 4, syncs.jobs 32 -> 3, write_bytes.ipc 2936 -> 2072, write_bytes.jobs 2290 -> 487, write_calls.ipc 42 -> 31, write_calls.jobs 16 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 13856 | 15600 | 12552 | 13840 | 0.892 | 0.889 to 0.898 |
| b | 78 | 14440 | 15216 | 12912 | 13232 | 0.889 | 0.885 to 0.891 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5576 | 6800 | 5488 | 6224 | 0.964 | 0.962 to 0.968 |
| b | 78 | 6648 | 7232 | 6472 | 6752 | 0.953 | 0.949 to 0.956 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 11376 | 11936 | 9912 | 10192 | 0.874 | 0.869 to 0.879 |
| daemon b | 15312 | 15872 | 13696 | 14112 | 0.887 | 0.880 to 0.892 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 4248 | 4832 | 3904 | 4192 | 0.928 | 0.910 to 0.941 |
| daemon b | 8072 | 9552 | 7256 | 8224 | 0.892 | 0.870 to 0.914 |
