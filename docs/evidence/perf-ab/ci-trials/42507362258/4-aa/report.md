### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 266.7 | 262.2 | 1.070 | 0.870 to 1.227 | not shown, median above 1 |
| acp launch | 15 | 43.7 | 39.1 | 0.994 | 0.965 to 1.073 | not shown |
| acp approve | 15 | 8.1 | 7.8 | 0.996 | 0.934 to 1.086 | not shown |
| claude new | 15 | 175.7 | 231.2 | 1.157 | 0.876 to 1.515 | not shown, median above 1 |
| claude launch | 15 | 156.0 | 199.4 | 1.262 | 0.858 to 1.410 | not shown, median above 1 |
| claude approve | 15 | 8.6 | 9.7 | 0.954 | 0.864 to 1.227 | not shown |
| codex new | 15 | 248.4 | 244.1 | 1.073 | 0.615 to 1.306 | not shown, median above 1 |
| codex launch | 15 | 214.1 | 177.8 | 0.970 | 0.661 to 1.086 | not shown |
| codex approve | 15 | 9.3 | 9.2 | 1.061 | 0.884 to 1.125 | not shown, median above 1 |
| ledger append | 15 | 9.6 | 8.1 | 0.942 | 0.820 to 1.020 | not shown |
| cli version | 15 | 3.7 | 3.6 | 0.984 | 0.924 to 1.248 | not shown |
| cli ls | 15 | 5.4 | 4.7 | 1.031 | 0.934 to 1.159 | not shown, median above 1 |
| acp suspend | 15 | 589.8 | 589.2 | 1.054 | 0.926 to 1.078 | not shown, median above 1 |
| acp move | 15 | 348.7 | 337.5 | 0.999 | 0.948 to 1.086 | not shown |
| acp resume | 15 | 1070.8 | 912.4 | 0.959 | 0.846 to 0.993 | faster |
| acp pull | 15 | 2010.1 | 1911.4 | 1.003 | 0.952 to 1.083 | not shown, median above 1 |
| claude suspend | 15 | 777.8 | 730.1 | 1.052 | 0.911 to 1.174 | not shown, median above 1 |
| claude move | 15 | 376.4 | 349.5 | 0.967 | 0.938 to 1.037 | not shown |
| claude resume | 15 | 1439.2 | 1483.6 | 1.018 | 0.992 to 1.106 | not shown, median above 1 |
| claude pull | 15 | 2514.8 | 2606.3 | 0.974 | 0.950 to 1.075 | not shown |
| codex suspend | 15 | 794.7 | 807.2 | 0.996 | 0.955 to 1.067 | not shown |
| codex move | 15 | 364.0 | 339.2 | 0.979 | 0.873 to 0.996 | faster |
| codex resume | 15 | 1496.3 | 1565.0 | 1.087 | 0.852 to 1.124 | not shown, median above 1 |
| codex pull | 15 | 2671.5 | 2595.2 | 0.969 | 0.935 to 1.078 | not shown |
| cold start | 15 | 73.5 | 87.2 | 1.100 | 0.971 to 1.287 | not shown, median above 1 |
| restart | 15 | 207.8 | 180.3 | 1.057 | 0.984 to 1.167 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38478 -> 38480 | 7040 -> 7072 | 4033 -> 4081 | 8.6 -> 8.8 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1182 -> 1185 | 3257 -> 3659 | 7008 -> 6992 | 3425 -> 3425 | 8.3 -> 8.5 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 733 -> 732 | 5184 -> 5184 | 3089 -> 3073 | 6.7 -> 6.5 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1205 -> 1204 | 5344 -> 5344 | 3201 -> 3201 | 6.8 -> 6.9 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1099 -> 1102 | 3057 -> 3048 | 7072 -> 7040 | 3489 -> 3441 | 8.6 -> 8.3 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 732 | 5200 -> 5200 | 3089 -> 3121 | 6.8 -> 6.9 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5360 -> 5312 | 3201 -> 3137 | 6.9 -> 6.9 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3367 -> 3365 | 7072 -> 7104 | 3393 -> 3425 | 9.1 -> 12.4 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5200 | 3105 -> 3121 | 7.1 -> 6.7 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 733 -> 732 | 5184 -> 5248 | 3089 -> 3153 | 6.6 -> 7.1 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2657 -> 2657 | 6.2 -> 6.4 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5216 -> 5232 | 3121 -> 3137 | 7.2 -> 6.7 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48182 -> 48171 | 14500 -> 14495 | 5616 -> 5632 | 3313 -> 3361 | 7.1 -> 6.8 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5218 -> 5227 | 7935 -> 7927 | 5792 -> 5792 | 3393 -> 3393 | 7.0 -> 6.9 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19930 -> 19816 | 11228 -> 11223 | 7088 -> 7088 | 3473 -> 3473 | 8.2 -> 9.2 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37000 -> 37005 | 30380 -> 30361 | 7168 -> 7152 | 3505 -> 3505 | 8.9 -> 9.2 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13824 -> 13810 | 20426 -> 20431 | 5600 -> 5536 | 3345 -> 3249 | 7.1 -> 6.7 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5294 -> 5287 | 7854 -> 7856 | 5760 -> 5744 | 3329 -> 3313 | 7.1 -> 7.4 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27845 -> 27814 | 11656 -> 11654 | 7424 -> 7424 | 3537 -> 3553 | 9.5 -> 10.1 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48344 -> 48429 | 38503 -> 38544 | 7472 -> 7472 | 3569 -> 3569 | 9.5 -> 11.7 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15215 -> 15213 | 15802 -> 15809 | 5520 -> 5584 | 3249 -> 3313 | 6.9 -> 11.7 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5334 -> 5328 | 7861 -> 7863 | 5776 -> 5776 | 3377 -> 3361 | 7.3 -> 7.9 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32937 -> 32928 | 11749 -> 11755 | 7376 -> 7360 | 3505 -> 3505 | 9.9 -> 9.5 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54968 -> 54803 | 33139 -> 33153 | 7472 -> 7456 | 3569 -> 3569 | 10.5 -> 9.9 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3258 -> 3262 | 5200 -> 5248 | 3137 -> 3137 | 19.6 -> 25.4 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 788467 -> 739027 | 10234 -> 8892 | 5168 -> 5248 | 3089 -> 3153 | 33.5 -> 23.6 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 8558 -> 8824, cli_footprint_kib 4033 -> 4081, cli_maxrss_kib 7040 -> 7072, malloc_bytes 13401144 -> 13401480, mallocs 38478 -> 38480
- acp launch: cli_cpu_us 8310 -> 8532, cli_maxrss_kib 7008 -> 6992, frees 2896 -> 2932, malloc_bytes 532333 -> 635845, mallocs 3257 -> 3659, read_bytes.jobs 1008 -> 1011, write_bytes.jobs 1030 -> 1033
- acp approve: cli_cpu_us 6669 -> 6464, cli_footprint_kib 3089 -> 3073, malloc_bytes 242304 -> 242243, mallocs 733 -> 732
- claude new: cli_cpu_us 6778 -> 6915, malloc_bytes 309145 -> 308812, mallocs 1205 -> 1204
- claude launch: cli_cpu_us 8569 -> 8306, cli_footprint_kib 3489 -> 3441, cli_maxrss_kib 7072 -> 7040, frees 2702 -> 2693, malloc_bytes 522076 -> 521325, mallocs 3057 -> 3048, read_bytes.jobs 925 -> 928, write_bytes.jobs 928 -> 931
- claude approve: cli_cpu_us 6828 -> 6897, cli_footprint_kib 3089 -> 3121, frees 606 -> 607, malloc_bytes 241984 -> 242051, mallocs 731 -> 732
- codex new: cli_cpu_us 6934 -> 6868, cli_footprint_kib 3201 -> 3137, cli_maxrss_kib 5360 -> 5312, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 9083 -> 12417, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 7072 -> 7104, frees 2994 -> 2992, malloc_bytes 613092 -> 612965, mallocs 3367 -> 3365
- codex approve: cli_cpu_us 7056 -> 6719, cli_footprint_kib 3105 -> 3121, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 6597 -> 7107, cli_footprint_kib 3089 -> 3153, cli_maxrss_kib 5184 -> 5248, frees 607 -> 606, malloc_bytes 242304 -> 242243, mallocs 733 -> 732
- cli version: cli_cpu_us 6155 -> 6442, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 7154 -> 6722, cli_footprint_kib 3121 -> 3137, cli_maxrss_kib 5216 -> 5232, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 7106 -> 6840, cli_footprint_kib 3313 -> 3361, cli_maxrss_kib 5616 -> 5632, frees 9883 -> 9880, malloc_bytes 3464052 -> 3482258, mallocs 14500 -> 14495, read_bytes.store 45793 -> 45782, read_calls.ipc 184 -> 183, write_bytes.jobs 3589 -> 3591, write_bytes.store 112178 -> 112172
- acp move: cli_cpu_us 7019 -> 6906, frees 6502 -> 6490, malloc_bytes 1975488 -> 1975340, mallocs 7935 -> 7927, read_bytes.jobs 4107 -> 4115, read_bytes.store 1111 -> 1112, write_bytes.ipc 2665 -> 2666
- acp resume: cli_cpu_us 8213 -> 9202, frees 9222 -> 9217, malloc_bytes 4660602 -> 4659881, mallocs 11228 -> 11223, opens 224 -> 222, read_bytes.jobs 3559 -> 3562, read_bytes.state 270 -> 122, read_bytes.store 16101 -> 16132, read_calls.state 8 -> 4, write_bytes.store 6148 -> 6145
- acp pull: cli_cpu_us 8945 -> 9240, cli_maxrss_kib 7168 -> 7152, frees 23591 -> 23567, malloc_bytes 9096063 -> 9091573, mallocs 30380 -> 30361, opens 427 -> 425, read_bytes.ipc 11995 -> 11993, read_bytes.jobs 8546 -> 8542, read_bytes.store 28280 -> 28289, write_bytes.ipc 4592 -> 4593, write_bytes.jobs 8400 -> 8397, write_bytes.store 94586 -> 69852, write_calls.store 57 -> 45
- claude suspend: cli_cpu_us 7087 -> 6719, cli_footprint_kib 3345 -> 3249, cli_maxrss_kib 5600 -> 5536, frees 14804 -> 14811, malloc_bytes 3694501 -> 3694988, mallocs 20426 -> 20431, read_bytes.store 9503 -> 9489, write_bytes.jobs 5319 -> 5330, write_bytes.store 87710 -> 62978, write_calls.store 49 -> 37
- claude move: cli_cpu_us 7119 -> 7410, cli_footprint_kib 3329 -> 3313, cli_maxrss_kib 5760 -> 5744, frees 6423 -> 6428, malloc_bytes 1966771 -> 1967194, mallocs 7854 -> 7856, read_bytes.store 1208 -> 1201, write_bytes.ipc 2329 -> 2330
- claude resume: cli_cpu_us 9513 -> 10112, cli_footprint_kib 3537 -> 3553, frees 9518 -> 9516, malloc_bytes 4793671 -> 4793545, mallocs 11656 -> 11654, read_bytes.jobs 10812 -> 10818, read_bytes.store 16911 -> 16874, write_bytes.jobs 5264 -> 5260, write_bytes.store 6146 -> 6139
- claude pull: cli_cpu_us 9503 -> 11740, frees 29527 -> 29553, malloc_bytes 9680515 -> 9684661, mallocs 38503 -> 38544, read_bytes.jobs 18519 -> 18510, read_bytes.state 174 -> 322, read_bytes.store 29651 -> 29597, read_calls.ipc 533 -> 534, read_calls.state 4 -> 8, write_bytes.ipc 4246 -> 4247, write_bytes.jobs 12010 -> 12003, write_bytes.store 102960 -> 102948
- codex suspend: cli_cpu_us 6930 -> 11652, cli_footprint_kib 3249 -> 3313, cli_maxrss_kib 5520 -> 5584, frees 11310 -> 11315, malloc_bytes 3693755 -> 3678289, mallocs 15802 -> 15809, read_bytes.store 9843 -> 9841, write_bytes.jobs 6438 -> 6442, write_bytes.store 88084 -> 96312, write_calls.store 49 -> 53
- codex move: cli_cpu_us 7280 -> 7851, cli_footprint_kib 3377 -> 3361, frees 6427 -> 6431, malloc_bytes 1967230 -> 1967724, mallocs 7861 -> 7863, read_bytes.jobs 4098 -> 4094, read_bytes.store 1236 -> 1234, write_bytes.ipc 2329 -> 2330
- codex resume: cli_cpu_us 9904 -> 9477, cli_maxrss_kib 7376 -> 7360, malloc_bytes 4895990 -> 4896403, mallocs 11749 -> 11755, read_bytes.jobs 15361 -> 15357, read_bytes.store 17454 -> 17449, read_calls.ipc 208 -> 209, write_bytes.jobs 6422 -> 6426, write_bytes.store 6149 -> 6147
- codex pull: cli_cpu_us 10516 -> 9918, cli_maxrss_kib 7472 -> 7456, frees 25540 -> 25560, malloc_bytes 9731104 -> 9731442, mallocs 33139 -> 33153, read_bytes.jobs 24068 -> 24077, read_bytes.state 322 -> 174, read_bytes.store 30578 -> 30552, read_calls.ipc 539 -> 541, read_calls.state 8 -> 4, write_bytes.ipc 4180 -> 4181, write_bytes.jobs 14253 -> 14274, write_bytes.store 128052 -> 131289, write_calls.store 73 -> 77
- cold start: cli_cpu_us 19574 -> 25355, cli_maxrss_kib 5200 -> 5248, frees 2311 -> 2315, malloc_bytes 1050514 -> 1051186, mallocs 3258 -> 3262
- restart: cli_cpu_us 33527 -> 23617, cli_footprint_kib 3089 -> 3153, cli_maxrss_kib 5168 -> 5248, cli_processes 4 -> 3, frees 6420 -> 5624, malloc_bytes 2382075 -> 2060747, mallocs 10234 -> 8892, opens 277 -> 261, read_bytes.store 758116 -> 708676, read_calls.store 186 -> 174, write_bytes.ipc 1673 -> 1307, write_calls.ipc 25 -> 20

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13784 | 15648 | 13824 | 15744 | 1.003 | 1.002 to 1.004 |
| b | 130 | 15312 | 15856 | 15568 | 16192 | 1.017 | 1.017 to 1.021 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5193 | 6193 | 5233 | 6289 | 1.008 | 1.006 to 1.011 |
| b | 130 | 5841 | 6385 | 6113 | 6737 | 1.047 | 1.047 to 1.055 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10064 | 10464 | 10080 | 10448 | 1.000 | 0.995 to 1.002 |
| daemon b | 15456 | 16608 | 15376 | 16480 | 0.992 | 0.991 to 0.995 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3216 | 3649 | 3216 | 3569 | 1.000 | 0.991 to 1.000 |
| daemon b | 5889 | 7009 | 5809 | 6897 | 0.984 | 0.978 to 0.987 |
