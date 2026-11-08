### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 240.0 | 238.2 | 1.019 | 0.728 to 1.223 | not shown, median above 1 |
| acp launch | 15 | 28.3 | 25.9 | 1.014 | 0.914 to 1.025 | not shown, median above 1 |
| acp approve | 15 | 6.1 | 6.2 | 1.048 | 0.932 to 1.132 | not shown, median above 1 |
| claude new | 15 | 251.4 | 236.9 | 0.991 | 0.651 to 1.082 | not shown |
| claude launch | 15 | 174.3 | 209.9 | 1.279 | 1.002 to 2.014 | slower |
| claude approve | 15 | 7.1 | 6.9 | 0.995 | 0.665 to 1.123 | not shown |
| codex new | 15 | 240.7 | 243.9 | 0.954 | 0.769 to 1.085 | not shown |
| codex launch | 15 | 199.6 | 253.5 | 1.069 | 0.821 to 1.423 | not shown, median above 1 |
| codex approve | 15 | 7.0 | 6.7 | 0.996 | 0.883 to 1.074 | not shown |
| ledger append | 15 | 6.8 | 6.7 | 0.975 | 0.944 to 1.068 | not shown |
| cli version | 15 | 3.0 | 3.0 | 0.976 | 0.942 to 1.074 | not shown |
| cli ls | 15 | 3.7 | 3.8 | 1.005 | 0.981 to 1.052 | not shown, median above 1 |
| acp suspend | 15 | 457.9 | 471.4 | 1.011 | 0.963 to 1.057 | not shown, median above 1 |
| acp move | 15 | 289.5 | 312.1 | 1.022 | 0.955 to 1.133 | not shown, median above 1 |
| acp resume | 15 | 920.4 | 852.6 | 1.004 | 0.968 to 1.090 | not shown, median above 1 |
| acp pull | 15 | 1585.3 | 1590.9 | 0.988 | 0.930 to 1.063 | not shown |
| claude suspend | 15 | 620.2 | 607.0 | 1.008 | 0.934 to 1.053 | not shown, median above 1 |
| claude move | 15 | 297.4 | 300.7 | 0.959 | 0.844 to 1.011 | not shown |
| claude resume | 15 | 1319.3 | 1310.8 | 1.021 | 0.904 to 1.086 | not shown, median above 1 |
| claude pull | 15 | 2228.7 | 2145.4 | 0.987 | 0.922 to 1.008 | not shown |
| codex suspend | 15 | 657.0 | 691.5 | 1.032 | 0.849 to 1.094 | not shown, median above 1 |
| codex move | 15 | 307.2 | 286.5 | 0.961 | 0.881 to 1.017 | not shown |
| codex resume | 15 | 1318.6 | 1360.3 | 1.027 | 0.959 to 1.161 | not shown, median above 1 |
| codex pull | 15 | 2342.4 | 2307.2 | 0.999 | 0.948 to 1.030 | not shown |
| cold start | 15 | 72.3 | 67.7 | 0.872 | 0.590 to 1.175 | not shown |
| restart | 15 | 147.7 | 131.2 | 0.926 | 0.829 to 0.991 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38480 -> 38479 | 7040 -> 7088 | 4033 -> 4081 | 8.9 -> 8.5 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1185 -> 1184 | 3649 -> 3656 | 7024 -> 7008 | 3457 -> 3441 | 7.8 -> 8.8 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5184 | 3089 -> 3089 | 6.4 -> 6.4 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5312 -> 5328 | 3169 -> 3169 | 6.7 -> 6.3 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1249 -> 1098 | 3051 -> 3055 | 7008 -> 7008 | 3409 -> 3425 | 8.7 -> 8.3 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5216 -> 5200 | 3121 -> 3089 | 6.8 -> 6.6 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5328 -> 5312 | 3185 -> 3153 | 6.7 -> 6.5 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3365 -> 3365 | 7088 -> 7104 | 3409 -> 3425 | 8.4 -> 8.8 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5184 | 3105 -> 3089 | 6.4 -> 6.6 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5184 | 3089 -> 3105 | 6.6 -> 6.8 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2657 -> 2657 | 6.1 -> 6.0 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1174 -> 1174 | 5232 -> 5200 | 3137 -> 3073 | 6.8 -> 6.8 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48201 -> 48151 | 14496 -> 14492 | 5552 -> 5600 | 3265 -> 3313 | 7.0 -> 6.8 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5230 -> 5213 | 7936 -> 7918 | 5728 -> 5824 | 3329 -> 3425 | 7.0 -> 7.4 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19801 -> 19768 | 11617 -> 11222 | 7088 -> 7088 | 3457 -> 3457 | 9.1 -> 8.6 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37085 -> 36959 | 30376 -> 30738 | 7168 -> 7152 | 3505 -> 3505 | 9.3 -> 9.0 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13825 -> 13840 | 20866 -> 20427 | 5552 -> 5584 | 3281 -> 3313 | 6.7 -> 7.0 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5291 -> 5277 | 7852 -> 7855 | 5776 -> 5760 | 3361 -> 3345 | 7.2 -> 7.2 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27842 -> 27843 | 11654 -> 11659 | 7408 -> 7408 | 3537 -> 3537 | 10.5 -> 10.9 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48519 -> 48368 | 38490 -> 38508 | 7488 -> 7456 | 3569 -> 3553 | 10.4 -> 10.6 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15215 -> 15196 | 16025 -> 15912 | 5504 -> 5600 | 3233 -> 3345 | 6.8 -> 6.5 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5350 -> 5308 | 7859 -> 7869 | 5760 -> 5776 | 3361 -> 3361 | 7.6 -> 7.3 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32946 -> 32927 | 11754 -> 11754 | 7408 -> 7360 | 3553 -> 3505 | 9.9 -> 9.5 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54974 -> 54955 | 33143 -> 33135 | 7456 -> 7456 | 3569 -> 3569 | 10.5 -> 10.1 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3684 -> 3257 | 5184 -> 5200 | 3105 -> 3121 | 25.9 -> 21.6 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 751387 -> 800827 | 10252 -> 9484 | 5264 -> 5248 | 3185 -> 3153 | 27.4 -> 33.8 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 8887 -> 8463, cli_footprint_kib 4033 -> 4081, cli_maxrss_kib 7040 -> 7088, malloc_bytes 13401544 -> 13401275, mallocs 38480 -> 38479
- acp launch: cli_cpu_us 7831 -> 8846, cli_footprint_kib 3457 -> 3441, cli_maxrss_kib 7024 -> 7008, frees 2923 -> 2930, malloc_bytes 635030 -> 635260, mallocs 3649 -> 3656, read_bytes.jobs 1011 -> 1010, write_bytes.jobs 1033 -> 1032
- acp approve: cli_cpu_us 6448 -> 6411, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 6732 -> 6280, cli_maxrss_kib 5312 -> 5328, malloc_bytes 308809 -> 308812
- claude launch: cli_cpu_us 8651 -> 8322, cli_footprint_kib 3409 -> 3425, frees 2696 -> 2700, malloc_bytes 521932 -> 521605, mallocs 3051 -> 3055, opens 45 -> 43, read_bytes.jobs 927 -> 924, read_bytes.state 322 -> 174, read_calls.state 8 -> 4, write_bytes.jobs 930 -> 927
- claude approve: cli_cpu_us 6786 -> 6638, cli_footprint_kib 3121 -> 3089, cli_maxrss_kib 5216 -> 5200, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 6730 -> 6480, cli_footprint_kib 3185 -> 3153, cli_maxrss_kib 5328 -> 5312, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 8420 -> 8823, cli_footprint_kib 3409 -> 3425, cli_maxrss_kib 7088 -> 7104, malloc_bytes 612956 -> 612963, read_calls.ipc 13 -> 12
- codex approve: cli_cpu_us 6430 -> 6618, cli_footprint_kib 3105 -> 3089, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 6614 -> 6760, cli_footprint_kib 3089 -> 3105, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 6148 -> 5970, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 6810 -> 6817, cli_footprint_kib 3137 -> 3073, cli_maxrss_kib 5232 -> 5200, malloc_bytes 297503 -> 297506
- acp suspend: cli_cpu_us 6953 -> 6786, cli_footprint_kib 3265 -> 3313, cli_maxrss_kib 5552 -> 5600, frees 9887 -> 9883, malloc_bytes 3476846 -> 3482866, mallocs 14496 -> 14492, read_bytes.store 45812 -> 45762, write_bytes.jobs 3589 -> 3587, write_bytes.store 95700 -> 103928, write_calls.store 53 -> 57
- acp move: cli_cpu_us 6998 -> 7370, cli_footprint_kib 3329 -> 3425, cli_maxrss_kib 5728 -> 5824, frees 6503 -> 6478, malloc_bytes 1975460 -> 1975666, mallocs 7936 -> 7918, read_bytes.jobs 4115 -> 4103, read_bytes.store 1115 -> 1110
- acp resume: cli_cpu_us 9143 -> 8615, frees 9246 -> 9216, malloc_bytes 4762745 -> 4659880, mallocs 11617 -> 11222, read_bytes.jobs 3559 -> 3556, read_bytes.store 16120 -> 16090, read_calls.ipc 201 -> 204, write_bytes.jobs 3556 -> 3555, write_bytes.store 6152 -> 6154
- acp pull: cli_cpu_us 9260 -> 9032, cli_maxrss_kib 7168 -> 7152, frees 23572 -> 23582, malloc_bytes 9092671 -> 9193350, mallocs 30376 -> 30738, opens 427 -> 425, read_bytes.ipc 11994 -> 11993, read_bytes.jobs 8547 -> 8552, read_bytes.state 322 -> 174, read_bytes.store 28216 -> 28233, read_calls.ipc 526 -> 521, read_calls.state 8 -> 4, write_bytes.jobs 8402 -> 8409, write_bytes.store 87920 -> 94585, write_calls.store 55 -> 57
- claude suspend: cli_cpu_us 6721 -> 6985, cli_footprint_kib 3281 -> 3313, cli_maxrss_kib 5552 -> 5584, frees 14834 -> 14805, malloc_bytes 3747916 -> 3694544, mallocs 20866 -> 20427, read_bytes.store 9504 -> 9519, write_bytes.jobs 5314 -> 5325, write_bytes.store 79464 -> 95960, write_calls.store 45 -> 53
- claude move: cli_cpu_us 7153 -> 7232, cli_footprint_kib 3361 -> 3345, cli_maxrss_kib 5776 -> 5760, frees 6419 -> 6425, malloc_bytes 1966653 -> 1966619, mallocs 7852 -> 7855, read_bytes.jobs 4086 -> 4070, read_bytes.store 1205 -> 1207
- claude resume: cli_cpu_us 10470 -> 10944, frees 9516 -> 9521, malloc_bytes 4793591 -> 4794282, mallocs 11654 -> 11659, opens 258 -> 260, read_bytes.ipc 5345 -> 5344, read_bytes.jobs 10815 -> 10813, read_bytes.store 16905 -> 16908, write_bytes.store 6150 -> 6151
- claude pull: cli_cpu_us 10395 -> 10620, cli_footprint_kib 3569 -> 3553, cli_maxrss_kib 7488 -> 7456, frees 29522 -> 29539, malloc_bytes 9679932 -> 9681077, mallocs 38490 -> 38508, opens 468 -> 470, read_bytes.jobs 18512 -> 18522, read_bytes.state 322 -> 174, read_bytes.store 29685 -> 29672, read_calls.ipc 535 -> 541, read_calls.state 8 -> 4, write_bytes.jobs 12008 -> 12012, write_bytes.store 102979 -> 94735, write_calls.store 61 -> 57
- codex suspend: cli_cpu_us 6775 -> 6549, cli_footprint_kib 3233 -> 3345, cli_maxrss_kib 5504 -> 5600, frees 11345 -> 11308, malloc_bytes 3676483 -> 3736773, mallocs 16025 -> 15912, read_bytes.store 9843 -> 9824, write_bytes.jobs 6450 -> 6437, write_bytes.store 96320 -> 104576, write_calls.store 53 -> 57
- codex move: cli_cpu_us 7569 -> 7257, cli_maxrss_kib 5760 -> 5776, frees 6428 -> 6439, malloc_bytes 1967188 -> 1967757, mallocs 7859 -> 7869, read_bytes.jobs 4114 -> 4074, read_bytes.store 1236 -> 1234
- codex resume: cli_cpu_us 9897 -> 9481, cli_footprint_kib 3553 -> 3505, cli_maxrss_kib 7408 -> 7360, frees 9651 -> 9645, malloc_bytes 4896735 -> 4896340, opens 304 -> 302, read_bytes.jobs 15358 -> 15363, read_bytes.store 17466 -> 17442, read_calls.ipc 211 -> 213, write_bytes.jobs 6426 -> 6430, write_bytes.store 6154 -> 6155
- codex pull: cli_cpu_us 10510 -> 10093, frees 25541 -> 25534, malloc_bytes 9730643 -> 9729861, mallocs 33143 -> 33135, opens 521 -> 519, read_bytes.jobs 24057 -> 24056, read_bytes.store 30595 -> 30577, read_calls.ipc 541 -> 539, write_bytes.jobs 14243 -> 14250, write_bytes.store 96720 -> 103347, write_calls.store 59 -> 61
- cold start: cli_cpu_us 25938 -> 21615, cli_footprint_kib 3105 -> 3121, cli_maxrss_kib 5184 -> 5200, cli_processes 4 -> 3, frees 2640 -> 2310, malloc_bytes 1260134 -> 1050462, mallocs 3684 -> 3257, write_bytes.ipc 487 -> 306, write_calls.ipc 7 -> 5
- restart: cli_cpu_us 27437 -> 33750, cli_footprint_kib 3185 -> 3153, cli_maxrss_kib 5264 -> 5248, cli_processes 4 -> 5, frees 6432 -> 6409, malloc_bytes 2383935 -> 2348133, mallocs 10252 -> 9484, opens 277 -> 261, read_bytes.store 721036 -> 770476, read_calls.store 177 -> 189

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13936 | 15584 | 13792 | 15696 | 0.998 | 0.996 to 0.999 |
| b | 130 | 15360 | 16016 | 15600 | 16208 | 1.014 | 1.012 to 1.015 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5329 | 6161 | 5225 | 6257 | 1.000 | 0.991 to 1.003 |
| b | 130 | 5905 | 6561 | 6145 | 6753 | 1.037 | 1.029 to 1.038 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10448 | 10096 | 10384 | 1.002 | 0.992 to 1.003 |
| daemon b | 15440 | 16368 | 15504 | 16352 | 1.003 | 1.000 to 1.004 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3232 | 3649 | 3216 | 3537 | 1.000 | 0.970 to 1.014 |
| daemon b | 5872 | 6768 | 5953 | 6785 | 1.013 | 1.003 to 1.015 |
