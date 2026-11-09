### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 180.4 | 44.6 | 0.255 | 0.224 to 0.310 | faster |
| acp launch | 15 | 23.5 | 15.3 | 0.661 | 0.640 to 0.672 | faster |
| acp approve | 15 | 6.0 | 4.7 | 0.845 | 0.787 to 0.870 | faster |
| claude new | 15 | 254.5 | 39.3 | 0.190 | 0.152 to 0.233 | faster |
| claude launch | 15 | 169.3 | 176.8 | 1.045 | 0.739 to 1.670 | not shown, median above 1 |
| claude approve | 15 | 8.8 | 6.8 | 0.794 | 0.509 to 0.923 | faster |
| codex new | 15 | 212.0 | 40.7 | 0.202 | 0.151 to 0.256 | faster |
| codex launch | 15 | 235.4 | 213.3 | 0.975 | 0.551 to 1.249 | not shown |
| codex approve | 15 | 7.6 | 5.3 | 0.794 | 0.553 to 0.970 | faster |
| ledger append | 15 | 5.6 | 4.9 | 0.858 | 0.835 to 0.895 | faster |
| cli version | 15 | 2.8 | 2.7 | 0.962 | 0.908 to 1.059 | not shown |
| cli ls | 15 | 3.3 | 3.2 | 0.954 | 0.931 to 0.987 | faster |
| acp suspend | 15 | 453.0 | 431.3 | 0.969 | 0.828 to 1.005 | not shown |
| acp move | 15 | 270.2 | 237.6 | 0.772 | 0.751 to 0.937 | faster |
| acp resume | 15 | 838.6 | 318.3 | 0.382 | 0.322 to 0.442 | faster |
| acp pull | 15 | 1537.5 | 987.2 | 0.623 | 0.561 to 0.669 | faster |
| claude suspend | 15 | 515.8 | 409.7 | 0.718 | 0.616 to 0.866 | faster |
| claude move | 15 | 279.6 | 234.8 | 0.841 | 0.770 to 0.947 | faster |
| claude resume | 15 | 1234.1 | 644.9 | 0.543 | 0.486 to 0.606 | faster |
| claude pull | 15 | 2262.3 | 1298.4 | 0.582 | 0.568 to 0.589 | faster |
| codex suspend | 15 | 639.6 | 456.7 | 0.700 | 0.634 to 0.747 | faster |
| codex move | 15 | 294.5 | 243.6 | 0.859 | 0.639 to 0.952 | faster |
| codex resume | 15 | 1323.2 | 601.0 | 0.441 | 0.405 to 0.489 | faster |
| codex pull | 15 | 2246.2 | 1337.9 | 0.575 | 0.568 to 0.641 | faster |
| cold start | 15 | 51.9 | 80.6 | 0.898 | 0.629 to 1.622 | not shown |
| restart | 15 | 128.7 | 61.6 | 0.531 | 0.371 to 0.634 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 37860 | 7040 -> 6384 | 4017 -> 3841 | 8.4 -> 8.4 | 0.0 -> 0.0 |
| acp launch | 4 -> 2 | 15 -> 5 | 0 -> 0 | 135 -> 135 | 1184 -> 295 | 3648 -> 4046 | 6944 -> 6528 | 3361 -> 3345 | 7.7 -> 8.0 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 782 | 5184 -> 4688 | 3073 -> 2833 | 6.0 -> 6.0 | 0.0 -> 0.0 |
| claude new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1054 | 5312 -> 4944 | 3137 -> 3057 | 6.6 -> 6.0 | 0.0 -> 0.0 |
| claude launch | 4 -> 2 | 15 -> 7 | 0 -> 0 | 116 -> 116 | 1099 -> 788 | 3058 -> 3121 | 7040 -> 6192 | 3425 -> 3249 | 8.1 -> 8.0 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 781 | 5168 -> 4752 | 3041 -> 2897 | 6.2 -> 5.9 | 0.0 -> 0.0 |
| codex new | 3 -> 1 | 8 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1054 | 5328 -> 4944 | 3153 -> 3057 | 6.4 -> 5.9 | 0.0 -> 0.0 |
| codex launch | 4 -> 2 | 17 -> 8 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3365 -> 3463 | 7104 -> 6288 | 3393 -> 3281 | 8.4 -> 7.5 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 781 | 5184 -> 4800 | 3057 -> 2945 | 6.6 -> 6.4 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 782 | 5184 -> 4768 | 3073 -> 2929 | 6.6 -> 6.1 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 244 | 3552 -> 3440 | 2657 -> 2545 | 5.7 -> 5.9 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1225 | 5168 -> 4928 | 3041 -> 3041 | 6.3 -> 6.9 | 0.0 -> 0.0 |
| acp suspend | 35 -> 34 | 39 -> 20 | 9 -> 14 | 522 -> 522 | 48209 -> 49247 | 14482 -> 12238 | 5632 -> 5104 | 3329 -> 3105 | 6.5 -> 6.7 | 0.0 -> 0.0 |
| acp move | 42 -> 8 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5212 -> 2461 | 7922 -> 4724 | 5776 -> 5280 | 3345 -> 3153 | 7.6 -> 6.7 | 0.0 -> 0.0 |
| acp resume | 36 -> 14 | 24 -> 13 | 51 -> 67 | 167 -> 167 | 19793 -> 26555 | 11620 -> 7950 | 7056 -> 6592 | 3425 -> 3361 | 8.8 -> 7.7 | 0.0 -> 0.0 |
| acp pull | 115 -> 56 | 81 -> 41 | 59 -> 83 | 1376 -> 1376 | 36866 -> 44817 | 30744 -> 21632 | 7184 -> 6656 | 3521 -> 3361 | 8.7 -> 9.3 | 0.0 -> 0.0 |
| claude suspend | 35 -> 34 | 41 -> 20 | 8 -> 15 | 522 -> 522 | 13860 -> 14736 | 20442 -> 17494 | 5584 -> 5088 | 3313 -> 3105 | 6.5 -> 6.2 | 0.0 -> 0.0 |
| claude move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5264 -> 2570 | 7854 -> 4945 | 5776 -> 5280 | 3361 -> 3169 | 6.7 -> 6.7 | 0.0 -> 0.0 |
| claude resume | 38 -> 16 | 24 -> 13 | 55 -> 71 | 148 -> 148 | 27895 -> 34036 | 11654 -> 8322 | 7424 -> 6608 | 3521 -> 3409 | 8.7 -> 8.8 | 0.0 -> 0.0 |
| claude pull | 118 -> 61 | 83 -> 41 | 63 -> 89 | 1338 -> 1338 | 48412 -> 55302 | 38965 -> 28818 | 7472 -> 6704 | 3537 -> 3441 | 10.4 -> 9.0 | 0.0 -> 0.0 |
| codex suspend | 37 -> 36 | 40 -> 19 | 8 -> 16 | 522 -> 522 | 15219 -> 16123 | 15799 -> 12062 | 5504 -> 5056 | 3217 -> 3057 | 6.1 -> 6.0 | 0.0 -> 0.0 |
| codex move | 42 -> 11 | 15 -> 7 | 0 -> 0 | 520 -> 520 | 5341 -> 2654 | 7864 -> 4946 | 5776 -> 5296 | 3361 -> 3169 | 6.9 -> 6.5 | 0.0 -> 0.0 |
| codex resume | 39 -> 17 | 24 -> 13 | 59 -> 75 | 148 -> 148 | 32941 -> 38998 | 11753 -> 8393 | 7392 -> 6656 | 3521 -> 3393 | 9.5 -> 9.3 | 0.0 -> 0.0 |
| codex pull | 121 -> 64 | 82 -> 40 | 67 -> 94 | 1338 -> 1338 | 54986 -> 61516 | 33130 -> 22617 | 7424 -> 6768 | 3521 -> 3441 | 8.9 -> 9.3 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3258 -> 3451 | 5120 -> 4800 | 3073 -> 2929 | 20.9 -> 20.6 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 6 | 0 -> 0 | 463 -> 463 | 759627 -> 759875 | 8892 -> 8694 | 5264 -> 4896 | 3137 -> 3009 | 30.2 -> 20.9 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 4, cli_cpu_us 8438 -> 8413, cli_footprint_kib 4017 -> 3841, cli_maxrss_kib 7040 -> 6384, cmd:defaults 2 -> 0, frees 27687 -> 27210, fullfsync 8 -> 3, malloc_bytes 13401480 -> 13275665, mallocs 38479 -> 37860, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_cpu_us 7749 -> 7987, cli_footprint_kib 3361 -> 3345, cli_maxrss_kib 6944 -> 6528, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 2922 -> 3313, fullfsync 15 -> 5, malloc_bytes 634960 -> 676961, mallocs 3648 -> 4046, opens 40 -> 26, read_bytes.ipc 500 -> 494, read_bytes.jobs 1010 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 9, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1032 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: barrierfsync 0 -> 1, cli_cpu_us 6028 -> 6043, cli_footprint_kib 3073 -> 2833, cli_maxrss_kib 5184 -> 4688, frees 606 -> 659, fullfsync 3 -> 1, malloc_bytes 242240 -> 254118, mallocs 732 -> 782, opens 3 -> 1, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 4, cli_cpu_us 6626 -> 6000, cli_footprint_kib 3137 -> 3057, cli_maxrss_kib 5312 -> 4944, cmd:defaults 2 -> 0, frees 978 -> 853, fullfsync 8 -> 3, malloc_bytes 308809 -> 293785, mallocs 1204 -> 1054, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_cpu_us 8114 -> 8006, cli_footprint_kib 3425 -> 3249, cli_maxrss_kib 7040 -> 6192, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 2703 -> 2780, fullfsync 15 -> 7, malloc_bytes 522140 -> 518221, mallocs 3058 -> 3121, opens 43 -> 33, read_bytes.ipc 169 -> 163, read_bytes.jobs 925 -> 614, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, spawn:claude 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 12, write_bytes.ipc 692 -> 693, write_bytes.jobs 928 -> 932, write_calls.ipc 11 -> 12
- claude approve: barrierfsync 0 -> 1, cli_cpu_us 6163 -> 5851, cli_footprint_kib 3041 -> 2897, cli_maxrss_kib 5168 -> 4752, frees 606 -> 659, fullfsync 3 -> 1, malloc_bytes 241984 -> 253862, mallocs 731 -> 781, opens 3 -> 1, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 4, cli_cpu_us 6426 -> 5924, cli_footprint_kib 3153 -> 3057, cli_maxrss_kib 5328 -> 4944, cmd:defaults 2 -> 0, frees 978 -> 853, fullfsync 8 -> 3, malloc_bytes 308753 -> 293793, mallocs 1204 -> 1054, opens 11 -> 8, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_cpu_us 8351 -> 7549, cli_footprint_kib 3393 -> 3281, cli_maxrss_kib 7104 -> 6288, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 2992 -> 3102, fullfsync 17 -> 8, malloc_bytes 612960 -> 608827, mallocs 3365 -> 3463, opens 46 -> 34, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 13 -> 8, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:dash 0 -> 1, spawn:ps 3 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 14, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: barrierfsync 0 -> 1, cli_cpu_us 6559 -> 6374, cli_footprint_kib 3057 -> 2945, cli_maxrss_kib 5184 -> 4800, frees 606 -> 659, fullfsync 3 -> 1, malloc_bytes 241984 -> 253862, mallocs 731 -> 781, opens 3 -> 1, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_cpu_us 6561 -> 6090, cli_footprint_kib 3073 -> 2929, cli_maxrss_kib 5184 -> 4768, frees 606 -> 659, fullfsync 3 -> 1, malloc_bytes 242240 -> 254118, mallocs 732 -> 782, opens 3 -> 1, syncs.jobs 2 -> 1
- cli version: cli_cpu_us 5747 -> 5927, cli_footprint_kib 2657 -> 2545, cli_maxrss_kib 3552 -> 3440, frees 176 -> 231, malloc_bytes 87612 -> 105993, mallocs 189 -> 244
- cli ls: cli_cpu_us 6307 -> 6875, cli_maxrss_kib 5168 -> 4928, frees 995 -> 1048, malloc_bytes 297439 -> 309430, mallocs 1173 -> 1225, opens 16 -> 0
- acp suspend: barrierfsync 0 -> 10, cli_cpu_us 6494 -> 6701, cli_footprint_kib 3329 -> 3105, cli_maxrss_kib 5632 -> 5104, cmd:git bundle 3 -> 2, frees 9869 -> 9212, fsync 9 -> 14, fullfsync 39 -> 20, malloc_bytes 3465086 -> 3425534, mallocs 14482 -> 12238, opens 140 -> 126, read_bytes.ipc 5214 -> 5144, read_bytes.jobs 2302 -> 2181, read_bytes.store 45820 -> 46979, read_calls.ipc 184 -> 173, read_calls.jobs 24 -> 22, read_calls.store 76 -> 77, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 22 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3586 -> 3592, write_bytes.store 95682 -> 112461, write_calls.store 53 -> 61
- acp move: barrierfsync 0 -> 4, cli_cpu_us 7637 -> 6659, cli_footprint_kib 3345 -> 3153, cli_maxrss_kib 5776 -> 5280, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6483 -> 3770, fullfsync 15 -> 7, malloc_bytes 1975320 -> 1612987, mallocs 7922 -> 4724, opens 73 -> 15, read_bytes.ipc 436 -> 78, read_bytes.jobs 4103 -> 119, read_bytes.store 1109 -> 2342, read_calls.ipc 122 -> 16, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 8, syncs.jobs 9 -> 5, write_bytes.ipc 2665 -> 1881, write_calls.ipc 31 -> 22
- acp resume: barrierfsync 0 -> 9, cli_cpu_us 8849 -> 7669, cli_footprint_kib 3425 -> 3361, cli_maxrss_kib 7056 -> 6592, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 14 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9249 -> 6525, fsync 51 -> 67, fullfsync 24 -> 13, malloc_bytes 4763334 -> 3477624, mallocs 11620 -> 7950, opens 224 -> 235, read_bytes.ipc 5865 -> 2406, read_bytes.jobs 3561 -> 3363, read_bytes.outside 554 -> 691, read_bytes.store 16110 -> 23070, read_calls.ipc 200 -> 59, read_calls.jobs 28 -> 24, read_calls.outside 13 -> 15, read_calls.store 155 -> 123, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 28 -> 10, spawn:ps 3 -> 1, spawns 36 -> 14, syncs.jobs 31 -> 29, syncs.outside 43 -> 59, unlinks 25 -> 20, write_bytes.ipc 2046 -> 2141, write_bytes.jobs 3554 -> 3557, write_bytes.outside 483 -> 640, write_bytes.store 6151 -> 2479, write_calls.ipc 28 -> 30, write_calls.outside 11 -> 12, write_calls.store 7 -> 4
- acp pull: barrierfsync 0 -> 24, cli_cpu_us 8658 -> 9254, cli_footprint_kib 3521 -> 3361, cli_maxrss_kib 7184 -> 6656, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:defaults 6 -> 0, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 19 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 23577 -> 17239, fsync 59 -> 83, fullfsync 81 -> 41, malloc_bytes 9194294 -> 7402077, mallocs 30744 -> 21632, opens 425 -> 386, read_bytes.ipc 11996 -> 7651, read_bytes.jobs 8537 -> 6232, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 28155 -> 38263, read_calls.ipc 528 -> 249, read_calls.jobs 59 -> 49, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 247 -> 217, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:fabric-fake-agent 2 -> 1, spawn:git 62 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 115 -> 56, syncs.jobs 64 -> 53, syncs.outside 48 -> 67, unlinks 46 -> 40, write_bytes.ipc 4592 -> 4052, write_bytes.jobs 8389 -> 8397, write_bytes.outside 747 -> 904, write_bytes.store 69813 -> 90291, write_calls.ipc 84 -> 68, write_calls.outside 12 -> 13, write_calls.store 45 -> 53
- claude suspend: barrierfsync 0 -> 10, cli_cpu_us 6502 -> 6154, cli_footprint_kib 3313 -> 3105, cli_maxrss_kib 5584 -> 5088, cmd:git bundle 3 -> 2, frees 14812 -> 13829, fsync 8 -> 15, fullfsync 41 -> 20, malloc_bytes 3696055 -> 3587801, mallocs 20442 -> 17494, opens 145 -> 131, read_bytes.ipc 5237 -> 5167, read_bytes.jobs 4234 -> 4113, read_bytes.store 9539 -> 10536, read_calls.ipc 185 -> 175, read_calls.jobs 27 -> 25, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, syncs.jobs 24 -> 20, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 5316 -> 5306, write_bytes.store 95960 -> 63177, write_calls.store 53 -> 37
- claude move: barrierfsync 0 -> 4, cli_cpu_us 6713 -> 6703, cli_footprint_kib 3361 -> 3169, cli_maxrss_kib 5776 -> 5280, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:codex 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6421 -> 3954, fullfsync 15 -> 7, malloc_bytes 1966990 -> 1647754, mallocs 7854 -> 4945, opens 73 -> 15, read_bytes.ipc 436 -> 122, read_bytes.jobs 4054 -> 82, read_bytes.store 1210 -> 2488, read_calls.ipc 122 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:codex 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2329 -> 1864, write_calls.ipc 29 -> 22
- claude resume: barrierfsync 0 -> 9, cli_cpu_us 8697 -> 8771, cli_footprint_kib 3521 -> 3409, cli_maxrss_kib 7424 -> 6608, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 14 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9516 -> 6840, fsync 55 -> 71, fullfsync 24 -> 13, malloc_bytes 4793609 -> 3592908, mallocs 11654 -> 8322, opens 258 -> 270, read_bytes.ipc 5344 -> 1884, read_bytes.jobs 10817 -> 10622, read_bytes.outside 4142 -> 4279, read_bytes.store 16956 -> 23292, read_calls.ipc 206 -> 65, read_calls.jobs 30 -> 26, read_calls.outside 16 -> 18, read_calls.store 155 -> 123, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 28 -> 10, spawn:ps 3 -> 1, spawns 38 -> 16, syncs.jobs 35 -> 33, syncs.outside 43 -> 59, unlinks 27 -> 22, write_bytes.ipc 1852 -> 1844, write_bytes.jobs 5255 -> 5262, write_bytes.outside 483 -> 640, write_bytes.store 6149 -> 2480, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 7 -> 4
- claude pull: barrierfsync 0 -> 24, cli_cpu_us 10411 -> 9003, cli_footprint_kib 3537 -> 3441, cli_maxrss_kib 7472 -> 6704, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 8 -> 6, cmd:codex 3 -> 0, cmd:dash holding claude 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 19 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 29585 -> 22982, fsync 63 -> 89, fullfsync 83 -> 41, malloc_bytes 9792966 -> 7858925, mallocs 38965 -> 28818, opens 468 -> 427, read_bytes.ipc 11496 -> 7195, read_bytes.jobs 18508 -> 16210, read_bytes.outside 7409 -> 7546, read_bytes.state 174 -> 322, read_bytes.store 29730 -> 38770, read_calls.ipc 537 -> 260, read_calls.jobs 64 -> 54, read_calls.outside 33 -> 35, read_calls.state 4 -> 8, read_calls.store 247 -> 217, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 8 -> 6, spawn:codex 3 -> 0, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 62 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 118 -> 61, syncs.jobs 70 -> 59, syncs.outside 48 -> 67, unlinks 50 -> 44, write_bytes.ipc 4246 -> 3693, write_bytes.jobs 11993 -> 12018, write_bytes.outside 617 -> 774, write_bytes.store 102966 -> 90429, write_calls.ipc 80 -> 67, write_calls.outside 12 -> 13, write_calls.store 61 -> 53
- codex suspend: barrierfsync 0 -> 9, cli_cpu_us 6111 -> 6041, cli_footprint_kib 3217 -> 3057, cli_maxrss_kib 5504 -> 5056, cmd:git bundle 3 -> 2, frees 11311 -> 9825, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3675839 -> 3610918, mallocs 15799 -> 12062, opens 152 -> 138, read_bytes.ipc 5251 -> 5181, read_bytes.jobs 5285 -> 5164, read_bytes.store 9847 -> 10872, read_calls.ipc 190 -> 180, read_calls.jobs 28 -> 26, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, syncs.jobs 26 -> 22, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 6442 -> 6405, write_bytes.store 88084 -> 88279
- codex move: barrierfsync 0 -> 4, cli_cpu_us 6862 -> 6491, cli_footprint_kib 3361 -> 3169, cli_maxrss_kib 5776 -> 5296, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:defaults 4 -> 0, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:python3 4 -> 0, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 6435 -> 3955, fullfsync 15 -> 7, malloc_bytes 1967012 -> 1647352, mallocs 7864 -> 4946, opens 73 -> 15, read_bytes.ipc 436 -> 114, read_bytes.jobs 4106 -> 106, read_bytes.store 1235 -> 2548, read_calls.ipc 122 -> 20, read_calls.jobs 10 -> 2, read_calls.store 10 -> 20, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:defaults 4 -> 0, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:python3 4 -> 0, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 42 -> 11, syncs.jobs 9 -> 5, write_bytes.ipc 2329 -> 1856, write_calls.ipc 29 -> 22
- codex resume: barrierfsync 0 -> 9, cli_cpu_us 9520 -> 9338, cli_footprint_kib 3521 -> 3393, cli_maxrss_kib 7392 -> 6656, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:defaults 2 -> 0, cmd:git bundle 14 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, frees 9650 -> 6947, fsync 59 -> 75, fullfsync 24 -> 13, malloc_bytes 4896245 -> 3696122, mallocs 11753 -> 8393, opens 302 -> 314, read_bytes.ipc 5494 -> 2034, read_bytes.jobs 15364 -> 15171, read_bytes.outside 12461 -> 12598, read_bytes.store 17455 -> 23705, read_calls.ipc 209 -> 68, read_calls.jobs 29 -> 25, read_calls.outside 15 -> 17, read_calls.store 157 -> 125, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:defaults 2 -> 0, spawn:git 28 -> 10, spawn:ps 3 -> 1, spawns 39 -> 17, syncs.jobs 39 -> 37, syncs.outside 43 -> 59, unlinks 30 -> 25, write_bytes.ipc 1786 -> 1778, write_bytes.jobs 6428 -> 6430, write_bytes.outside 483 -> 640, write_bytes.store 6151 -> 2478, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12, write_calls.store 7 -> 4
- codex pull: barrierfsync 0 -> 23, cli_cpu_us 8883 -> 9309, cli_footprint_kib 3521 -> 3441, cli_maxrss_kib 7424 -> 6768, cmd:bun 2 -> 0, cmd:cargo 2 -> 0, cmd:claude 3 -> 0, cmd:codex 9 -> 7, cmd:dash holding codex 0 -> 1, cmd:defaults 6 -> 0, cmd:git bundle 19 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:npm 2 -> 0, cmd:pnpm 4 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:python3 6 -> 2, cmd:rustc 2 -> 0, cmd:sw_vers 2 -> 0, cmd:uv 2 -> 0, cmd:xcrun 2 -> 0, cmd:yarn 2 -> 0, frees 25530 -> 18465, fsync 67 -> 94, fullfsync 82 -> 40, malloc_bytes 9729836 -> 7906250, mallocs 33130 -> 22617, opens 519 -> 478, read_bytes.ipc 11658 -> 7351, read_bytes.jobs 24070 -> 21759, read_bytes.outside 23926 -> 24063, read_bytes.state 322 -> 174, read_bytes.store 30594 -> 39583, read_calls.ipc 541 -> 268, read_calls.jobs 63 -> 53, read_calls.outside 33 -> 35, read_calls.state 8 -> 4, read_calls.store 249 -> 219, spawn:bun 2 -> 0, spawn:cargo 2 -> 0, spawn:claude 3 -> 0, spawn:codex 9 -> 7, spawn:dash 0 -> 1, spawn:defaults 6 -> 0, spawn:git 62 -> 41, spawn:npm 2 -> 0, spawn:pnpm 4 -> 0, spawn:ps 3 -> 1, spawn:python3 6 -> 2, spawn:rustc 2 -> 0, spawn:sw_vers 2 -> 0, spawn:uv 2 -> 0, spawn:xcrun 2 -> 0, spawn:yarn 2 -> 0, spawns 121 -> 64, syncs.jobs 76 -> 65, syncs.outside 45 -> 64, unlinks 56 -> 50, write_bytes.ipc 4180 -> 3619, write_bytes.jobs 14268 -> 14247, write_bytes.outside 504 -> 661, write_bytes.store 98343 -> 115527, write_calls.ipc 80 -> 67, write_calls.outside 11 -> 12, write_calls.store 61 -> 65
- cold start: cli_cpu_us 20926 -> 20606, cli_footprint_kib 3073 -> 2929, cli_maxrss_kib 5120 -> 4800, frees 2311 -> 2503, malloc_bytes 1050514 -> 1106906, mallocs 3258 -> 3451
- restart: barrierfsync 0 -> 3, cli_cpu_us 30171 -> 20934, cli_footprint_kib 3137 -> 3009, cli_maxrss_kib 5264 -> 4896, cli_processes 4 -> 3, frees 5823 -> 5691, fullfsync 44 -> 6, malloc_bytes 2271020 -> 2053212, mallocs 8892 -> 8694, opens 261 -> 102, read_bytes.jobs 8002 -> 4130, read_bytes.store 729276 -> 733396, read_calls.jobs 128 -> 64, read_calls.store 179 -> 180, renames 20 -> 4, syncs.jobs 38 -> 3, write_bytes.ipc 1673 -> 1508, write_bytes.jobs 2681 -> 487, write_calls.ipc 25 -> 23, write_calls.jobs 19 -> 3

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13760 | 15664 | 12168 | 14144 | 0.884 | 0.882 to 0.887 |
| b | 130 | 15280 | 15920 | 13888 | 14320 | 0.902 | 0.900 to 0.904 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5153 | 6209 | 4993 | 6065 | 0.964 | 0.958 to 0.965 |
| b | 130 | 5825 | 6465 | 6033 | 6465 | 1.013 | 1.008 to 1.027 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10080 | 10432 | 8624 | 8816 | 0.856 | 0.850 to 0.858 |
| daemon b | 15408 | 16528 | 13616 | 14432 | 0.881 | 0.878 to 0.884 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3200 | 3617 | 2912 | 3121 | 0.900 | 0.888 to 0.920 |
| daemon b | 5824 | 6944 | 5632 | 6448 | 0.960 | 0.953 to 0.974 |
