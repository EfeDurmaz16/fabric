### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 268.8 | 220.2 | 0.856 | 0.684 to 1.052 | not shown |
| acp launch | 15 | 29.5 | 28.9 | 1.026 | 0.953 to 1.049 | not shown, median above 1 |
| acp approve | 15 | 6.5 | 7.0 | 1.047 | 1.002 to 1.084 | slower |
| claude new | 15 | 215.1 | 248.4 | 1.064 | 0.919 to 1.290 | not shown, median above 1 |
| claude launch | 15 | 229.9 | 206.7 | 0.828 | 0.670 to 1.207 | not shown |
| claude approve | 15 | 8.2 | 7.5 | 0.963 | 0.796 to 1.101 | not shown |
| codex new | 15 | 248.8 | 229.6 | 0.958 | 0.843 to 1.088 | not shown |
| codex launch | 15 | 244.3 | 238.2 | 0.946 | 0.833 to 1.275 | not shown |
| codex approve | 15 | 7.4 | 8.3 | 1.028 | 0.915 to 1.172 | not shown, median above 1 |
| ledger append | 15 | 6.8 | 7.5 | 1.034 | 1.022 to 1.083 | slower |
| cli version | 15 | 3.1 | 3.3 | 1.019 | 0.970 to 1.126 | not shown, median above 1 |
| cli ls | 15 | 4.3 | 3.7 | 0.985 | 0.888 to 1.018 | not shown |
| acp suspend | 15 | 460.7 | 482.1 | 1.034 | 0.920 to 1.129 | not shown, median above 1 |
| acp move | 15 | 304.2 | 301.9 | 1.032 | 0.939 to 1.056 | not shown, median above 1 |
| acp resume | 15 | 854.8 | 834.1 | 0.961 | 0.858 to 1.184 | not shown |
| acp pull | 15 | 1569.4 | 1626.5 | 1.019 | 0.996 to 1.061 | not shown, median above 1 |
| claude suspend | 15 | 663.0 | 609.9 | 0.929 | 0.799 to 1.076 | not shown |
| claude move | 15 | 315.1 | 306.9 | 0.985 | 0.940 to 1.061 | not shown |
| claude resume | 15 | 1281.2 | 1291.8 | 1.008 | 0.876 to 1.068 | not shown, median above 1 |
| claude pull | 15 | 2332.5 | 2242.2 | 0.978 | 0.938 to 1.028 | not shown |
| codex suspend | 15 | 667.5 | 697.8 | 1.027 | 0.948 to 1.163 | not shown, median above 1 |
| codex move | 15 | 298.5 | 303.7 | 0.999 | 0.966 to 1.254 | not shown |
| codex resume | 15 | 1419.2 | 1299.4 | 0.970 | 0.906 to 1.110 | not shown |
| codex pull | 15 | 2270.3 | 2361.8 | 0.985 | 0.921 to 1.047 | not shown |
| cold start | 15 | 87.5 | 85.5 | 0.997 | 0.547 to 1.329 | not shown |
| restart | 15 | 176.8 | 165.0 | 0.881 | 0.820 to 1.174 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38478 -> 38479 | 7136 -> 7040 | 4129 -> 4033 | 9.9 -> 9.9 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1187 -> 1184 | 3657 -> 3271 | 7008 -> 6992 | 3441 -> 3425 | 11.1 -> 10.3 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5200 -> 5168 | 3105 -> 3089 | 8.2 -> 6.3 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1205 | 5248 -> 5360 | 3089 -> 3201 | 7.7 -> 7.5 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1099 -> 1102 | 3051 -> 3055 | 7008 -> 7072 | 3425 -> 3473 | 10.9 -> 9.1 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5168 | 3073 -> 3073 | 7.8 -> 7.8 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5296 -> 5296 | 3153 -> 3137 | 7.1 -> 7.6 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3364 -> 3371 | 7072 -> 7120 | 3393 -> 3425 | 10.7 -> 9.6 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5184 | 3105 -> 3073 | 8.3 -> 8.8 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 733 | 5184 -> 5184 | 3105 -> 3105 | 9.3 -> 9.6 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2673 -> 2673 | 6.5 -> 7.3 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1177 | 5216 -> 5200 | 3121 -> 3089 | 8.0 -> 7.2 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48186 -> 48256 | 14485 -> 14504 | 5584 -> 5552 | 3313 -> 3281 | 8.1 -> 8.0 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5216 -> 5225 | 7930 -> 7929 | 5776 -> 5744 | 3345 -> 3329 | 8.4 -> 7.8 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19946 -> 19827 | 11223 -> 11224 | 7104 -> 7072 | 3489 -> 3441 | 10.6 -> 10.0 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37031 -> 37061 | 30349 -> 30372 | 7168 -> 7184 | 3521 -> 3521 | 11.2 -> 10.6 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13845 -> 13825 | 20427 -> 20424 | 5552 -> 5520 | 3297 -> 3249 | 8.4 -> 8.1 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5273 -> 5262 | 7864 -> 7857 | 5776 -> 5760 | 3361 -> 3361 | 8.1 -> 8.2 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27869 -> 27815 | 11658 -> 11674 | 7408 -> 7408 | 3521 -> 3521 | 11.5 -> 11.7 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48543 -> 48305 | 38507 -> 38503 | 7472 -> 7472 | 3569 -> 3569 | 10.4 -> 11.2 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15220 -> 15202 | 16024 -> 15923 | 5536 -> 5584 | 3265 -> 3313 | 6.2 -> 7.2 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5325 -> 5324 | 7864 -> 7862 | 5712 -> 5808 | 3297 -> 3409 | 7.2 -> 7.7 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32959 -> 32921 | 11754 -> 11754 | 7408 -> 7392 | 3553 -> 3537 | 10.3 -> 11.8 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 55027 -> 54957 | 33136 -> 33144 | 7424 -> 7440 | 3521 -> 3537 | 11.8 -> 12.4 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4125 -> 3258 | 5184 -> 5200 | 3153 -> 3089 | 33.7 -> 23.6 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 751387 -> 751387 | 10525 -> 9810 | 5296 -> 5248 | 3185 -> 3137 | 27.6 -> 38.1 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 9950 -> 9890, cli_footprint_kib 4129 -> 4033, cli_maxrss_kib 7136 -> 7040, malloc_bytes 13401141 -> 13401211, mallocs 38478 -> 38479
- acp launch: cli_cpu_us 11127 -> 10312, cli_footprint_kib 3441 -> 3425, cli_maxrss_kib 7008 -> 6992, frees 2931 -> 2906, malloc_bytes 635308 -> 533380, mallocs 3657 -> 3271, read_bytes.jobs 1013 -> 1010, write_bytes.jobs 1035 -> 1032
- acp approve: cli_cpu_us 8201 -> 6323, cli_footprint_kib 3105 -> 3089, cli_maxrss_kib 5200 -> 5168, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 7675 -> 7515, cli_footprint_kib 3089 -> 3201, cli_maxrss_kib 5248 -> 5360, malloc_bytes 308809 -> 308816, mallocs 1204 -> 1205
- claude launch: cli_cpu_us 10869 -> 9109, cli_footprint_kib 3425 -> 3473, cli_maxrss_kib 7008 -> 7072, frees 2693 -> 2700, malloc_bytes 521932 -> 521874, mallocs 3051 -> 3055, read_bytes.jobs 925 -> 928, write_bytes.jobs 928 -> 931
- claude approve: cli_cpu_us 7813 -> 7837, cli_maxrss_kib 5184 -> 5168, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 7134 -> 7558, cli_footprint_kib 3153 -> 3137, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 10676 -> 9569, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 7072 -> 7120, frees 2991 -> 2994, malloc_bytes 612892 -> 613332, mallocs 3364 -> 3371
- codex approve: cli_cpu_us 8274 -> 8753, cli_footprint_kib 3105 -> 3073, cli_maxrss_kib 5200 -> 5184, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 9290 -> 9614, malloc_bytes 242240 -> 242306, mallocs 732 -> 733
- cli version: cli_cpu_us 6500 -> 7308, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 7966 -> 7160, cli_footprint_kib 3121 -> 3089, cli_maxrss_kib 5216 -> 5200, frees 995 -> 999, malloc_bytes 297439 -> 298102, mallocs 1173 -> 1177, opens 16 -> 18
- acp suspend: cli_cpu_us 8056 -> 7985, cli_footprint_kib 3313 -> 3281, cli_maxrss_kib 5584 -> 5552, frees 9873 -> 9885, malloc_bytes 3463419 -> 3464505, mallocs 14485 -> 14504, read_bytes.store 45797 -> 45867, read_calls.ipc 184 -> 183, write_bytes.jobs 3584 -> 3586, write_bytes.store 112186 -> 112180
- acp move: cli_cpu_us 8391 -> 7758, cli_footprint_kib 3345 -> 3329, cli_maxrss_kib 5776 -> 5744, frees 6493 -> 6490, malloc_bytes 1975443 -> 1975946, mallocs 7930 -> 7929, read_bytes.jobs 4103 -> 4111, read_bytes.store 1113 -> 1114, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 10570 -> 10018, cli_footprint_kib 3489 -> 3441, cli_maxrss_kib 7104 -> 7072, frees 9217 -> 9218, malloc_bytes 4659914 -> 4660060, mallocs 11223 -> 11224, opens 222 -> 224, read_bytes.ipc 5867 -> 5866, read_bytes.jobs 3558 -> 3559, read_bytes.state 270 -> 122, read_bytes.store 16118 -> 16146, read_calls.ipc 202 -> 200, read_calls.state 8 -> 4, write_bytes.jobs 3556 -> 3559
- acp pull: cli_cpu_us 11194 -> 10578, cli_maxrss_kib 7168 -> 7184, frees 23560 -> 23566, malloc_bytes 9090680 -> 9092369, mallocs 30349 -> 30372, opens 427 -> 425, read_bytes.ipc 11995 -> 11996, read_bytes.jobs 8539 -> 8544, read_bytes.state 322 -> 174, read_bytes.store 28170 -> 28343, read_calls.ipc 523 -> 524, read_calls.state 8 -> 4, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8398 -> 8406, write_bytes.store 87926 -> 94591, write_calls.store 55 -> 57
- claude suspend: cli_cpu_us 8448 -> 8128, cli_footprint_kib 3297 -> 3249, cli_maxrss_kib 5552 -> 5520, frees 14805 -> 14804, malloc_bytes 3694582 -> 3694223, mallocs 20427 -> 20424, read_bytes.store 9524 -> 9504, read_calls.ipc 185 -> 184, write_bytes.jobs 5321 -> 5319, write_bytes.store 62996 -> 79464, write_calls.store 37 -> 45
- claude move: cli_cpu_us 8076 -> 8243, cli_maxrss_kib 5776 -> 5760, frees 6436 -> 6429, malloc_bytes 1967455 -> 1967170, mallocs 7864 -> 7857, read_bytes.store 1211 -> 1200, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 11538 -> 11682, frees 9520 -> 9536, malloc_bytes 4793756 -> 4797630, mallocs 11658 -> 11674, read_bytes.ipc 5344 -> 5342, read_bytes.jobs 10818 -> 10815, read_bytes.store 16929 -> 16878, read_calls.ipc 204 -> 207, write_bytes.jobs 5263 -> 5252
- claude pull: cli_cpu_us 10419 -> 11198, frees 29540 -> 29536, malloc_bytes 9681078 -> 9680646, mallocs 38507 -> 38503, read_bytes.ipc 11496 -> 11494, read_bytes.jobs 18533 -> 18522, read_bytes.state 322 -> 174, read_bytes.store 29688 -> 29609, read_calls.ipc 532 -> 526, read_calls.state 8 -> 4, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 12030 -> 12009, write_bytes.store 93108 -> 78244, write_calls.store 55 -> 49
- codex suspend: cli_cpu_us 6231 -> 7178, cli_footprint_kib 3265 -> 3313, cli_maxrss_kib 5536 -> 5584, frees 11310 -> 11365, malloc_bytes 3737180 -> 3693700, mallocs 16024 -> 15923, read_bytes.ipc 5251 -> 5250, read_bytes.store 9848 -> 9830, write_bytes.jobs 6426 -> 6445, write_bytes.store 79844 -> 96314, write_calls.store 45 -> 53
- codex move: cli_cpu_us 7245 -> 7670, cli_footprint_kib 3297 -> 3409, cli_maxrss_kib 5712 -> 5808, frees 6436 -> 6428, malloc_bytes 1966951 -> 1967261, mallocs 7864 -> 7862, read_bytes.jobs 4090 -> 4094, read_bytes.store 1235 -> 1230, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 10306 -> 11815, cli_footprint_kib 3553 -> 3537, cli_maxrss_kib 7408 -> 7392, frees 9645 -> 9651, malloc_bytes 4896329 -> 4896697, opens 302 -> 304, read_bytes.ipc 5495 -> 5493, read_bytes.jobs 15361 -> 15358, read_bytes.store 17476 -> 17441, read_calls.ipc 204 -> 208, write_bytes.jobs 6426 -> 6433, write_bytes.store 6149 -> 6151
- codex pull: cli_cpu_us 11797 -> 12426, cli_footprint_kib 3521 -> 3537, cli_maxrss_kib 7424 -> 7440, frees 25536 -> 25540, malloc_bytes 9729911 -> 9730854, mallocs 33136 -> 33144, opens 519 -> 521, read_bytes.ipc 11660 -> 11658, read_bytes.jobs 24069 -> 24061, read_bytes.store 30636 -> 30574, read_calls.ipc 542 -> 535, write_bytes.ipc 4181 -> 4180, write_bytes.jobs 14265 -> 14256, write_bytes.store 121447 -> 121434
- cold start: cli_cpu_us 33664 -> 23626, cli_footprint_kib 3153 -> 3089, cli_maxrss_kib 5184 -> 5200, cli_processes 5 -> 3, frees 2975 -> 2311, malloc_bytes 1470624 -> 1050526, mallocs 4125 -> 3258, write_bytes.ipc 668 -> 306, write_calls.ipc 9 -> 5
- restart: cli_cpu_us 27563 -> 38117, cli_footprint_kib 3185 -> 3137, cli_maxrss_kib 5296 -> 5248, frees 6976 -> 6093, malloc_bytes 2485898 -> 2268610, mallocs 10525 -> 9810, write_bytes.ipc 2633 -> 1508, write_calls.ipc 39 -> 23

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13816 | 15568 | 13784 | 15696 | 1.001 | 1.000 to 1.001 |
| b | 130 | 15640 | 15856 | 15528 | 16096 | 0.992 | 0.988 to 0.993 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5217 | 6145 | 5217 | 6257 | 1.005 | 1.001 to 1.009 |
| b | 130 | 6201 | 6417 | 6073 | 6625 | 0.976 | 0.967 to 0.979 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10144 | 10448 | 10064 | 10480 | 0.995 | 0.992 to 1.002 |
| daemon b | 15488 | 16672 | 15536 | 16384 | 1.002 | 0.996 to 1.004 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3248 | 3568 | 3200 | 3617 | 0.996 | 0.985 to 1.015 |
| daemon b | 5921 | 7089 | 5969 | 6785 | 1.005 | 0.991 to 1.013 |
