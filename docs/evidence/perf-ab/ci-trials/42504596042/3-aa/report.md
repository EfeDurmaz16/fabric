### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 254.7 | 246.9 | 0.996 | 0.672 to 1.200 | not shown |
| acp launch | 15 | 28.1 | 25.1 | 0.968 | 0.840 to 1.084 | not shown |
| acp approve | 15 | 6.4 | 6.3 | 0.969 | 0.936 to 1.018 | not shown |
| claude new | 15 | 242.1 | 242.5 | 0.938 | 0.852 to 1.217 | not shown |
| claude launch | 15 | 184.7 | 149.2 | 0.926 | 0.706 to 0.960 | faster |
| claude approve | 15 | 7.6 | 7.5 | 0.986 | 0.880 to 1.047 | not shown |
| codex new | 15 | 254.7 | 242.9 | 0.967 | 0.893 to 1.238 | not shown |
| codex launch | 15 | 218.8 | 249.9 | 1.127 | 0.839 to 1.323 | not shown, median above 1 |
| codex approve | 15 | 8.0 | 8.2 | 1.000 | 0.972 to 1.186 | not shown |
| ledger append | 15 | 8.0 | 7.5 | 0.941 | 0.823 to 1.035 | not shown |
| cli version | 15 | 3.6 | 3.8 | 1.006 | 0.929 to 1.116 | not shown, median above 1 |
| cli ls | 15 | 4.3 | 4.2 | 0.995 | 0.956 to 1.016 | not shown |
| acp suspend | 15 | 481.4 | 473.9 | 1.023 | 0.941 to 1.079 | not shown, median above 1 |
| acp move | 15 | 311.2 | 300.4 | 0.910 | 0.830 to 1.044 | not shown |
| acp resume | 15 | 837.2 | 794.5 | 0.916 | 0.886 to 1.070 | not shown |
| acp pull | 15 | 1650.3 | 1631.7 | 1.009 | 0.955 to 1.043 | not shown, median above 1 |
| claude suspend | 15 | 616.4 | 640.6 | 1.026 | 0.973 to 1.114 | not shown, median above 1 |
| claude move | 15 | 306.5 | 303.1 | 0.948 | 0.907 to 1.019 | not shown |
| claude resume | 15 | 1357.5 | 1266.0 | 0.904 | 0.876 to 0.929 | faster |
| claude pull | 15 | 2385.4 | 2264.4 | 0.951 | 0.913 to 0.967 | faster |
| codex suspend | 15 | 696.8 | 663.6 | 0.938 | 0.903 to 1.039 | not shown |
| codex move | 15 | 292.7 | 314.1 | 1.012 | 0.956 to 1.104 | not shown, median above 1 |
| codex resume | 15 | 1327.4 | 1349.5 | 0.973 | 0.953 to 1.061 | not shown |
| codex pull | 15 | 2238.3 | 2265.9 | 1.033 | 1.011 to 1.082 | slower |
| cold start | 15 | 83.7 | 77.5 | 1.012 | 0.855 to 1.419 | not shown, median above 1 |
| restart | 15 | 142.5 | 146.7 | 1.091 | 0.939 to 1.362 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 6992 -> 7056 | 3985 -> 4049 | 11.8 -> 10.6 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1185 -> 1183 | 3660 -> 3271 | 6960 -> 7008 | 3393 -> 3425 | 10.2 -> 14.5 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5168 -> 5168 | 3073 -> 3089 | 10.0 -> 10.6 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1206 -> 1205 | 5344 -> 5328 | 3201 -> 3169 | 10.3 -> 8.5 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1105 | 3059 -> 3048 | 6976 -> 7056 | 3393 -> 3457 | 17.1 -> 11.6 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 731 | 5184 -> 5184 | 3105 -> 3089 | 9.2 -> 8.2 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5312 -> 5344 | 3169 -> 3201 | 8.9 -> 8.7 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3366 -> 3365 | 7104 -> 7088 | 3409 -> 3393 | 13.1 -> 12.6 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5168 -> 5120 | 3057 -> 3025 | 9.3 -> 8.2 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5248 -> 5184 | 3153 -> 3089 | 7.2 -> 11.2 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2657 -> 2657 | 8.4 -> 7.6 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1178 -> 1173 | 5312 -> 5248 | 3201 -> 3153 | 12.4 -> 11.4 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48174 -> 48211 | 14488 -> 14489 | 5648 -> 5616 | 3361 -> 3329 | 7.3 -> 8.3 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5236 -> 5222 | 7942 -> 7936 | 5760 -> 5760 | 3345 -> 3345 | 8.9 -> 11.3 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19849 -> 19816 | 11616 -> 11229 | 7072 -> 7072 | 3457 -> 3457 | 13.8 -> 10.2 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37095 -> 37115 | 30636 -> 30764 | 7152 -> 7168 | 3489 -> 3505 | 13.1 -> 14.2 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13820 -> 13856 | 20432 -> 20874 | 5568 -> 5568 | 3297 -> 3297 | 8.1 -> 8.6 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5251 -> 5281 | 7855 -> 7856 | 5824 -> 5760 | 3409 -> 3361 | 10.2 -> 9.7 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27989 -> 27899 | 11664 -> 11653 | 7392 -> 7392 | 3521 -> 3521 | 13.5 -> 11.2 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48486 -> 48463 | 38514 -> 38515 | 7456 -> 7456 | 3537 -> 3537 | 12.4 -> 12.6 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15194 -> 15208 | 15820 -> 15800 | 5568 -> 5552 | 3297 -> 3297 | 8.9 -> 10.5 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5334 -> 5324 | 7864 -> 7886 | 5776 -> 5776 | 3377 -> 3361 | 10.5 -> 8.2 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 33086 -> 32931 | 11753 -> 11749 | 7376 -> 7376 | 3521 -> 3521 | 11.5 -> 14.2 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54994 -> 54834 | 33154 -> 33138 | 7456 -> 7440 | 3569 -> 3553 | 15.6 -> 11.5 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3259 -> 4129 | 5184 -> 5184 | 3089 -> 3137 | 23.4 -> 38.7 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 747267 -> 804947 | 10691 -> 9834 | 5232 -> 5248 | 3137 -> 3153 | 44.4 -> 38.8 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 11773 -> 10614, cli_footprint_kib 3985 -> 4049, cli_maxrss_kib 6992 -> 7056, malloc_bytes 13401208 -> 13401339
- acp launch: cli_cpu_us 10250 -> 14531, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 6960 -> 7008, frees 2934 -> 2909, malloc_bytes 635578 -> 533290, mallocs 3660 -> 3271, read_bytes.jobs 1011 -> 1009, write_bytes.jobs 1033 -> 1031
- acp approve: cli_cpu_us 10015 -> 10633, cli_footprint_kib 3073 -> 3089, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 10291 -> 8500, cli_footprint_kib 3201 -> 3169, cli_maxrss_kib 5344 -> 5328, malloc_bytes 309465 -> 308940, mallocs 1206 -> 1205
- claude launch: cli_cpu_us 17142 -> 11556, cli_footprint_kib 3393 -> 3457, cli_maxrss_kib 6976 -> 7056, frees 2703 -> 2693, malloc_bytes 522086 -> 521332, mallocs 3059 -> 3048, read_bytes.jobs 927 -> 931, write_bytes.jobs 930 -> 934
- claude approve: cli_cpu_us 9225 -> 8223, cli_footprint_kib 3105 -> 3089, frees 607 -> 606, malloc_bytes 242047 -> 241987, mallocs 732 -> 731
- codex new: cli_cpu_us 8922 -> 8661, cli_footprint_kib 3169 -> 3201, cli_maxrss_kib 5312 -> 5344, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 13064 -> 12601, cli_footprint_kib 3409 -> 3393, cli_maxrss_kib 7104 -> 7088, frees 2993 -> 2992, malloc_bytes 613028 -> 612959, mallocs 3366 -> 3365
- codex approve: cli_cpu_us 9259 -> 8235, cli_footprint_kib 3057 -> 3025, cli_maxrss_kib 5168 -> 5120, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 7246 -> 11189, cli_footprint_kib 3153 -> 3089, cli_maxrss_kib 5248 -> 5184, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 8418 -> 7635, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 12357 -> 11450, cli_footprint_kib 3201 -> 3153, cli_maxrss_kib 5312 -> 5248, frees 1000 -> 995, malloc_bytes 297759 -> 297442, mallocs 1178 -> 1173
- acp suspend: cli_cpu_us 7345 -> 8306, cli_footprint_kib 3361 -> 3329, cli_maxrss_kib 5648 -> 5616, frees 9871 -> 9872, malloc_bytes 3481716 -> 3482020, mallocs 14488 -> 14489, read_bytes.store 45785 -> 45822, read_calls.ipc 182 -> 183, write_bytes.jobs 3591 -> 3586, write_bytes.store 103972 -> 79224, write_calls.store 57 -> 45
- acp move: cli_cpu_us 8894 -> 11338, frees 6508 -> 6502, malloc_bytes 1976217 -> 1975548, mallocs 7942 -> 7936, read_bytes.store 1129 -> 1115, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 13757 -> 10177, frees 9245 -> 9223, malloc_bytes 4762645 -> 4660619, mallocs 11616 -> 11229, opens 222 -> 224, read_bytes.ipc 5859 -> 5860, read_bytes.store 16169 -> 16136, read_calls.ipc 197 -> 199, write_bytes.jobs 3552 -> 3558, write_bytes.store 6149 -> 6150
- acp pull: cli_cpu_us 13074 -> 14205, cli_footprint_kib 3489 -> 3505, cli_maxrss_kib 7152 -> 7168, frees 23564 -> 23593, malloc_bytes 9091370 -> 9195174, mallocs 30636 -> 30764, opens 425 -> 427, read_bytes.jobs 8546 -> 8542, read_bytes.store 28227 -> 28251, read_calls.ipc 518 -> 523, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8397 -> 8394, write_bytes.store 94533 -> 94539
- claude suspend: cli_cpu_us 8105 -> 8566, frees 14810 -> 14836, malloc_bytes 3695165 -> 3748681, mallocs 20432 -> 20874, read_bytes.store 9499 -> 9535, read_calls.ipc 184 -> 180, write_bytes.jobs 5318 -> 5319, write_bytes.store 87708 -> 87720
- claude move: cli_cpu_us 10163 -> 9720, cli_footprint_kib 3409 -> 3361, cli_maxrss_kib 5824 -> 5760, frees 6424 -> 6425, malloc_bytes 1966786 -> 1966811, mallocs 7855 -> 7856, read_bytes.jobs 4046 -> 4070, read_bytes.store 1205 -> 1211, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 13523 -> 11193, frees 9520 -> 9515, malloc_bytes 4794625 -> 4793517, mallocs 11664 -> 11653, opens 260 -> 258, read_bytes.state 270 -> 122, read_bytes.store 16900 -> 16958, read_calls.ipc 205 -> 204, read_calls.state 8 -> 4, write_bytes.jobs 5267 -> 5264, write_bytes.store 6151 -> 6148
- claude pull: cli_cpu_us 12417 -> 12606, frees 29537 -> 29539, malloc_bytes 9681398 -> 9681499, mallocs 38514 -> 38515, opens 470 -> 468, read_bytes.ipc 11495 -> 11493, read_bytes.jobs 18512 -> 18522, read_bytes.state 322 -> 174, read_bytes.store 29652 -> 29767, read_calls.ipc 538 -> 531, read_calls.state 8 -> 4, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 12002 -> 12016, write_bytes.store 84854 -> 102981, write_calls.store 51 -> 61
- codex suspend: cli_cpu_us 8930 -> 10539, cli_maxrss_kib 5568 -> 5552, frees 11316 -> 11306, malloc_bytes 3675517 -> 3676389, mallocs 15820 -> 15800, read_bytes.store 9822 -> 9836, read_calls.ipc 190 -> 188, write_bytes.jobs 6448 -> 6429, write_bytes.store 112810 -> 121048, write_calls.store 61 -> 65
- codex move: cli_cpu_us 10451 -> 8240, cli_footprint_kib 3377 -> 3361, frees 6435 -> 6459, malloc_bytes 1966951 -> 1970883, mallocs 7864 -> 7886, read_bytes.jobs 4098 -> 4090, read_bytes.store 1236 -> 1234, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 11531 -> 14237, frees 9650 -> 9646, malloc_bytes 4896653 -> 4895987, mallocs 11753 -> 11749, opens 304 -> 302, read_bytes.jobs 15364 -> 15356, read_bytes.state 270 -> 122, read_bytes.store 17452 -> 17453, read_calls.ipc 209 -> 207, read_calls.state 8 -> 4, write_bytes.jobs 6429 -> 6422, write_bytes.store 6150 -> 6148
- codex pull: cli_cpu_us 15634 -> 11482, cli_footprint_kib 3569 -> 3553, cli_maxrss_kib 7456 -> 7440, frees 25556 -> 25548, malloc_bytes 9730830 -> 9729379, mallocs 33154 -> 33138, opens 521 -> 519, read_bytes.ipc 11659 -> 11656, read_bytes.jobs 24070 -> 24064, read_bytes.state 322 -> 174, read_bytes.store 30602 -> 30596, read_calls.ipc 533 -> 539, read_calls.state 8 -> 4, write_bytes.ipc 4181 -> 4180, write_bytes.jobs 14262 -> 14263, write_bytes.store 95105 -> 103338, write_calls.store 57 -> 61
- cold start: cli_cpu_us 23356 -> 38679, cli_footprint_kib 3089 -> 3137, cli_processes 3 -> 5, frees 2312 -> 2979, malloc_bytes 1050578 -> 1471633, mallocs 3259 -> 4129, write_bytes.ipc 306 -> 697, write_calls.ipc 5 -> 9
- restart: cli_cpu_us 44350 -> 38791, cli_footprint_kib 3137 -> 3153, cli_maxrss_kib 5232 -> 5248, cli_processes 5 -> 4, frees 6762 -> 6106, malloc_bytes 2594388 -> 2271323, mallocs 10691 -> 9834, opens 277 -> 261, read_bytes.store 716916 -> 774596, read_calls.store 176 -> 190, write_bytes.ipc 1838 -> 1508, write_calls.ipc 27 -> 23

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13960 | 15872 | 13840 | 15568 | 0.999 | 0.998 to 1.000 |
| b | 130 | 15496 | 16128 | 15840 | 16256 | 1.019 | 1.019 to 1.019 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5481 | 6449 | 5249 | 6129 | 0.997 | 0.995 to 1.000 |
| b | 130 | 6041 | 6673 | 6369 | 6785 | 1.047 | 1.045 to 1.047 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10432 | 10080 | 10464 | 0.998 | 0.998 to 1.003 |
| daemon b | 15328 | 16096 | 15344 | 16448 | 1.005 | 1.000 to 1.007 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3248 | 3569 | 3216 | 3601 | 1.000 | 1.000 to 1.010 |
| daemon b | 5744 | 6480 | 5760 | 6848 | 1.016 | 1.000 to 1.018 |
