### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 270.8 | 246.5 | 0.940 | 0.720 to 1.298 | not shown |
| acp launch | 15 | 35.8 | 37.8 | 1.006 | 0.832 to 1.109 | not shown, median above 1 |
| acp approve | 15 | 7.7 | 7.7 | 0.993 | 0.943 to 1.053 | not shown |
| claude new | 15 | 240.7 | 262.5 | 1.055 | 0.996 to 1.218 | not shown, median above 1 |
| claude launch | 15 | 191.1 | 199.6 | 1.085 | 0.823 to 1.193 | not shown, median above 1 |
| claude approve | 15 | 8.3 | 8.8 | 0.975 | 0.954 to 1.037 | not shown |
| codex new | 15 | 251.0 | 239.8 | 1.020 | 0.989 to 1.323 | not shown, median above 1 |
| codex launch | 15 | 172.9 | 251.5 | 1.165 | 0.906 to 1.987 | not shown, median above 1 |
| codex approve | 15 | 8.0 | 8.1 | 1.044 | 0.963 to 1.136 | not shown, median above 1 |
| ledger append | 15 | 8.0 | 8.0 | 0.966 | 0.892 to 1.011 | not shown |
| cli version | 15 | 3.6 | 3.6 | 1.084 | 0.999 to 1.129 | not shown, median above 1 |
| cli ls | 15 | 4.2 | 4.5 | 1.002 | 0.986 to 1.057 | not shown, median above 1 |
| acp suspend | 15 | 517.9 | 524.8 | 1.020 | 0.959 to 1.063 | not shown, median above 1 |
| acp move | 15 | 326.7 | 348.9 | 1.119 | 1.014 to 1.122 | slower |
| acp resume | 15 | 1014.0 | 1014.3 | 1.012 | 0.911 to 1.114 | not shown, median above 1 |
| acp pull | 15 | 1766.2 | 1833.2 | 0.976 | 0.935 to 1.062 | not shown |
| claude suspend | 15 | 704.5 | 682.5 | 0.985 | 0.913 to 1.167 | not shown |
| claude move | 15 | 324.5 | 320.9 | 1.055 | 0.868 to 1.203 | not shown, median above 1 |
| claude resume | 15 | 1419.1 | 1470.5 | 0.993 | 0.975 to 1.079 | not shown |
| claude pull | 15 | 2414.0 | 2467.2 | 1.031 | 0.965 to 1.089 | not shown, median above 1 |
| codex suspend | 15 | 737.5 | 721.9 | 1.046 | 0.915 to 1.107 | not shown, median above 1 |
| codex move | 15 | 319.0 | 312.4 | 0.959 | 0.870 to 1.107 | not shown |
| codex resume | 15 | 1501.4 | 1491.5 | 1.022 | 0.901 to 1.044 | not shown, median above 1 |
| codex pull | 15 | 2405.4 | 2389.3 | 0.987 | 0.941 to 1.079 | not shown |
| cold start | 15 | 67.1 | 69.9 | 1.029 | 0.634 to 1.319 | not shown, median above 1 |
| restart | 15 | 194.2 | 202.0 | 1.113 | 0.908 to 1.250 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38480 | 6944 -> 7088 | 3937 -> 4065 | 10.4 -> 10.2 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1185 -> 1185 | 3658 -> 3656 | 6960 -> 6960 | 3377 -> 3377 | 13.0 -> 12.1 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 733 | 5264 -> 5232 | 3169 -> 3153 | 9.9 -> 8.0 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1206 | 5344 -> 5328 | 3185 -> 3153 | 7.7 -> 8.3 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1102 -> 1100 | 3053 -> 3053 | 7008 -> 7040 | 3425 -> 3441 | 14.0 -> 9.8 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5184 | 3105 -> 3089 | 9.3 -> 9.2 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5328 -> 5328 | 3169 -> 3169 | 8.0 -> 8.8 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3372 -> 3365 | 7104 -> 7104 | 3425 -> 3425 | 11.7 -> 11.9 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5200 | 3121 -> 3105 | 9.2 -> 7.4 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 148 | 732 -> 736 | 5168 -> 5184 | 3073 -> 3105 | 7.0 -> 7.4 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2673 -> 2657 | 7.6 -> 8.1 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5200 -> 5264 | 3105 -> 3137 | 7.4 -> 8.1 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48157 -> 48136 | 14490 -> 14490 | 5600 -> 5584 | 3313 -> 3313 | 8.0 -> 9.2 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5234 -> 5234 | 7933 -> 7934 | 5776 -> 5760 | 3361 -> 3361 | 9.1 -> 8.8 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19795 -> 19731 | 11225 -> 11227 | 7120 -> 7120 | 3489 -> 3489 | 13.5 -> 10.7 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 36922 -> 36782 | 30375 -> 30368 | 7152 -> 7168 | 3489 -> 3505 | 11.7 -> 13.5 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13823 -> 13967 | 20446 -> 20429 | 5568 -> 5568 | 3297 -> 3297 | 7.9 -> 9.4 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5278 -> 5275 | 7861 -> 7854 | 5744 -> 5824 | 3329 -> 3409 | 8.6 -> 9.1 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27832 -> 27820 | 11648 -> 11652 | 7408 -> 7376 | 3521 -> 3505 | 11.1 -> 13.0 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48441 -> 48309 | 38523 -> 38504 | 7456 -> 7488 | 3553 -> 3585 | 16.3 -> 14.5 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15352 -> 15190 | 15921 -> 16038 | 5568 -> 5536 | 3313 -> 3265 | 8.0 -> 7.8 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5334 -> 5481 | 7863 -> 7866 | 5776 -> 5744 | 3377 -> 3345 | 8.0 -> 10.7 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 33081 -> 32904 | 11754 -> 11751 | 7408 -> 7392 | 3537 -> 3521 | 15.6 -> 13.4 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54977 -> 54901 | 33142 -> 33145 | 7456 -> 7472 | 3569 -> 3569 | 15.4 -> 11.5 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3263 -> 3259 | 5216 -> 5200 | 3121 -> 3105 | 37.0 -> 30.1 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 755507 -> 771987 | 10949 -> 9330 | 5248 -> 5216 | 3137 -> 3089 | 46.4 -> 36.0 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 10385 -> 10187, cli_footprint_kib 3937 -> 4065, cli_maxrss_kib 6944 -> 7088, malloc_bytes 13401480 -> 13401547, mallocs 38479 -> 38480
- acp launch: cli_cpu_us 13002 -> 12053, frees 2934 -> 2930, malloc_bytes 635390 -> 635258, mallocs 3658 -> 3656
- acp approve: cli_cpu_us 9882 -> 7971, cli_footprint_kib 3169 -> 3153, cli_maxrss_kib 5264 -> 5232, frees 606 -> 607, malloc_bytes 242240 -> 242307, mallocs 732 -> 733
- claude new: cli_cpu_us 7728 -> 8298, cli_footprint_kib 3185 -> 3153, cli_maxrss_kib 5344 -> 5328, malloc_bytes 308809 -> 309132, mallocs 1204 -> 1206
- claude launch: cli_cpu_us 13976 -> 9787, cli_footprint_kib 3425 -> 3441, cli_maxrss_kib 7008 -> 7040, frees 2693 -> 2697, malloc_bytes 521661 -> 521475, read_bytes.jobs 928 -> 926, write_bytes.jobs 931 -> 929
- claude approve: cli_cpu_us 9324 -> 9217, cli_footprint_kib 3105 -> 3089, cli_maxrss_kib 5200 -> 5184, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 7982 -> 8771, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 11739 -> 11907, malloc_bytes 613193 -> 612967, mallocs 3372 -> 3365
- codex approve: cli_cpu_us 9198 -> 7362, cli_footprint_kib 3121 -> 3105, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 7015 -> 7429, cli_footprint_kib 3073 -> 3105, cli_maxrss_kib 5168 -> 5184, frees 606 -> 610, malloc_bytes 242240 -> 242903, mallocs 732 -> 736, opens 3 -> 5, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli version: cli_cpu_us 7630 -> 8080, cli_footprint_kib 2673 -> 2657, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 7400 -> 8092, cli_footprint_kib 3105 -> 3137, cli_maxrss_kib 5200 -> 5264, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 8047 -> 9211, cli_maxrss_kib 5600 -> 5584, frees 9871 -> 9873, malloc_bytes 3481831 -> 3463702, read_bytes.store 45768 -> 45747, read_calls.ipc 183 -> 180, write_bytes.jobs 3585 -> 3591, write_bytes.store 95730 -> 95688
- acp move: cli_cpu_us 9113 -> 8806, cli_maxrss_kib 5776 -> 5760, frees 6496 -> 6498, malloc_bytes 1976144 -> 1975631, mallocs 7933 -> 7934, read_bytes.jobs 4107 -> 4127, read_bytes.store 1127 -> 1107
- acp resume: cli_cpu_us 13531 -> 10702, frees 9219 -> 9221, malloc_bytes 4660011 -> 4660537, mallocs 11225 -> 11227, opens 222 -> 224, read_bytes.ipc 5860 -> 5859, read_bytes.store 16115 -> 16051, read_calls.ipc 192 -> 200, write_bytes.jobs 3551 -> 3555, write_bytes.store 6148 -> 6150
- acp pull: cli_cpu_us 11737 -> 13513, cli_footprint_kib 3489 -> 3505, cli_maxrss_kib 7152 -> 7168, frees 23574 -> 23555, malloc_bytes 9095894 -> 9092409, mallocs 30375 -> 30368, read_bytes.ipc 11994 -> 11993, read_bytes.jobs 8556 -> 8544, read_bytes.store 28192 -> 28064, read_calls.ipc 509 -> 521, write_bytes.jobs 8418 -> 8394, write_bytes.store 86306 -> 94537, write_calls.store 53 -> 57
- claude suspend: cli_cpu_us 7901 -> 9357, frees 14811 -> 14807, malloc_bytes 3696274 -> 3694616, mallocs 20446 -> 20429, opens 145 -> 147, read_bytes.state 87 -> 235, read_bytes.store 9502 -> 9498, read_calls.ipc 183 -> 182, read_calls.state 2 -> 6, write_bytes.jobs 5311 -> 5317, write_bytes.store 62994 -> 87708, write_calls.store 37 -> 49
- claude move: cli_cpu_us 8606 -> 9112, cli_footprint_kib 3329 -> 3409, cli_maxrss_kib 5744 -> 5824, frees 6435 -> 6425, mallocs 7861 -> 7854, read_bytes.jobs 4074 -> 4070, read_bytes.store 1204 -> 1205
- claude resume: cli_cpu_us 11075 -> 13027, cli_footprint_kib 3521 -> 3505, cli_maxrss_kib 7408 -> 7376, frees 9510 -> 9514, malloc_bytes 4793441 -> 4793484, mallocs 11648 -> 11652, opens 260 -> 258, read_bytes.jobs 10816 -> 10813, read_bytes.store 16894 -> 16885, read_calls.ipc 200 -> 206, write_bytes.jobs 5264 -> 5247, write_bytes.store 6151 -> 6147
- claude pull: cli_cpu_us 16275 -> 14470, cli_footprint_kib 3553 -> 3585, cli_maxrss_kib 7456 -> 7488, frees 29540 -> 29528, malloc_bytes 9682173 -> 9680496, mallocs 38523 -> 38504, opens 470 -> 468, read_bytes.ipc 11492 -> 11495, read_bytes.jobs 18506 -> 18508, read_bytes.state 322 -> 174, read_bytes.store 29613 -> 29627, read_calls.ipc 532 -> 526, read_calls.state 8 -> 4, write_bytes.jobs 11990 -> 11999, write_bytes.store 102960 -> 111209, write_calls.store 61 -> 65
- codex suspend: cli_cpu_us 7997 -> 7834, cli_footprint_kib 3313 -> 3265, cli_maxrss_kib 5568 -> 5536, frees 11307 -> 11366, malloc_bytes 3718853 -> 3806093, mallocs 15921 -> 16038, opens 154 -> 152, read_bytes.state 235 -> 87, read_bytes.store 9832 -> 9818, read_calls.ipc 189 -> 190, read_calls.state 6 -> 2, write_bytes.jobs 6430 -> 6444, write_bytes.store 96326 -> 96308
- codex move: cli_cpu_us 8008 -> 10663, cli_footprint_kib 3377 -> 3345, cli_maxrss_kib 5776 -> 5744, frees 6433 -> 6436, malloc_bytes 1967034 -> 1967693, mallocs 7863 -> 7866, opens 73 -> 75, read_bytes.jobs 4098 -> 4102, read_bytes.state 0 -> 148, read_bytes.store 1236 -> 1231, read_calls.state 0 -> 4
- codex resume: cli_cpu_us 15599 -> 13446, cli_footprint_kib 3537 -> 3521, cli_maxrss_kib 7408 -> 7392, frees 9651 -> 9648, malloc_bytes 4896678 -> 4896092, mallocs 11754 -> 11751, opens 304 -> 302, read_bytes.ipc 5487 -> 5488, read_bytes.jobs 15360 -> 15365, read_bytes.state 270 -> 122, read_bytes.store 17451 -> 17417, read_calls.ipc 211 -> 209, read_calls.state 8 -> 4, write_bytes.jobs 6422 -> 6438, write_bytes.store 6149 -> 6148
- codex pull: cli_cpu_us 15444 -> 11465, cli_maxrss_kib 7456 -> 7472, frees 25544 -> 25545, malloc_bytes 9730551 -> 9730597, mallocs 33142 -> 33145, read_bytes.ipc 11657 -> 11659, read_bytes.jobs 24067 -> 24060, read_bytes.store 30588 -> 30519, read_calls.ipc 535 -> 536, write_bytes.jobs 14263 -> 14257, write_bytes.store 136292 -> 114807, write_calls.store 77 -> 69
- cold start: cli_cpu_us 37040 -> 30112, cli_footprint_kib 3121 -> 3105, cli_maxrss_kib 5216 -> 5200, frees 2317 -> 2313, malloc_bytes 1051861 -> 1051325, mallocs 3263 -> 3259
- restart: cli_cpu_us 46386 -> 36042, cli_footprint_kib 3137 -> 3089, cli_maxrss_kib 5248 -> 5216, cli_processes 5 -> 4, frees 7107 -> 5960, malloc_bytes 2706725 -> 2270975, mallocs 10949 -> 9330, opens 277 -> 261, read_bytes.store 725156 -> 741636, read_calls.store 178 -> 182, write_bytes.ipc 2003 -> 1472, write_calls.ipc 29 -> 22

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13856 | 15648 | 13808 | 15520 | 1.000 | 0.999 to 1.001 |
| b | 130 | 15536 | 15840 | 15584 | 15968 | 1.002 | 1.002 to 1.002 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5257 | 6209 | 5201 | 6097 | 1.003 | 1.000 to 1.004 |
| b | 130 | 6081 | 6385 | 6129 | 6513 | 1.005 | 1.004 to 1.005 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10112 | 10496 | 10064 | 10464 | 0.997 | 0.994 to 1.002 |
| daemon b | 15472 | 16160 | 15280 | 16688 | 1.001 | 0.987 to 1.009 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3297 | 3681 | 3216 | 3633 | 0.980 | 0.970 to 1.004 |
| daemon b | 5905 | 6561 | 5713 | 7105 | 1.003 | 0.965 to 1.026 |
