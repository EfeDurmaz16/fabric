### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 237.5 | 262.7 | 1.073 | 0.868 to 1.508 | not shown, median above 1 |
| acp launch | 15 | 32.3 | 33.1 | 1.008 | 0.962 to 1.055 | not shown, median above 1 |
| acp approve | 15 | 7.4 | 7.3 | 1.021 | 0.952 to 1.070 | not shown, median above 1 |
| claude new | 15 | 242.8 | 257.1 | 1.051 | 0.943 to 1.545 | not shown, median above 1 |
| claude launch | 15 | 228.8 | 188.2 | 0.729 | 0.609 to 0.987 | faster |
| claude approve | 15 | 7.8 | 8.2 | 1.013 | 0.936 to 1.214 | not shown, median above 1 |
| codex new | 15 | 244.0 | 259.4 | 1.000 | 0.874 to 1.127 | not shown, median above 1 |
| codex launch | 15 | 210.6 | 224.9 | 0.956 | 0.717 to 1.458 | not shown |
| codex approve | 15 | 7.5 | 7.5 | 1.023 | 0.950 to 1.074 | not shown, median above 1 |
| ledger append | 15 | 7.3 | 7.3 | 1.019 | 0.983 to 1.044 | not shown, median above 1 |
| cli version | 15 | 3.2 | 3.1 | 1.022 | 0.938 to 1.071 | not shown, median above 1 |
| cli ls | 15 | 4.0 | 4.1 | 1.031 | 0.995 to 1.089 | not shown, median above 1 |
| acp suspend | 15 | 496.6 | 492.2 | 1.013 | 0.912 to 1.069 | not shown, median above 1 |
| acp move | 15 | 313.8 | 316.9 | 1.016 | 0.902 to 1.061 | not shown, median above 1 |
| acp resume | 15 | 888.7 | 933.2 | 1.015 | 0.948 to 1.062 | not shown, median above 1 |
| acp pull | 15 | 1706.1 | 1595.1 | 0.978 | 0.918 to 1.000 | faster |
| claude suspend | 15 | 667.5 | 677.4 | 1.017 | 0.899 to 1.168 | not shown, median above 1 |
| claude move | 15 | 290.7 | 299.9 | 1.064 | 0.820 to 1.202 | not shown, median above 1 |
| claude resume | 15 | 1306.1 | 1373.0 | 1.080 | 0.993 to 1.095 | not shown, median above 1 |
| claude pull | 15 | 2312.2 | 2361.1 | 1.002 | 0.973 to 1.067 | not shown, median above 1 |
| codex suspend | 15 | 723.2 | 721.4 | 0.972 | 0.903 to 1.029 | not shown |
| codex move | 15 | 318.7 | 309.2 | 0.995 | 0.906 to 1.052 | not shown |
| codex resume | 15 | 1418.1 | 1399.0 | 1.000 | 0.915 to 1.085 | not shown, median above 1 |
| codex pull | 15 | 2324.1 | 2318.3 | 0.971 | 0.941 to 0.983 | faster |
| cold start | 15 | 103.7 | 70.2 | 0.933 | 0.491 to 1.321 | not shown |
| restart | 15 | 141.1 | 170.9 | 1.085 | 0.992 to 1.191 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 7072 -> 7136 | 4081 -> 4113 | 10.2 -> 11.5 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1188 -> 1186 | 3649 -> 3657 | 7008 -> 7008 | 3409 -> 3409 | 11.6 -> 13.9 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5200 -> 5184 | 3073 -> 3073 | 8.7 -> 8.7 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1206 -> 1205 | 5344 -> 5312 | 3185 -> 3153 | 8.9 -> 9.7 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1102 | 3055 -> 3058 | 7072 -> 7040 | 3473 -> 3441 | 11.0 -> 11.8 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5120 -> 5184 | 3009 -> 3089 | 9.8 -> 8.4 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5248 -> 5248 | 3073 -> 3057 | 10.4 -> 7.9 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3366 -> 3365 | 7056 -> 7104 | 3361 -> 3393 | 11.2 -> 14.3 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5168 | 3089 -> 3057 | 7.7 -> 7.5 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5248 -> 5168 | 3153 -> 3073 | 9.3 -> 8.7 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2641 -> 2641 | 7.5 -> 7.8 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5248 -> 5232 | 3121 -> 3105 | 8.4 -> 11.7 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48166 -> 48161 | 14496 -> 14494 | 5552 -> 5568 | 3249 -> 3265 | 9.4 -> 11.9 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5230 -> 5202 | 7934 -> 7940 | 5792 -> 5808 | 3377 -> 3393 | 9.8 -> 11.9 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19815 -> 19775 | 11223 -> 11223 | 7072 -> 7104 | 3441 -> 3457 | 11.9 -> 13.0 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37072 -> 37025 | 30378 -> 30371 | 7152 -> 7168 | 3473 -> 3489 | 12.0 -> 14.0 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13842 -> 13831 | 20868 -> 20427 | 5568 -> 5584 | 3265 -> 3313 | 9.0 -> 11.6 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5286 -> 5288 | 7880 -> 7857 | 5792 -> 5824 | 3361 -> 3393 | 10.7 -> 8.9 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27864 -> 27853 | 11654 -> 11660 | 7440 -> 7392 | 3537 -> 3505 | 13.7 -> 13.1 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48528 -> 48532 | 38510 -> 38516 | 7472 -> 7472 | 3537 -> 3553 | 15.6 -> 15.7 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15200 -> 15212 | 15797 -> 15919 | 5568 -> 5584 | 3265 -> 3313 | 10.4 -> 8.6 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5323 -> 5327 | 7868 -> 7866 | 5776 -> 5760 | 3361 -> 3329 | 8.3 -> 9.3 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32920 -> 32954 | 11759 -> 11755 | 7392 -> 7392 | 3505 -> 3505 | 14.4 -> 11.6 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54962 -> 54886 | 33144 -> 33147 | 7472 -> 7456 | 3553 -> 3537 | 15.4 -> 14.5 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4129 -> 3258 | 5168 -> 5200 | 3089 -> 3089 | 42.4 -> 30.2 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 767867 -> 767867 | 9765 -> 11122 | 5216 -> 5248 | 3105 -> 3121 | 47.2 -> 55.0 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 10234 -> 11459, cli_footprint_kib 4081 -> 4113, cli_maxrss_kib 7072 -> 7136, malloc_bytes 13401208 -> 13401483
- acp launch: cli_cpu_us 11593 -> 13881, frees 2923 -> 2931, malloc_bytes 635301 -> 635331, mallocs 3649 -> 3657, read_bytes.jobs 1014 -> 1012, write_bytes.jobs 1036 -> 1034
- acp approve: cli_cpu_us 8668 -> 8736, cli_maxrss_kib 5200 -> 5184, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 8938 -> 9693, cli_footprint_kib 3185 -> 3153, cli_maxrss_kib 5344 -> 5312, malloc_bytes 309465 -> 309148, mallocs 1206 -> 1205
- claude launch: cli_cpu_us 11047 -> 11765, cli_footprint_kib 3473 -> 3441, cli_maxrss_kib 7072 -> 7040, frees 2700 -> 2698, malloc_bytes 521932 -> 522016, mallocs 3055 -> 3058, read_bytes.jobs 926 -> 928, write_bytes.jobs 929 -> 931
- claude approve: cli_cpu_us 9804 -> 8433, cli_footprint_kib 3009 -> 3089, cli_maxrss_kib 5120 -> 5184, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 10403 -> 7932, cli_footprint_kib 3073 -> 3057, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 11160 -> 14322, cli_footprint_kib 3361 -> 3393, cli_maxrss_kib 7056 -> 7104, frees 2993 -> 2992, malloc_bytes 613020 -> 612967, mallocs 3366 -> 3365
- codex approve: cli_cpu_us 7667 -> 7518, cli_footprint_kib 3089 -> 3057, cli_maxrss_kib 5200 -> 5168, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 9277 -> 8678, cli_footprint_kib 3153 -> 3073, cli_maxrss_kib 5248 -> 5168, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 7505 -> 7803, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 8439 -> 11657, cli_footprint_kib 3121 -> 3105, cli_maxrss_kib 5248 -> 5232, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 9362 -> 11883, cli_footprint_kib 3249 -> 3265, cli_maxrss_kib 5552 -> 5568, frees 9877 -> 9872, malloc_bytes 3481925 -> 3483414, mallocs 14496 -> 14494, read_bytes.store 45777 -> 45772, write_bytes.jobs 3595 -> 3580, write_bytes.store 103964 -> 112172, write_calls.store 57 -> 61
- acp move: cli_cpu_us 9831 -> 11875, cli_footprint_kib 3377 -> 3393, cli_maxrss_kib 5792 -> 5808, frees 6500 -> 6507, malloc_bytes 1975469 -> 1976593, mallocs 7934 -> 7940, read_bytes.jobs 4115 -> 4091, read_bytes.store 1115 -> 1111
- acp resume: cli_cpu_us 11873 -> 13047, cli_footprint_kib 3441 -> 3457, cli_maxrss_kib 7072 -> 7104, malloc_bytes 4659942 -> 4659916, read_bytes.ipc 5865 -> 5867, read_bytes.jobs 3560 -> 3559, read_bytes.store 16133 -> 16094, write_bytes.jobs 3558 -> 3553, write_bytes.store 6157 -> 6148
- acp pull: cli_cpu_us 12013 -> 13980, cli_footprint_kib 3473 -> 3489, cli_maxrss_kib 7152 -> 7168, frees 23574 -> 23579, malloc_bytes 9092219 -> 9091969, mallocs 30378 -> 30371, read_bytes.ipc 11994 -> 11992, read_bytes.jobs 8538 -> 8541, read_bytes.store 28212 -> 28162, read_calls.ipc 522 -> 520, write_bytes.jobs 8395 -> 8397, write_bytes.store 87919 -> 94540, write_calls.store 55 -> 57
- claude suspend: cli_cpu_us 8999 -> 11567, cli_footprint_kib 3265 -> 3313, cli_maxrss_kib 5568 -> 5584, frees 14835 -> 14807, malloc_bytes 3748538 -> 3694448, mallocs 20868 -> 20427, read_bytes.store 9521 -> 9510, write_bytes.jobs 5328 -> 5314, write_bytes.store 63000 -> 79468, write_calls.store 37 -> 45
- claude move: cli_cpu_us 10683 -> 8860, cli_footprint_kib 3361 -> 3393, cli_maxrss_kib 5792 -> 5824, frees 6455 -> 6430, malloc_bytes 1971153 -> 1967139, mallocs 7880 -> 7857, read_bytes.jobs 4078 -> 4082, read_bytes.store 1208 -> 1206
- claude resume: cli_cpu_us 13678 -> 13104, cli_footprint_kib 3537 -> 3505, cli_maxrss_kib 7440 -> 7392, malloc_bytes 4793606 -> 4794000, mallocs 11654 -> 11660, read_bytes.jobs 10814 -> 10817, read_bytes.store 16928 -> 16914, read_calls.ipc 201 -> 203, write_bytes.jobs 5276 -> 5249, write_bytes.store 6146 -> 6150
- claude pull: cli_cpu_us 15621 -> 15675, cli_footprint_kib 3537 -> 3553, frees 29538 -> 29544, malloc_bytes 9681164 -> 9681387, mallocs 38510 -> 38516, read_bytes.ipc 11495 -> 11496, read_bytes.jobs 18518 -> 18525, read_bytes.store 29688 -> 29685, read_calls.ipc 529 -> 527, write_bytes.jobs 12006 -> 12007, write_bytes.store 102959 -> 96353, write_calls.store 61 -> 59
- codex suspend: cli_cpu_us 10445 -> 8609, cli_footprint_kib 3265 -> 3313, cli_maxrss_kib 5568 -> 5584, frees 11307 -> 11306, malloc_bytes 3679622 -> 3676128, mallocs 15797 -> 15919, read_bytes.store 9828 -> 9840, write_bytes.jobs 6433 -> 6438, write_bytes.store 88070 -> 71620, write_calls.store 49 -> 41
- codex move: cli_cpu_us 8350 -> 9288, cli_footprint_kib 3361 -> 3329, cli_maxrss_kib 5776 -> 5760, frees 6439 -> 6435, malloc_bytes 1967096 -> 1968219, mallocs 7868 -> 7866, read_bytes.store 1233 -> 1237
- codex resume: cli_cpu_us 14429 -> 11555, frees 9650 -> 9646, malloc_bytes 4897061 -> 4896403, mallocs 11759 -> 11755, opens 304 -> 302, read_bytes.jobs 15360 -> 15356, read_bytes.store 17438 -> 17476, read_calls.ipc 205 -> 202, write_bytes.jobs 6417 -> 6421, write_bytes.store 6148 -> 6151
- codex pull: cli_cpu_us 15399 -> 14501, cli_footprint_kib 3553 -> 3537, cli_maxrss_kib 7472 -> 7456, malloc_bytes 9730701 -> 9730323, mallocs 33144 -> 33147, opens 521 -> 519, read_bytes.ipc 11659 -> 11658, read_bytes.jobs 24079 -> 24065, read_bytes.state 322 -> 174, read_bytes.store 30561 -> 30647, read_calls.ipc 535 -> 538, read_calls.state 8 -> 4, write_bytes.jobs 14276 -> 14254, write_bytes.store 98317 -> 96719, write_calls.store 61 -> 59
- cold start: cli_cpu_us 42395 -> 30191, cli_maxrss_kib 5168 -> 5200, cli_processes 5 -> 3, frees 2979 -> 2311, malloc_bytes 1471615 -> 1050516, mallocs 4129 -> 3258, write_bytes.ipc 697 -> 306, write_calls.ipc 9 -> 5
- restart: cli_cpu_us 47161 -> 54957, cli_footprint_kib 3105 -> 3121, cli_maxrss_kib 5216 -> 5248, cli_processes 5 -> 6, frees 6286 -> 7101, malloc_bytes 2481126 -> 2802863, mallocs 9765 -> 11122, opens 261 -> 293, write_bytes.ipc 1841 -> 2003, write_calls.ipc 27 -> 29

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13968 | 15728 | 13984 | 15664 | 0.998 | 0.995 to 0.999 |
| b | 130 | 15568 | 16080 | 15344 | 15872 | 0.988 | 0.987 to 0.989 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5377 | 6305 | 5377 | 6209 | 0.988 | 0.984 to 0.991 |
| b | 130 | 6113 | 6625 | 5905 | 6433 | 0.971 | 0.971 to 0.974 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10128 | 10400 | 10080 | 10560 | 0.998 | 0.995 to 1.002 |
| daemon b | 15392 | 16608 | 15472 | 16624 | 1.003 | 0.989 to 1.005 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3232 | 3520 | 3216 | 3729 | 1.000 | 0.971 to 1.005 |
| daemon b | 5825 | 7025 | 5905 | 7041 | 1.005 | 0.972 to 1.014 |
