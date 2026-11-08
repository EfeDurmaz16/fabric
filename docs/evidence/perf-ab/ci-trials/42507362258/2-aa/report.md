### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 257.1 | 237.2 | 0.957 | 0.921 to 1.025 | not shown |
| acp launch | 15 | 25.3 | 25.7 | 0.989 | 0.961 to 1.021 | not shown |
| acp approve | 15 | 6.0 | 6.2 | 1.000 | 0.970 to 1.030 | not shown |
| claude new | 15 | 223.0 | 246.3 | 1.111 | 1.034 to 1.447 | slower |
| claude launch | 15 | 212.9 | 230.9 | 1.097 | 0.935 to 1.171 | not shown, median above 1 |
| claude approve | 15 | 7.6 | 8.0 | 0.972 | 0.802 to 1.211 | not shown |
| codex new | 15 | 232.9 | 249.8 | 0.968 | 0.803 to 1.021 | not shown |
| codex launch | 15 | 226.1 | 242.5 | 1.202 | 0.940 to 1.425 | not shown, median above 1 |
| codex approve | 15 | 6.8 | 7.3 | 0.984 | 0.939 to 1.432 | not shown |
| ledger append | 15 | 6.4 | 6.3 | 0.961 | 0.930 to 1.031 | not shown |
| cli version | 15 | 2.9 | 3.0 | 1.030 | 0.967 to 1.087 | not shown, median above 1 |
| cli ls | 15 | 3.9 | 3.7 | 0.945 | 0.916 to 1.000 | faster |
| acp suspend | 15 | 463.9 | 461.8 | 1.025 | 0.962 to 1.045 | not shown, median above 1 |
| acp move | 15 | 305.6 | 286.5 | 0.953 | 0.929 to 0.993 | faster |
| acp resume | 15 | 860.7 | 921.7 | 0.984 | 0.961 to 1.087 | not shown |
| acp pull | 15 | 1622.1 | 1601.2 | 1.037 | 1.031 to 1.089 | slower |
| claude suspend | 15 | 628.4 | 652.2 | 1.010 | 0.896 to 1.119 | not shown, median above 1 |
| claude move | 15 | 312.3 | 295.8 | 1.031 | 0.859 to 1.322 | not shown, median above 1 |
| claude resume | 15 | 1354.2 | 1242.6 | 0.990 | 0.903 to 1.064 | not shown |
| claude pull | 15 | 2235.3 | 2223.9 | 0.987 | 0.948 to 1.080 | not shown |
| codex suspend | 15 | 684.9 | 693.7 | 1.015 | 0.951 to 1.069 | not shown, median above 1 |
| codex move | 15 | 279.4 | 283.1 | 1.035 | 0.891 to 1.081 | not shown, median above 1 |
| codex resume | 15 | 1319.5 | 1251.8 | 0.969 | 0.868 to 1.115 | not shown |
| codex pull | 15 | 2358.5 | 2319.4 | 1.003 | 0.956 to 1.028 | not shown, median above 1 |
| cold start | 15 | 59.1 | 75.5 | 1.267 | 0.729 to 1.582 | not shown, median above 1 |
| restart | 15 | 140.1 | 157.4 | 1.095 | 0.980 to 1.153 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38480 -> 38479 | 7056 -> 6960 | 4049 -> 3953 | 9.0 -> 9.5 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1186 -> 1188 | 3656 -> 3266 | 6976 -> 6992 | 3409 -> 3409 | 8.7 -> 8.7 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5200 | 3089 -> 3105 | 6.7 -> 7.0 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1205 | 5280 -> 5344 | 3121 -> 3185 | 7.0 -> 6.8 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1102 -> 1100 | 3056 -> 3051 | 7024 -> 6976 | 3441 -> 3393 | 8.6 -> 8.8 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5248 -> 5200 | 3169 -> 3105 | 7.1 -> 6.8 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5344 -> 5344 | 3201 -> 3201 | 6.7 -> 7.0 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3365 -> 3365 | 7136 -> 7120 | 3441 -> 3425 | 8.8 -> 9.1 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 731 | 5184 -> 5200 | 3089 -> 3105 | 6.6 -> 6.9 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5232 -> 5168 | 3137 -> 3073 | 7.1 -> 6.7 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2673 -> 2673 | 6.3 -> 6.3 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5232 -> 5296 | 3137 -> 3185 | 7.0 -> 6.8 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48174 -> 48167 | 14482 -> 14489 | 5584 -> 5584 | 3297 -> 3313 | 7.2 -> 7.0 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5211 -> 5205 | 7929 -> 7920 | 5808 -> 5728 | 3393 -> 3313 | 7.1 -> 7.2 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19826 -> 19785 | 11223 -> 11243 | 7088 -> 7072 | 3473 -> 3441 | 8.6 -> 8.8 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37092 -> 36894 | 30358 -> 31673 | 7184 -> 7168 | 3521 -> 3505 | 10.2 -> 9.0 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13832 -> 13864 | 20429 -> 20428 | 5584 -> 5600 | 3297 -> 3345 | 8.3 -> 7.0 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5293 -> 5269 | 7855 -> 7868 | 5776 -> 5760 | 3361 -> 3345 | 7.6 -> 7.1 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27850 -> 28022 | 11654 -> 11657 | 7408 -> 7408 | 3537 -> 3537 | 9.6 -> 9.2 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48494 -> 48410 | 38507 -> 38514 | 7456 -> 7456 | 3537 -> 3537 | 10.0 -> 9.6 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15197 -> 15217 | 15811 -> 15915 | 5552 -> 5552 | 3297 -> 3281 | 7.1 -> 7.4 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5324 -> 5320 | 7861 -> 7870 | 5760 -> 5792 | 3361 -> 3377 | 7.8 -> 7.4 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32915 -> 33099 | 11749 -> 11753 | 7392 -> 7392 | 3537 -> 3537 | 9.6 -> 9.5 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54791 -> 55015 | 33141 -> 33142 | 7456 -> 7456 | 3569 -> 3553 | 9.9 -> 10.1 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3264 -> 3264 | 5216 -> 5200 | 3121 -> 3089 | 27.9 -> 18.6 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 804947 -> 784347 | 10093 -> 8889 | 5248 -> 5248 | 3137 -> 3137 | 26.3 -> 26.1 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 8988 -> 9464, cli_footprint_kib 4049 -> 3953, cli_maxrss_kib 7056 -> 6960, malloc_bytes 13401544 -> 13401275, mallocs 38480 -> 38479
- acp launch: cli_cpu_us 8690 -> 8701, cli_maxrss_kib 6976 -> 6992, frees 2930 -> 2905, malloc_bytes 635258 -> 532708, mallocs 3656 -> 3266, read_bytes.jobs 1012 -> 1014, write_bytes.jobs 1034 -> 1036
- acp approve: cli_cpu_us 6743 -> 6975, cli_footprint_kib 3089 -> 3105, cli_maxrss_kib 5184 -> 5200, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 6955 -> 6840, cli_footprint_kib 3121 -> 3185, cli_maxrss_kib 5280 -> 5344, malloc_bytes 308809 -> 309148, mallocs 1204 -> 1205
- claude launch: cli_cpu_us 8586 -> 8796, cli_footprint_kib 3441 -> 3393, cli_maxrss_kib 7024 -> 6976, frees 2694 -> 2696, malloc_bytes 521790 -> 521519, mallocs 3056 -> 3051, read_bytes.jobs 928 -> 926, write_bytes.jobs 931 -> 929
- claude approve: cli_cpu_us 7081 -> 6758, cli_footprint_kib 3169 -> 3105, cli_maxrss_kib 5248 -> 5200, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 6714 -> 6989, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 8788 -> 9120, cli_footprint_kib 3441 -> 3425, cli_maxrss_kib 7136 -> 7120, malloc_bytes 612964 -> 612967
- codex approve: cli_cpu_us 6572 -> 6933, cli_footprint_kib 3089 -> 3105, cli_maxrss_kib 5184 -> 5200, frees 607 -> 606, malloc_bytes 242047 -> 241987, mallocs 732 -> 731
- ledger append: cli_cpu_us 7080 -> 6704, cli_footprint_kib 3137 -> 3073, cli_maxrss_kib 5232 -> 5168, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 6267 -> 6290, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 6977 -> 6761, cli_footprint_kib 3137 -> 3185, cli_maxrss_kib 5232 -> 5296, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 7159 -> 7027, cli_footprint_kib 3297 -> 3313, frees 9871 -> 9869, malloc_bytes 3481620 -> 3477034, mallocs 14482 -> 14489, read_bytes.ipc 5214 -> 5213, read_bytes.store 45785 -> 45778, write_bytes.jobs 3583 -> 3584, write_bytes.store 95730 -> 95724
- acp move: cli_cpu_us 7106 -> 7250, cli_footprint_kib 3393 -> 3313, cli_maxrss_kib 5808 -> 5728, frees 6491 -> 6485, malloc_bytes 1975564 -> 1974918, mallocs 7929 -> 7920, read_bytes.jobs 4099 -> 4095, read_bytes.store 1112 -> 1110, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 8559 -> 8781, cli_footprint_kib 3473 -> 3441, cli_maxrss_kib 7088 -> 7072, frees 9217 -> 9222, malloc_bytes 4659882 -> 4661770, mallocs 11223 -> 11243, opens 222 -> 224, read_bytes.store 16144 -> 16103, read_calls.ipc 205 -> 200, write_bytes.jobs 3552 -> 3555, write_bytes.store 6145 -> 6141
- acp pull: cli_cpu_us 10197 -> 8976, cli_footprint_kib 3521 -> 3505, cli_maxrss_kib 7184 -> 7168, frees 23567 -> 23659, malloc_bytes 9092316 -> 9428710, mallocs 30358 -> 31673, opens 427 -> 425, read_bytes.ipc 11994 -> 11993, read_bytes.jobs 8549 -> 8544, read_bytes.state 322 -> 174, read_bytes.store 28221 -> 28176, read_calls.ipc 527 -> 529, read_calls.state 8 -> 4, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8409 -> 8400, write_bytes.store 94542 -> 94531
- claude suspend: cli_cpu_us 8282 -> 6986, cli_footprint_kib 3297 -> 3345, cli_maxrss_kib 5584 -> 5600, frees 14807 -> 14808, malloc_bytes 3694593 -> 3694427, mallocs 20429 -> 20428, read_bytes.store 9511 -> 9543, write_bytes.jobs 5319 -> 5320, write_bytes.store 95948 -> 79462, write_calls.store 53 -> 45
- claude move: cli_cpu_us 7602 -> 7070, cli_footprint_kib 3361 -> 3345, cli_maxrss_kib 5776 -> 5760, frees 6420 -> 6431, malloc_bytes 1967602 -> 1970577, mallocs 7855 -> 7868, read_bytes.jobs 4086 -> 4062, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 9550 -> 9209, frees 9516 -> 9523, malloc_bytes 4793569 -> 4794180, mallocs 11654 -> 11657, opens 258 -> 260, read_bytes.ipc 5336 -> 5338, read_bytes.jobs 10811 -> 10815, read_bytes.state 122 -> 270, read_bytes.store 16917 -> 16937, read_calls.state 4 -> 8, write_bytes.jobs 5245 -> 5256, write_bytes.store 6147 -> 6149
- claude pull: cli_cpu_us 9990 -> 9629, frees 29532 -> 29527, malloc_bytes 9680736 -> 9682075, mallocs 38507 -> 38514, opens 468 -> 470, read_bytes.ipc 11493 -> 11492, read_bytes.jobs 18520 -> 18529, read_bytes.state 322 -> 174, read_bytes.store 29652 -> 29707, read_calls.ipc 532 -> 530, read_calls.state 8 -> 4, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 12016 -> 12029, write_bytes.store 96344 -> 96335
- codex suspend: cli_cpu_us 7072 -> 7411, cli_footprint_kib 3297 -> 3281, frees 11309 -> 11343, malloc_bytes 3676061 -> 3676324, mallocs 15811 -> 15915, read_bytes.store 9825 -> 9845, read_calls.ipc 190 -> 189, write_bytes.jobs 6414 -> 6418, write_bytes.store 88076 -> 112802, write_calls.store 49 -> 61
- codex move: cli_cpu_us 7838 -> 7446, cli_footprint_kib 3361 -> 3377, cli_maxrss_kib 5760 -> 5792, frees 6430 -> 6438, malloc_bytes 1966933 -> 1967426, mallocs 7861 -> 7870, read_bytes.jobs 4090 -> 4086, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 9566 -> 9460, frees 9646 -> 9650, malloc_bytes 4895981 -> 4896651, mallocs 11749 -> 11753, opens 302 -> 304, read_bytes.ipc 5488 -> 5487, read_bytes.state 122 -> 270, read_bytes.store 17435 -> 17471, read_calls.ipc 213 -> 217, read_calls.state 4 -> 8, write_bytes.jobs 6425 -> 6419, write_bytes.store 6143 -> 6153
- codex pull: cli_cpu_us 9877 -> 10076, cli_footprint_kib 3569 -> 3553, malloc_bytes 9730007 -> 9730183, mallocs 33141 -> 33142, read_bytes.jobs 24066 -> 24061, read_bytes.state 174 -> 322, read_bytes.store 30551 -> 30632, read_calls.ipc 541 -> 540, read_calls.state 4 -> 8, write_bytes.ipc 4181 -> 4180, write_bytes.jobs 14259 -> 14253, write_bytes.store 98312 -> 98337
- cold start: cli_cpu_us 27893 -> 18563, cli_footprint_kib 3121 -> 3089, cli_maxrss_kib 5216 -> 5200, malloc_bytes 1051633 -> 1051645
- restart: cli_cpu_us 26326 -> 26068, frees 6644 -> 5621, malloc_bytes 2359555 -> 2060566, mallocs 10093 -> 8889, opens 277 -> 261, read_bytes.store 774596 -> 753996, read_calls.store 190 -> 185, write_bytes.ipc 2036 -> 1307, write_calls.ipc 30 -> 20

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13840 | 15536 | 13928 | 15616 | 1.000 | 0.997 to 1.002 |
| b | 130 | 15616 | 16192 | 15376 | 15904 | 0.989 | 0.988 to 0.990 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5233 | 6097 | 5409 | 6193 | 1.000 | 0.992 to 1.003 |
| b | 130 | 6161 | 6737 | 5905 | 6433 | 0.970 | 0.967 to 0.970 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10464 | 10112 | 10416 | 0.998 | 0.994 to 1.002 |
| daemon b | 15392 | 16416 | 15488 | 16384 | 1.006 | 0.998 to 1.011 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3265 | 3649 | 3232 | 3553 | 0.990 | 0.981 to 1.000 |
| daemon b | 5841 | 6849 | 5905 | 6801 | 1.011 | 0.993 to 1.029 |
