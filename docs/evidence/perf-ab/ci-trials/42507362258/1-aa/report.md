### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 270.6 | 240.8 | 0.904 | 0.868 to 1.076 | not shown |
| acp launch | 15 | 29.5 | 31.3 | 1.028 | 0.972 to 1.104 | not shown, median above 1 |
| acp approve | 15 | 6.6 | 6.7 | 0.984 | 0.967 to 0.990 | faster |
| claude new | 15 | 265.8 | 222.4 | 0.915 | 0.644 to 1.095 | not shown |
| claude launch | 15 | 188.0 | 213.0 | 1.095 | 0.943 to 1.331 | not shown, median above 1 |
| claude approve | 15 | 8.1 | 8.8 | 0.994 | 0.939 to 1.235 | not shown |
| codex new | 15 | 237.6 | 272.7 | 1.110 | 0.926 to 1.396 | not shown, median above 1 |
| codex launch | 15 | 220.3 | 224.6 | 0.969 | 0.673 to 1.089 | not shown |
| codex approve | 15 | 8.3 | 8.2 | 1.008 | 0.946 to 1.049 | not shown, median above 1 |
| ledger append | 15 | 7.3 | 7.3 | 0.987 | 0.964 to 1.032 | not shown |
| cli version | 15 | 3.8 | 3.5 | 0.968 | 0.948 to 1.021 | not shown |
| cli ls | 15 | 4.6 | 4.4 | 1.033 | 0.993 to 1.185 | not shown, median above 1 |
| acp suspend | 15 | 530.9 | 510.1 | 1.028 | 0.963 to 1.104 | not shown, median above 1 |
| acp move | 15 | 384.1 | 344.8 | 1.030 | 0.902 to 1.088 | not shown, median above 1 |
| acp resume | 15 | 916.0 | 959.5 | 1.045 | 0.901 to 1.130 | not shown, median above 1 |
| acp pull | 15 | 1772.1 | 1795.5 | 1.052 | 0.999 to 1.108 | not shown, median above 1 |
| claude suspend | 15 | 628.8 | 670.2 | 0.999 | 0.914 to 1.148 | not shown |
| claude move | 15 | 326.4 | 303.2 | 0.930 | 0.879 to 1.112 | not shown |
| claude resume | 15 | 1374.1 | 1496.1 | 1.074 | 0.981 to 1.176 | not shown, median above 1 |
| claude pull | 15 | 2402.4 | 2496.3 | 0.993 | 0.966 to 1.031 | not shown |
| codex suspend | 15 | 698.8 | 734.4 | 0.971 | 0.916 to 1.092 | not shown |
| codex move | 15 | 337.5 | 336.6 | 1.014 | 0.991 to 1.124 | not shown, median above 1 |
| codex resume | 15 | 1258.8 | 1346.1 | 0.997 | 0.859 to 1.120 | not shown |
| codex pull | 15 | 2427.1 | 2416.9 | 1.005 | 0.949 to 1.057 | not shown, median above 1 |
| cold start | 15 | 90.4 | 75.6 | 1.004 | 0.709 to 1.109 | not shown, median above 1 |
| restart | 15 | 149.8 | 147.9 | 1.061 | 0.714 to 1.255 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 7040 -> 7056 | 3969 -> 4001 | 11.6 -> 10.6 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1185 -> 1187 | 3656 -> 3658 | 6992 -> 6976 | 3377 -> 3345 | 11.7 -> 10.5 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5200 | 3057 -> 3057 | 7.8 -> 8.6 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5360 -> 5328 | 3137 -> 3121 | 8.0 -> 7.9 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1102 -> 1101 | 3055 -> 3049 | 7040 -> 6976 | 3409 -> 3329 | 11.2 -> 10.8 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5232 -> 5200 | 3105 -> 3057 | 8.1 -> 9.1 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5328 -> 5248 | 3137 -> 3057 | 9.6 -> 8.9 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3365 -> 3365 | 7120 -> 7088 | 3377 -> 3361 | 10.7 -> 10.3 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5200 | 3057 -> 3057 | 9.2 -> 8.2 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5200 -> 5216 | 3057 -> 3089 | 8.5 -> 9.3 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 148 | 189 -> 193 | 3552 -> 3552 | 2625 -> 2609 | 7.4 -> 9.5 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5232 -> 5232 | 3073 -> 3089 | 8.4 -> 9.0 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48208 -> 48169 | 14479 -> 14481 | 5600 -> 5568 | 3281 -> 3233 | 11.4 -> 8.7 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5212 -> 5223 | 7936 -> 7935 | 5760 -> 5760 | 3313 -> 3297 | 8.7 -> 8.6 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19823 -> 19794 | 11219 -> 11223 | 7104 -> 7088 | 3425 -> 3409 | 12.8 -> 9.9 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37097 -> 37036 | 30786 -> 30373 | 7184 -> 7184 | 3473 -> 3489 | 11.4 -> 11.4 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13828 -> 13840 | 20474 -> 20425 | 5600 -> 5552 | 3297 -> 3233 | 9.2 -> 8.8 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5274 -> 5282 | 7876 -> 7864 | 5776 -> 5760 | 3313 -> 3297 | 9.8 -> 8.8 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27837 -> 27885 | 11643 -> 11660 | 7408 -> 7392 | 3489 -> 3473 | 15.7 -> 14.6 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48498 -> 48553 | 38501 -> 38508 | 7456 -> 7472 | 3489 -> 3505 | 13.6 -> 14.1 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15198 -> 15215 | 15928 -> 15802 | 5584 -> 5488 | 3249 -> 3185 | 8.7 -> 8.6 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5320 -> 5326 | 7867 -> 7868 | 5776 -> 5792 | 3297 -> 3345 | 9.5 -> 7.9 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32921 -> 32932 | 11748 -> 11762 | 7376 -> 7392 | 3457 -> 3489 | 12.4 -> 11.9 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54950 -> 54962 | 33151 -> 33126 | 7440 -> 7440 | 3505 -> 3505 | 13.6 -> 13.2 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3259 -> 3258 | 5216 -> 5248 | 3073 -> 3089 | 28.7 -> 33.3 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 776107 -> 784347 | 10272 -> 9354 | 5248 -> 5248 | 3105 -> 3105 | 45.6 -> 42.0 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 11595 -> 10636, cli_footprint_kib 3969 -> 4001, cli_maxrss_kib 7040 -> 7056, malloc_bytes 13401480 -> 13401211
- acp launch: cli_cpu_us 11683 -> 10469, cli_footprint_kib 3377 -> 3345, cli_maxrss_kib 6992 -> 6976, frees 2930 -> 2931, malloc_bytes 635260 -> 635663, mallocs 3656 -> 3658, read_bytes.jobs 1011 -> 1013, write_bytes.jobs 1033 -> 1035
- acp approve: cli_cpu_us 7831 -> 8573, cli_maxrss_kib 5184 -> 5200, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 8001 -> 7859, cli_footprint_kib 3137 -> 3121, cli_maxrss_kib 5360 -> 5328, malloc_bytes 308809 -> 308812
- claude launch: cli_cpu_us 11198 -> 10840, cli_footprint_kib 3409 -> 3329, cli_maxrss_kib 7040 -> 6976, frees 2700 -> 2692, malloc_bytes 521569 -> 521338, mallocs 3055 -> 3049, read_bytes.jobs 928 -> 927, write_bytes.jobs 931 -> 930
- claude approve: cli_cpu_us 8052 -> 9095, cli_footprint_kib 3105 -> 3057, cli_maxrss_kib 5232 -> 5200, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 9635 -> 8883, cli_footprint_kib 3137 -> 3057, cli_maxrss_kib 5328 -> 5248, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 10664 -> 10279, cli_footprint_kib 3377 -> 3361, cli_maxrss_kib 7120 -> 7088, malloc_bytes 612957 -> 612967
- codex approve: cli_cpu_us 9247 -> 8241, cli_maxrss_kib 5184 -> 5200, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 8524 -> 9326, cli_footprint_kib 3057 -> 3089, cli_maxrss_kib 5200 -> 5216, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 7388 -> 9475, cli_footprint_kib 2625 -> 2609, frees 176 -> 180, malloc_bytes 87612 -> 88275, mallocs 189 -> 193, opens 0 -> 2, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli ls: cli_cpu_us 8449 -> 9039, cli_footprint_kib 3073 -> 3089, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 11414 -> 8730, cli_footprint_kib 3281 -> 3233, cli_maxrss_kib 5600 -> 5568, frees 9870 -> 9872, malloc_bytes 3464246 -> 3477249, mallocs 14479 -> 14481, read_bytes.store 45819 -> 45780, read_calls.ipc 183 -> 184, write_bytes.jobs 3583 -> 3587, write_bytes.store 112178 -> 112164
- acp move: cli_cpu_us 8665 -> 8575, cli_footprint_kib 3313 -> 3297, frees 6504 -> 6498, malloc_bytes 1975425 -> 1975680, mallocs 7936 -> 7935, read_bytes.jobs 4099 -> 4107, read_bytes.store 1113 -> 1116
- acp resume: cli_cpu_us 12848 -> 9859, cli_footprint_kib 3425 -> 3409, cli_maxrss_kib 7104 -> 7088, frees 9213 -> 9217, malloc_bytes 4659339 -> 4659912, mallocs 11219 -> 11223, read_bytes.jobs 3562 -> 3557, read_bytes.store 16139 -> 16115, read_calls.ipc 190 -> 197, write_bytes.jobs 3556 -> 3547, write_bytes.store 6153 -> 6149
- acp pull: cli_cpu_us 11433 -> 11363, cli_footprint_kib 3473 -> 3489, frees 23612 -> 23588, malloc_bytes 9199500 -> 9094900, mallocs 30786 -> 30373, read_bytes.jobs 8543 -> 8542, read_bytes.state 174 -> 322, read_bytes.store 28380 -> 28172, read_calls.ipc 522 -> 527, read_calls.state 4 -> 8, write_bytes.jobs 8400 -> 8395, write_bytes.store 69859 -> 86296, write_calls.store 45 -> 53
- claude suspend: cli_cpu_us 9232 -> 8837, cli_footprint_kib 3297 -> 3233, cli_maxrss_kib 5600 -> 5552, frees 14808 -> 14803, malloc_bytes 3699886 -> 3694469, mallocs 20474 -> 20425, read_bytes.store 9507 -> 9519, read_calls.ipc 184 -> 185, write_bytes.jobs 5312 -> 5325, write_bytes.store 79460 -> 87716, write_calls.store 45 -> 49
- claude move: cli_cpu_us 9783 -> 8809, cli_footprint_kib 3313 -> 3297, cli_maxrss_kib 5776 -> 5760, frees 6451 -> 6439, malloc_bytes 1968199 -> 1966761, mallocs 7876 -> 7864, read_bytes.jobs 4070 -> 4074, read_bytes.store 1204 -> 1208
- claude resume: cli_cpu_us 15656 -> 14569, cli_footprint_kib 3489 -> 3473, cli_maxrss_kib 7408 -> 7392, frees 9505 -> 9516, malloc_bytes 4792741 -> 4794000, mallocs 11643 -> 11660, read_bytes.jobs 10812 -> 10822, read_bytes.store 16903 -> 16941, read_calls.ipc 205 -> 198, write_bytes.jobs 5258 -> 5266, write_bytes.store 6150 -> 6145
- claude pull: cli_cpu_us 13565 -> 14076, cli_footprint_kib 3489 -> 3505, cli_maxrss_kib 7456 -> 7472, frees 29529 -> 29544, malloc_bytes 9681451 -> 9681126, mallocs 38501 -> 38508, read_bytes.ipc 11495 -> 11496, read_bytes.jobs 18524 -> 18517, read_bytes.store 29652 -> 29714, read_calls.ipc 537 -> 531, write_bytes.jobs 12020 -> 12009, write_bytes.store 94724 -> 102962, write_calls.store 57 -> 61
- codex suspend: cli_cpu_us 8721 -> 8616, cli_footprint_kib 3249 -> 3185, cli_maxrss_kib 5584 -> 5488, frees 11335 -> 11308, malloc_bytes 3694290 -> 3735854, mallocs 15928 -> 15802, read_bytes.store 9826 -> 9843, write_bytes.jobs 6431 -> 6433, write_bytes.store 112788 -> 96312, write_calls.store 61 -> 53
- codex move: cli_cpu_us 9528 -> 7949, cli_footprint_kib 3297 -> 3345, cli_maxrss_kib 5776 -> 5792, frees 6436 -> 6435, malloc_bytes 1967741 -> 1968386, mallocs 7867 -> 7868, read_bytes.jobs 4090 -> 4094, read_bytes.store 1230 -> 1232
- codex resume: cli_cpu_us 12378 -> 11853, cli_footprint_kib 3457 -> 3489, cli_maxrss_kib 7376 -> 7392, frees 9645 -> 9659, malloc_bytes 4895959 -> 4897070, mallocs 11748 -> 11762, opens 302 -> 304, read_bytes.jobs 15360 -> 15362, read_bytes.store 17439 -> 17448, read_calls.ipc 214 -> 212, write_bytes.jobs 6413 -> 6421, write_bytes.store 6154 -> 6153
- codex pull: cli_cpu_us 13589 -> 13185, frees 25553 -> 25528, malloc_bytes 9730322 -> 9729679, mallocs 33151 -> 33126, read_bytes.jobs 24069 -> 24066, read_bytes.store 30559 -> 30574, read_calls.ipc 538 -> 544, write_bytes.jobs 14262 -> 14267, write_bytes.store 113183 -> 104961, write_calls.store 67 -> 63
- cold start: cli_cpu_us 28662 -> 33336, cli_footprint_kib 3073 -> 3089, cli_maxrss_kib 5216 -> 5248, frees 2312 -> 2311, malloc_bytes 1050578 -> 1050526, mallocs 3259 -> 3258
- restart: cli_cpu_us 45640 -> 42048, frees 6451 -> 5978, malloc_bytes 2286732 -> 2274863, mallocs 10272 -> 9354, opens 277 -> 261, read_bytes.store 745756 -> 753996, read_calls.store 183 -> 185

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13992 | 15920 | 13960 | 15664 | 1.000 | 0.998 to 1.000 |
| b | 130 | 15568 | 15968 | 15328 | 16048 | 0.987 | 0.986 to 0.990 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5425 | 6433 | 5361 | 6177 | 1.003 | 0.995 to 1.007 |
| b | 130 | 6081 | 6481 | 5825 | 6529 | 0.962 | 0.962 to 0.969 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10112 | 10448 | 10128 | 10544 | 1.002 | 0.998 to 1.008 |
| daemon b | 15376 | 16336 | 15488 | 16096 | 1.000 | 0.983 to 1.006 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3232 | 3569 | 3249 | 3681 | 1.005 | 1.005 to 1.027 |
| daemon b | 5809 | 6753 | 5921 | 6513 | 1.003 | 0.960 to 1.016 |
