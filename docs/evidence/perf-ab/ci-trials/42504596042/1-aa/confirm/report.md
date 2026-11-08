### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 254.8 | 212.1 | 0.938 | 0.705 to 1.180 | not shown |
| acp launch | 15 | 23.8 | 24.1 | 1.010 | 0.948 to 1.068 | not shown, median above 1 |
| acp approve | 15 | 5.6 | 5.5 | 1.016 | 0.975 to 1.050 | not shown, median above 1 |
| claude new | 15 | 239.0 | 257.8 | 1.035 | 0.972 to 1.276 | not shown, median above 1 |
| claude launch | 15 | 174.5 | 199.0 | 1.031 | 0.947 to 1.187 | not shown, median above 1 |
| claude approve | 15 | 9.1 | 7.6 | 0.994 | 0.826 to 1.214 | not shown |
| codex new | 15 | 233.0 | 250.7 | 0.989 | 0.884 to 1.216 | not shown |
| codex launch | 15 | 235.4 | 226.4 | 0.918 | 0.741 to 1.254 | not shown |
| codex approve | 15 | 7.2 | 7.0 | 1.000 | 0.897 to 1.181 | not shown |
| ledger append | 15 | 6.4 | 6.3 | 0.969 | 0.913 to 1.034 | not shown |
| cli version | 15 | 3.1 | 3.1 | 0.986 | 0.950 to 1.066 | not shown |
| cli ls | 15 | 3.7 | 3.8 | 1.015 | 0.906 to 1.034 | not shown, median above 1 |
| acp suspend | 15 | 465.3 | 462.1 | 0.987 | 0.874 to 1.110 | not shown |
| acp move | 15 | 295.9 | 307.6 | 1.040 | 0.885 to 1.079 | not shown, median above 1 |
| acp resume | 15 | 847.5 | 838.2 | 1.015 | 0.932 to 1.102 | not shown, median above 1 |
| acp pull | 15 | 1561.6 | 1694.1 | 0.997 | 0.957 to 1.075 | not shown |
| claude suspend | 15 | 626.7 | 651.0 | 0.983 | 0.923 to 1.167 | not shown |
| claude move | 15 | 300.3 | 309.1 | 1.055 | 0.868 to 1.159 | not shown, median above 1 |
| claude resume | 15 | 1285.0 | 1315.0 | 1.040 | 0.972 to 1.048 | not shown, median above 1 |
| claude pull | 15 | 2233.7 | 2281.6 | 0.953 | 0.902 to 1.086 | not shown |
| codex suspend | 15 | 702.9 | 690.2 | 0.974 | 0.889 to 1.048 | not shown |
| codex move | 15 | 290.1 | 291.4 | 1.031 | 0.956 to 1.135 | not shown, median above 1 |
| codex resume | 15 | 1355.6 | 1446.9 | 1.045 | 0.907 to 1.125 | not shown, median above 1 |
| codex pull | 15 | 2292.7 | 2335.8 | 0.997 | 0.979 to 1.074 | not shown |
| cold start | 15 | 51.3 | 59.0 | 0.959 | 0.888 to 1.080 | not shown |
| restart | 15 | 146.6 | 167.1 | 0.915 | 0.807 to 1.250 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38480 -> 38480 | 7088 -> 7040 | 4033 -> 4001 | 8.9 -> 10.0 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1186 -> 1185 | 3658 -> 3658 | 6960 -> 6992 | 3361 -> 3393 | 9.2 -> 10.2 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5168 | 3057 -> 3041 | 7.2 -> 7.0 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5328 -> 5296 | 3153 -> 3089 | 7.0 -> 6.8 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1099 | 3050 -> 3053 | 7056 -> 7024 | 3425 -> 3393 | 9.3 -> 10.4 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5184 | 3057 -> 3057 | 6.9 -> 6.5 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5328 -> 5344 | 3153 -> 3153 | 7.4 -> 7.3 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3365 -> 3371 | 7088 -> 7104 | 3361 -> 3393 | 10.0 -> 10.2 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5168 -> 5200 | 3057 -> 3089 | 7.6 -> 7.2 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 734 | 5184 -> 5248 | 3057 -> 3137 | 7.6 -> 7.5 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2625 -> 2625 | 6.7 -> 6.7 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5248 -> 5232 | 3105 -> 3105 | 7.3 -> 7.5 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48208 -> 48244 | 14495 -> 14493 | 5584 -> 5552 | 3281 -> 3249 | 7.7 -> 8.5 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5212 -> 5206 | 7922 -> 7923 | 5776 -> 5776 | 3329 -> 3329 | 7.8 -> 8.0 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19752 -> 19962 | 11213 -> 11227 | 7104 -> 7088 | 3441 -> 3441 | 9.7 -> 8.9 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37001 -> 37029 | 30767 -> 30750 | 7184 -> 7184 | 3505 -> 3489 | 9.2 -> 9.1 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13849 -> 13849 | 20430 -> 20419 | 5568 -> 5552 | 3249 -> 3265 | 6.9 -> 7.3 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5264 -> 5275 | 7846 -> 7859 | 5840 -> 5744 | 3393 -> 3297 | 7.2 -> 6.6 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27858 -> 27870 | 11660 -> 11643 | 7408 -> 7408 | 3489 -> 3505 | 11.2 -> 12.6 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48517 -> 48513 | 38504 -> 38503 | 7456 -> 7424 | 3521 -> 3457 | 11.5 -> 13.3 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15214 -> 15221 | 15809 -> 15827 | 5552 -> 5552 | 3265 -> 3249 | 7.0 -> 7.4 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5339 -> 5319 | 7867 -> 7869 | 5744 -> 5760 | 3281 -> 3329 | 7.5 -> 8.4 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 33092 -> 32941 | 11750 -> 11750 | 7408 -> 7376 | 3521 -> 3489 | 10.8 -> 10.7 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54984 -> 54844 | 33166 -> 33145 | 7440 -> 7440 | 3505 -> 3521 | 11.5 -> 10.6 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3264 -> 3267 | 5216 -> 5184 | 3105 -> 3057 | 27.0 -> 24.5 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 784347 -> 809067 | 10260 -> 9327 | 5248 -> 5216 | 3105 -> 3105 | 25.0 -> 27.9 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 8903 -> 10015, cli_footprint_kib 4033 -> 4001, cli_maxrss_kib 7088 -> 7040, malloc_bytes 13401541 -> 13401275
- acp launch: cli_cpu_us 9159 -> 10178, cli_footprint_kib 3361 -> 3393, cli_maxrss_kib 6960 -> 6992, malloc_bytes 635366 -> 635644, read_bytes.jobs 1012 -> 1011, write_bytes.jobs 1034 -> 1033
- acp approve: cli_cpu_us 7220 -> 7000, cli_footprint_kib 3057 -> 3041, cli_maxrss_kib 5184 -> 5168, malloc_bytes 242239 -> 242242
- claude new: cli_cpu_us 7016 -> 6802, cli_footprint_kib 3153 -> 3089, cli_maxrss_kib 5328 -> 5296, malloc_bytes 308809 -> 308812
- claude launch: cli_cpu_us 9282 -> 10426, cli_footprint_kib 3425 -> 3393, cli_maxrss_kib 7056 -> 7024, frees 2694 -> 2692, malloc_bytes 521723 -> 521650, mallocs 3050 -> 3053, read_bytes.jobs 926 -> 925, write_bytes.jobs 929 -> 928
- claude approve: cli_cpu_us 6929 -> 6482, malloc_bytes 241983 -> 241986
- codex new: cli_cpu_us 7411 -> 7270, cli_maxrss_kib 5328 -> 5344, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 9970 -> 10163, cli_footprint_kib 3361 -> 3393, cli_maxrss_kib 7088 -> 7104, malloc_bytes 612960 -> 613342, mallocs 3365 -> 3371
- codex approve: cli_cpu_us 7558 -> 7160, cli_footprint_kib 3057 -> 3089, cli_maxrss_kib 5168 -> 5200, malloc_bytes 241984 -> 241986
- ledger append: cli_cpu_us 7625 -> 7478, cli_footprint_kib 3057 -> 3137, cli_maxrss_kib 5184 -> 5248, frees 606 -> 607, malloc_bytes 242240 -> 242902, mallocs 732 -> 734
- cli version: cli_cpu_us 6691 -> 6650, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 7266 -> 7495, cli_maxrss_kib 5248 -> 5232, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 7713 -> 8523, cli_footprint_kib 3281 -> 3249, cli_maxrss_kib 5584 -> 5552, frees 9873 -> 9884, malloc_bytes 3477973 -> 3464169, mallocs 14495 -> 14493, read_bytes.store 45819 -> 45855, read_calls.ipc 184 -> 183, write_bytes.store 95686 -> 103942, write_calls.store 53 -> 57
- acp move: cli_cpu_us 7783 -> 8004, frees 6484 -> 6482, malloc_bytes 1975692 -> 1975546, mallocs 7922 -> 7923, read_bytes.jobs 4103 -> 4095, read_bytes.store 1109 -> 1111, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 9695 -> 8945, cli_maxrss_kib 7104 -> 7088, frees 9207 -> 9221, malloc_bytes 4659098 -> 4660576, mallocs 11213 -> 11227, opens 222 -> 224, read_bytes.ipc 5865 -> 5866, read_bytes.jobs 3557 -> 3560, read_bytes.state 122 -> 270, read_bytes.store 16073 -> 16132, read_calls.ipc 198 -> 200, read_calls.state 4 -> 8, write_bytes.store 6144 -> 6149
- acp pull: cli_cpu_us 9229 -> 9062, cli_footprint_kib 3505 -> 3489, frees 23602 -> 23591, malloc_bytes 9198577 -> 9193943, mallocs 30767 -> 30750, opens 427 -> 425, read_bytes.ipc 11986 -> 11988, read_bytes.jobs 8542 -> 8543, read_bytes.state 322 -> 174, read_bytes.store 28137 -> 28312, read_calls.ipc 519 -> 526, read_calls.state 8 -> 4, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8394 -> 8398, write_bytes.store 69817 -> 94589, write_calls.store 45 -> 57
- claude suspend: cli_cpu_us 6948 -> 7347, cli_footprint_kib 3249 -> 3265, cli_maxrss_kib 5568 -> 5552, frees 14808 -> 14801, malloc_bytes 3694597 -> 3694038, mallocs 20430 -> 20419, write_bytes.jobs 5307 -> 5308, write_bytes.store 79470 -> 87704, write_calls.store 45 -> 49
- claude move: cli_cpu_us 7196 -> 6624, cli_footprint_kib 3393 -> 3297, cli_maxrss_kib 5840 -> 5744, frees 6412 -> 6434, malloc_bytes 1967516 -> 1966555, mallocs 7846 -> 7859, read_bytes.jobs 4058 -> 4066, read_bytes.store 1206 -> 1209, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 11188 -> 12573, cli_footprint_kib 3489 -> 3505, frees 9516 -> 9505, malloc_bytes 4793997 -> 4792752, mallocs 11660 -> 11643, read_bytes.jobs 10816 -> 10821, read_bytes.store 16920 -> 16927, read_calls.ipc 206 -> 209, write_bytes.jobs 5253 -> 5263, write_bytes.store 6151 -> 6149
- claude pull: cli_cpu_us 11516 -> 13276, cli_footprint_kib 3521 -> 3457, cli_maxrss_kib 7456 -> 7424, frees 29537 -> 29535, malloc_bytes 9680306 -> 9681341, mallocs 38504 -> 38503, opens 468 -> 470, read_bytes.jobs 18516 -> 18520, read_bytes.store 29679 -> 29671, read_calls.ipc 531 -> 533, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 12004 -> 12010, write_bytes.store 102966 -> 102967
- codex suspend: cli_cpu_us 7029 -> 7446, cli_footprint_kib 3265 -> 3249, frees 11309 -> 11317, malloc_bytes 3720244 -> 3694343, mallocs 15809 -> 15827, read_bytes.store 9842 -> 9849, read_calls.ipc 189 -> 190, write_bytes.jobs 6438 -> 6428, write_bytes.store 96324 -> 96332
- codex move: cli_cpu_us 7501 -> 8392, cli_footprint_kib 3281 -> 3329, cli_maxrss_kib 5744 -> 5760, frees 6436 -> 6440, malloc_bytes 1967257 -> 1967148, mallocs 7867 -> 7869, read_bytes.jobs 4106 -> 4086, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 10822 -> 10749, cli_footprint_kib 3521 -> 3489, cli_maxrss_kib 7408 -> 7376, malloc_bytes 4896052 -> 4896082, read_bytes.jobs 15358 -> 15356, read_bytes.state 270 -> 122, read_bytes.store 17464 -> 17463, read_calls.ipc 209 -> 210, read_calls.state 8 -> 4, write_bytes.jobs 6426 -> 6415, write_bytes.store 6153 -> 6150
- codex pull: cli_cpu_us 11458 -> 10594, cli_footprint_kib 3505 -> 3521, frees 25539 -> 25533, malloc_bytes 9733931 -> 9731373, mallocs 33166 -> 33145, read_bytes.jobs 24069 -> 24064, read_bytes.state 322 -> 174, read_bytes.store 30593 -> 30606, read_calls.ipc 535 -> 541, read_calls.state 8 -> 4, write_bytes.ipc 4181 -> 4180, write_bytes.jobs 14264 -> 14267, write_bytes.store 96737 -> 113212, write_calls.store 59 -> 67
- cold start: cli_cpu_us 26976 -> 24519, cli_footprint_kib 3105 -> 3057, cli_maxrss_kib 5216 -> 5184, frees 2317 -> 2319, malloc_bytes 1051678 -> 1052241, mallocs 3264 -> 3267
- restart: cli_cpu_us 24961 -> 27949, cli_maxrss_kib 5248 -> 5216, frees 6438 -> 5957, malloc_bytes 2383721 -> 2275444, mallocs 10260 -> 9327, opens 277 -> 261, read_bytes.store 753996 -> 778716, read_calls.store 185 -> 191

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13864 | 15616 | 13864 | 15600 | 1.000 | 0.999 to 1.001 |
| b | 130 | 15440 | 16048 | 15568 | 15984 | 1.006 | 1.006 to 1.007 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5217 | 6145 | 5257 | 6145 | 1.005 | 1.001 to 1.006 |
| b | 130 | 5953 | 6545 | 6065 | 6481 | 1.016 | 1.015 to 1.016 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10496 | 10096 | 10528 | 0.998 | 0.995 to 1.006 |
| daemon b | 15328 | 16352 | 15472 | 16560 | 1.013 | 1.007 to 1.014 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3232 | 3649 | 3265 | 3713 | 1.000 | 0.977 to 1.020 |
| daemon b | 5761 | 6769 | 5921 | 7009 | 1.035 | 1.022 to 1.040 |
