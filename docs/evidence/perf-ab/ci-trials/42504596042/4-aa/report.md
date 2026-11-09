### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 219.7 | 208.3 | 0.949 | 0.729 to 1.306 | not shown |
| acp launch | 15 | 34.3 | 29.8 | 1.030 | 0.913 to 1.102 | not shown, median above 1 |
| acp approve | 15 | 6.8 | 7.2 | 1.016 | 0.961 to 1.053 | not shown, median above 1 |
| claude new | 15 | 206.0 | 237.1 | 1.146 | 0.975 to 1.305 | not shown, median above 1 |
| claude launch | 15 | 160.2 | 182.1 | 1.217 | 0.933 to 1.470 | not shown, median above 1 |
| claude approve | 15 | 8.0 | 8.6 | 1.022 | 0.857 to 1.261 | not shown, median above 1 |
| codex new | 15 | 218.7 | 259.9 | 1.122 | 1.033 to 1.420 | slower |
| codex launch | 15 | 214.4 | 232.4 | 1.077 | 0.801 to 1.392 | not shown, median above 1 |
| codex approve | 15 | 9.5 | 8.6 | 1.041 | 0.815 to 1.182 | not shown, median above 1 |
| ledger append | 15 | 7.6 | 8.0 | 1.035 | 0.993 to 1.141 | not shown, median above 1 |
| cli version | 15 | 3.4 | 3.6 | 1.031 | 0.984 to 1.082 | not shown, median above 1 |
| cli ls | 15 | 4.1 | 4.4 | 1.003 | 0.935 to 1.087 | not shown, median above 1 |
| acp suspend | 15 | 521.1 | 509.3 | 1.059 | 0.924 to 1.089 | not shown, median above 1 |
| acp move | 15 | 305.4 | 330.4 | 1.049 | 0.981 to 1.079 | not shown, median above 1 |
| acp resume | 15 | 955.5 | 961.8 | 1.038 | 0.947 to 1.148 | not shown, median above 1 |
| acp pull | 15 | 1813.9 | 1632.7 | 0.937 | 0.825 to 1.006 | not shown |
| claude suspend | 15 | 665.3 | 685.2 | 1.046 | 0.949 to 1.089 | not shown, median above 1 |
| claude move | 15 | 313.3 | 314.6 | 0.985 | 0.894 to 1.111 | not shown |
| claude resume | 15 | 1353.6 | 1399.0 | 1.010 | 0.987 to 1.087 | not shown, median above 1 |
| claude pull | 15 | 2252.9 | 2201.7 | 0.998 | 0.895 to 1.044 | not shown |
| codex suspend | 15 | 690.2 | 715.8 | 0.958 | 0.921 to 1.141 | not shown |
| codex move | 15 | 295.3 | 314.9 | 1.011 | 0.987 to 1.197 | not shown, median above 1 |
| codex resume | 15 | 1384.3 | 1363.7 | 0.953 | 0.880 to 1.006 | not shown |
| codex pull | 15 | 2342.5 | 2493.1 | 1.009 | 0.949 to 1.069 | not shown, median above 1 |
| cold start | 15 | 83.7 | 78.9 | 1.309 | 0.803 to 1.427 | not shown, median above 1 |
| restart | 15 | 156.2 | 163.9 | 0.946 | 0.870 to 1.050 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38480 -> 38478 | 7056 -> 7056 | 4017 -> 4001 | 13.6 -> 11.2 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1186 -> 1187 | 3649 -> 3657 | 6992 -> 7024 | 3393 -> 3425 | 11.9 -> 13.2 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 733 | 5184 -> 5184 | 3089 -> 3057 | 15.8 -> 13.3 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1205 | 5280 -> 5296 | 3105 -> 3105 | 9.3 -> 9.8 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1104 -> 1101 | 3059 -> 3060 | 7024 -> 7040 | 3425 -> 3441 | 14.6 -> 11.9 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5168 -> 5168 | 3041 -> 3041 | 7.7 -> 9.0 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5392 -> 5328 | 3217 -> 3169 | 10.8 -> 9.1 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3371 -> 3372 | 7120 -> 7088 | 3409 -> 3393 | 13.4 -> 15.1 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5184 | 3057 -> 3057 | 10.4 -> 11.5 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5216 -> 5184 | 3121 -> 3073 | 9.6 -> 8.8 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2657 -> 2657 | 8.0 -> 7.1 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5248 -> 5248 | 3137 -> 3121 | 10.4 -> 10.5 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48168 -> 48197 | 14490 -> 14492 | 5616 -> 5568 | 3313 -> 3281 | 12.3 -> 10.6 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5208 -> 5213 | 7931 -> 7930 | 5776 -> 5824 | 3345 -> 3393 | 9.3 -> 12.5 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19794 -> 19807 | 11218 -> 11225 | 7104 -> 7104 | 3457 -> 3457 | 12.9 -> 15.2 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 36901 -> 37116 | 30752 -> 30775 | 7168 -> 7168 | 3489 -> 3505 | 16.8 -> 16.1 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13817 -> 13865 | 20431 -> 20475 | 5568 -> 5568 | 3281 -> 3281 | 8.4 -> 11.1 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5267 -> 5279 | 7854 -> 7849 | 5776 -> 5760 | 3345 -> 3345 | 11.7 -> 12.7 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27838 -> 28032 | 11652 -> 11660 | 7376 -> 7408 | 3473 -> 3521 | 11.8 -> 14.8 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48470 -> 48593 | 38510 -> 38516 | 7456 -> 7456 | 3537 -> 3521 | 14.4 -> 16.0 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15194 -> 15182 | 16015 -> 15796 | 5584 -> 5536 | 3281 -> 3249 | 8.6 -> 12.5 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5323 -> 5320 | 7866 -> 7870 | 5760 -> 5760 | 3329 -> 3329 | 9.7 -> 10.4 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32921 -> 32903 | 11755 -> 11756 | 7376 -> 7392 | 3505 -> 3505 | 15.1 -> 11.8 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54956 -> 54909 | 33145 -> 33146 | 7456 -> 7440 | 3553 -> 3537 | 12.6 -> 11.8 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3695 -> 3692 | 5200 -> 5168 | 3073 -> 3057 | 36.7 -> 29.2 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 767867 -> 755507 | 9748 -> 12160 | 5232 -> 5360 | 3105 -> 3201 | 26.6 -> 57.8 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 13554 -> 11217, cli_footprint_kib 4017 -> 4001, malloc_bytes 13401488 -> 13401147, mallocs 38480 -> 38478
- acp launch: cli_cpu_us 11949 -> 13197, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 6992 -> 7024, frees 2923 -> 2930, malloc_bytes 635024 -> 635325, mallocs 3649 -> 3657, read_bytes.jobs 1012 -> 1013, write_bytes.jobs 1034 -> 1035
- acp approve: cli_cpu_us 15785 -> 13283, cli_footprint_kib 3089 -> 3057, malloc_bytes 242240 -> 242307, mallocs 732 -> 733
- claude new: cli_cpu_us 9291 -> 9777, cli_maxrss_kib 5280 -> 5296, malloc_bytes 308809 -> 309148, mallocs 1204 -> 1205
- claude launch: cli_cpu_us 14642 -> 11852, cli_footprint_kib 3425 -> 3441, cli_maxrss_kib 7024 -> 7040, frees 2701 -> 2698, malloc_bytes 522067 -> 521946, mallocs 3059 -> 3060, read_bytes.jobs 930 -> 927, write_bytes.jobs 933 -> 930
- claude approve: cli_cpu_us 7746 -> 9046, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 10848 -> 9054, cli_footprint_kib 3217 -> 3169, cli_maxrss_kib 5392 -> 5328, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 13364 -> 15141, cli_footprint_kib 3409 -> 3393, cli_maxrss_kib 7120 -> 7088, frees 2992 -> 2993, malloc_bytes 613340 -> 613253, mallocs 3371 -> 3372
- codex approve: cli_cpu_us 10365 -> 11549, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 9622 -> 8789, cli_footprint_kib 3121 -> 3073, cli_maxrss_kib 5216 -> 5184, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 7991 -> 7085, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 10374 -> 10454, cli_footprint_kib 3137 -> 3121, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 12316 -> 10640, cli_footprint_kib 3313 -> 3281, cli_maxrss_kib 5616 -> 5568, malloc_bytes 3483000 -> 3464665, mallocs 14490 -> 14492, read_bytes.store 45779 -> 45808, write_bytes.jobs 3580 -> 3584, write_bytes.store 112180 -> 112208
- acp move: cli_cpu_us 9292 -> 12454, cli_footprint_kib 3345 -> 3393, cli_maxrss_kib 5776 -> 5824, frees 6495 -> 6491, malloc_bytes 1975500 -> 1975982, mallocs 7931 -> 7930, read_bytes.jobs 4095 -> 4103, read_bytes.store 1113 -> 1110
- acp resume: cli_cpu_us 12864 -> 15240, frees 9212 -> 9219, malloc_bytes 4659368 -> 4659986, mallocs 11218 -> 11225, read_bytes.jobs 3559 -> 3561, read_bytes.store 16113 -> 16124, read_calls.ipc 193 -> 197, write_bytes.jobs 3555 -> 3547, write_bytes.store 6151 -> 6149
- acp pull: cli_cpu_us 16810 -> 16070, cli_footprint_kib 3489 -> 3505, frees 23596 -> 23595, malloc_bytes 9194437 -> 9196067, mallocs 30752 -> 30775, opens 425 -> 427, read_bytes.jobs 8542 -> 8538, read_bytes.state 174 -> 322, read_bytes.store 28185 -> 28256, read_calls.ipc 520 -> 515, read_calls.state 4 -> 8, write_bytes.jobs 8399 -> 8393, write_bytes.store 86308 -> 94553, write_calls.store 53 -> 57
- claude suspend: cli_cpu_us 8419 -> 11147, frees 14804 -> 14807, malloc_bytes 3694703 -> 3700091, mallocs 20431 -> 20475, read_bytes.store 9496 -> 9544, read_calls.ipc 185 -> 184, write_bytes.jobs 5314 -> 5320, write_bytes.store 87708 -> 62998, write_calls.store 49 -> 37
- claude move: cli_cpu_us 11731 -> 12719, cli_maxrss_kib 5776 -> 5760, frees 6424 -> 6414, malloc_bytes 1966761 -> 1966829, mallocs 7854 -> 7849, read_bytes.jobs 4062 -> 4066, read_bytes.store 1205 -> 1213
- claude resume: cli_cpu_us 11762 -> 14766, cli_footprint_kib 3473 -> 3521, cli_maxrss_kib 7376 -> 7408, frees 9514 -> 9516, malloc_bytes 4793600 -> 4793993, mallocs 11652 -> 11660, opens 260 -> 258, read_bytes.jobs 10820 -> 10810, read_bytes.state 122 -> 270, read_bytes.store 16896 -> 16952, read_calls.ipc 203 -> 204, read_calls.state 4 -> 8, write_bytes.jobs 5257 -> 5244
- claude pull: cli_cpu_us 14385 -> 16028, cli_footprint_kib 3537 -> 3521, frees 29536 -> 29553, malloc_bytes 9681069 -> 9681036, mallocs 38510 -> 38516, opens 468 -> 470, read_bytes.ipc 11494 -> 11496, read_bytes.jobs 18513 -> 18522, read_bytes.store 29635 -> 29749, read_calls.ipc 533 -> 538, write_bytes.jobs 12000 -> 12022, write_bytes.store 106194 -> 102974, write_calls.store 65 -> 61
- codex suspend: cli_cpu_us 8622 -> 12530, cli_footprint_kib 3281 -> 3249, cli_maxrss_kib 5584 -> 5536, frees 11311 -> 11308, malloc_bytes 3719050 -> 3693631, mallocs 16015 -> 15796, read_bytes.store 9822 -> 9810, read_calls.ipc 186 -> 188, write_bytes.jobs 6432 -> 6424, write_bytes.store 71588 -> 112778, write_calls.store 41 -> 61
- codex move: cli_cpu_us 9659 -> 10367, frees 6438 -> 6441, malloc_bytes 1967015 -> 1967174, mallocs 7866 -> 7870, read_bytes.store 1233 -> 1230
- codex resume: cli_cpu_us 15131 -> 11772, cli_maxrss_kib 7376 -> 7392, frees 9646 -> 9653, malloc_bytes 4896401 -> 4896875, mallocs 11755 -> 11756, opens 302 -> 304, read_bytes.jobs 15362 -> 15361, read_bytes.store 17437 -> 17420, read_calls.ipc 209 -> 212, write_bytes.jobs 6426 -> 6418, write_bytes.store 6146 -> 6150
- codex pull: cli_cpu_us 12561 -> 11820, cli_footprint_kib 3553 -> 3537, cli_maxrss_kib 7456 -> 7440, frees 25547 -> 25548, malloc_bytes 9730144 -> 9731562, mallocs 33145 -> 33146, opens 519 -> 521, read_bytes.ipc 11658 -> 11660, read_bytes.jobs 24068 -> 24063, read_bytes.store 30566 -> 30524, read_calls.ipc 534 -> 540, write_bytes.jobs 14260 -> 14262, write_bytes.store 113177 -> 123039, write_calls.store 67 -> 73
- cold start: cli_cpu_us 36680 -> 29151, cli_footprint_kib 3073 -> 3057, cli_maxrss_kib 5200 -> 5168, frees 2648 -> 2643, malloc_bytes 1260809 -> 1260616, mallocs 3695 -> 3692
- restart: cli_cpu_us 26623 -> 57758, cli_footprint_kib 3105 -> 3201, cli_maxrss_kib 5232 -> 5360, cli_processes 3 -> 6, frees 6107 -> 7916, malloc_bytes 2173661 -> 3011864, mallocs 9748 -> 12160, opens 261 -> 293, read_bytes.store 737516 -> 725156, read_calls.store 181 -> 178, write_bytes.ipc 1508 -> 2402, write_calls.ipc 23 -> 35

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13960 | 15840 | 13808 | 15776 | 0.990 | 0.989 to 0.993 |
| b | 130 | 15568 | 16048 | 15520 | 16064 | 0.998 | 0.997 to 0.998 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5393 | 6385 | 5201 | 6321 | 0.978 | 0.975 to 0.984 |
| b | 130 | 6065 | 6545 | 6033 | 6577 | 0.995 | 0.995 to 0.995 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10480 | 10112 | 10448 | 1.000 | 0.997 to 1.005 |
| daemon b | 15472 | 16304 | 15456 | 16384 | 0.999 | 0.995 to 1.005 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3248 | 3617 | 3232 | 3601 | 1.005 | 0.989 to 1.027 |
| daemon b | 5905 | 6705 | 5889 | 6801 | 0.997 | 0.986 to 1.014 |
