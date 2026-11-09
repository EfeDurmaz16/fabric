### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 270.7 | 179.7 | 0.737 | 0.673 to 0.894 | faster |
| acp launch | 15 | 37.1 | 32.2 | 0.973 | 0.861 to 1.004 | not shown |
| acp approve | 15 | 7.0 | 7.2 | 0.986 | 0.939 to 1.015 | not shown |
| claude new | 15 | 219.5 | 244.5 | 1.088 | 0.924 to 1.747 | not shown, median above 1 |
| claude launch | 15 | 160.2 | 201.0 | 1.122 | 0.887 to 1.643 | not shown, median above 1 |
| claude approve | 15 | 8.0 | 7.3 | 0.983 | 0.932 to 1.135 | not shown |
| codex new | 15 | 257.9 | 235.8 | 0.959 | 0.878 to 1.060 | not shown |
| codex launch | 15 | 215.9 | 212.9 | 1.081 | 0.796 to 1.351 | not shown, median above 1 |
| codex approve | 15 | 7.5 | 7.9 | 1.032 | 0.913 to 1.101 | not shown, median above 1 |
| ledger append | 15 | 7.2 | 6.9 | 0.985 | 0.922 to 1.063 | not shown |
| cli version | 15 | 3.1 | 3.3 | 1.032 | 0.945 to 1.054 | not shown, median above 1 |
| cli ls | 15 | 4.3 | 4.1 | 0.983 | 0.944 to 1.052 | not shown |
| acp suspend | 15 | 509.4 | 500.5 | 1.043 | 0.979 to 1.099 | not shown, median above 1 |
| acp move | 15 | 309.4 | 306.4 | 0.982 | 0.924 to 1.106 | not shown |
| acp resume | 15 | 942.6 | 928.7 | 0.992 | 0.968 to 1.163 | not shown |
| acp pull | 15 | 1816.3 | 1690.0 | 1.016 | 0.936 to 1.047 | not shown, median above 1 |
| claude suspend | 15 | 634.9 | 711.9 | 1.076 | 0.873 to 1.184 | not shown, median above 1 |
| claude move | 15 | 307.7 | 320.8 | 1.027 | 0.953 to 1.073 | not shown, median above 1 |
| claude resume | 15 | 1418.8 | 1405.2 | 1.002 | 0.928 to 1.095 | not shown, median above 1 |
| claude pull | 15 | 2370.2 | 2260.0 | 1.002 | 0.918 to 1.062 | not shown, median above 1 |
| codex suspend | 15 | 761.5 | 731.6 | 0.966 | 0.902 to 1.076 | not shown |
| codex move | 15 | 330.5 | 316.5 | 0.995 | 0.898 to 1.063 | not shown |
| codex resume | 15 | 1501.0 | 1368.6 | 1.022 | 0.876 to 1.046 | not shown, median above 1 |
| codex pull | 15 | 2429.7 | 2429.9 | 0.976 | 0.912 to 1.044 | not shown |
| cold start | 15 | 70.2 | 55.2 | 1.011 | 0.499 to 1.189 | not shown, median above 1 |
| restart | 15 | 147.5 | 142.2 | 1.075 | 0.872 to 1.130 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 7024 -> 6992 | 3969 -> 3905 | 9.6 -> 9.4 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1187 -> 1182 | 3650 -> 3657 | 7008 -> 6992 | 3377 -> 3361 | 9.1 -> 10.2 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 733 | 5200 -> 5168 | 3057 -> 3041 | 8.1 -> 7.3 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5344 -> 5280 | 3137 -> 3057 | 7.6 -> 8.4 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1100 | 3051 -> 3053 | 7008 -> 7040 | 3377 -> 3409 | 9.6 -> 11.4 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5104 | 3057 -> 2961 | 8.1 -> 7.9 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5360 -> 5392 | 3137 -> 3201 | 8.4 -> 9.5 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3372 -> 3365 | 7104 -> 7104 | 3377 -> 3377 | 11.8 -> 11.8 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5168 | 3057 -> 3025 | 7.8 -> 7.0 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5168 -> 5184 | 3041 -> 3041 | 8.3 -> 7.8 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 61 -> 0 | 191 -> 189 | 3552 -> 3552 | 2609 -> 2609 | 7.0 -> 7.0 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1177 | 5232 -> 5264 | 3089 -> 3121 | 7.8 -> 8.3 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48237 -> 48259 | 14503 -> 14495 | 5584 -> 5584 | 3265 -> 3265 | 8.8 -> 7.7 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5211 -> 5218 | 7946 -> 7935 | 5760 -> 5792 | 3297 -> 3329 | 9.1 -> 8.8 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19811 -> 20009 | 11225 -> 11242 | 7072 -> 7104 | 3393 -> 3425 | 10.5 -> 10.9 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37011 -> 37161 | 30743 -> 31276 | 7168 -> 7136 | 3473 -> 3441 | 10.8 -> 12.6 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13973 -> 13851 | 20430 -> 20427 | 5568 -> 5584 | 3249 -> 3265 | 8.0 -> 7.8 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5282 -> 5279 | 7852 -> 7852 | 5760 -> 5760 | 3297 -> 3297 | 9.3 -> 8.2 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 28006 -> 27876 | 11666 -> 11650 | 7408 -> 7392 | 3473 -> 3457 | 11.6 -> 12.3 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48515 -> 48367 | 38513 -> 38516 | 7456 -> 7488 | 3505 -> 3521 | 12.2 -> 11.1 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15379 -> 15221 | 16053 -> 15808 | 5536 -> 5520 | 3217 -> 3201 | 8.9 -> 8.1 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5332 -> 5321 | 7866 -> 7858 | 5744 -> 5792 | 3297 -> 3329 | 8.3 -> 7.8 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 33118 -> 32943 | 11749 -> 11753 | 7376 -> 7376 | 3473 -> 3457 | 10.6 -> 12.9 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 55042 -> 54842 | 33149 -> 33151 | 7472 -> 7456 | 3521 -> 3505 | 12.7 -> 12.7 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3691 -> 3262 | 5216 -> 5248 | 3073 -> 3105 | 31.8 -> 27.4 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 776107 -> 726667 | 8898 -> 9764 | 5296 -> 5248 | 3137 -> 3089 | 27.4 -> 35.3 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 9551 -> 9388, cli_footprint_kib 3969 -> 3905, cli_maxrss_kib 7024 -> 6992, malloc_bytes 13401480 -> 13401211
- acp launch: cli_cpu_us 9050 -> 10239, cli_footprint_kib 3377 -> 3361, cli_maxrss_kib 7008 -> 6992, frees 2924 -> 2931, malloc_bytes 635034 -> 635365, mallocs 3650 -> 3657, read_bytes.jobs 1013 -> 1008, write_bytes.jobs 1035 -> 1030
- acp approve: cli_cpu_us 8139 -> 7287, cli_footprint_kib 3057 -> 3041, cli_maxrss_kib 5200 -> 5168, malloc_bytes 242240 -> 242306, mallocs 732 -> 733
- claude new: cli_cpu_us 7613 -> 8419, cli_footprint_kib 3137 -> 3057, cli_maxrss_kib 5344 -> 5280, malloc_bytes 308809 -> 308812
- claude launch: cli_cpu_us 9594 -> 11430, cli_footprint_kib 3377 -> 3409, cli_maxrss_kib 7008 -> 7040, frees 2696 -> 2693, malloc_bytes 521924 -> 521652, mallocs 3051 -> 3053, read_bytes.jobs 927 -> 926, write_bytes.jobs 930 -> 929
- claude approve: cli_cpu_us 8095 -> 7876, cli_footprint_kib 3057 -> 2961, cli_maxrss_kib 5200 -> 5104, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 8377 -> 9527, cli_footprint_kib 3137 -> 3201, cli_maxrss_kib 5360 -> 5392, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 11759 -> 11846, frees 2994 -> 2992, malloc_bytes 613253 -> 612959, mallocs 3372 -> 3365, read_calls.ipc 12 -> 13
- codex approve: cli_cpu_us 7759 -> 7007, cli_footprint_kib 3057 -> 3025, cli_maxrss_kib 5200 -> 5168, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 8276 -> 7816, cli_maxrss_kib 5168 -> 5184, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 7021 -> 6983, frees 178 -> 176, malloc_bytes 87929 -> 87615, mallocs 191 -> 189, opens 1 -> 0, read_bytes.state 61 -> 0, read_calls.state 2 -> 0
- cli ls: cli_cpu_us 7830 -> 8308, cli_footprint_kib 3089 -> 3121, cli_maxrss_kib 5232 -> 5264, frees 995 -> 999, malloc_bytes 297439 -> 298102, mallocs 1173 -> 1177, opens 16 -> 18
- acp suspend: cli_cpu_us 8808 -> 7651, frees 9886 -> 9881, malloc_bytes 3464080 -> 3477479, mallocs 14503 -> 14495, read_bytes.store 45848 -> 45870, write_bytes.jobs 3587 -> 3578, write_bytes.store 103982 -> 95724, write_calls.store 57 -> 53
- acp move: cli_cpu_us 9055 -> 8807, cli_footprint_kib 3297 -> 3329, cli_maxrss_kib 5760 -> 5792, frees 6501 -> 6499, malloc_bytes 1979544 -> 1976127, mallocs 7946 -> 7935, read_bytes.jobs 4099 -> 4091, read_bytes.store 1112 -> 1127, write_bytes.ipc 2665 -> 2666
- acp resume: cli_cpu_us 10491 -> 10873, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 7072 -> 7104, frees 9219 -> 9221, malloc_bytes 4659977 -> 4661755, mallocs 11225 -> 11242, opens 222 -> 224, read_bytes.ipc 5866 -> 5867, read_bytes.jobs 3557 -> 3560, read_bytes.state 122 -> 270, read_bytes.store 16132 -> 16179, read_calls.ipc 197 -> 190, read_calls.state 4 -> 8, write_bytes.jobs 3551 -> 3556, write_bytes.store 6152 -> 6147
- acp pull: cli_cpu_us 10799 -> 12597, cli_footprint_kib 3473 -> 3441, cli_maxrss_kib 7168 -> 7136, frees 23583 -> 23634, malloc_bytes 9193612 -> 9325927, mallocs 30743 -> 31276, read_bytes.jobs 8549 -> 8540, read_bytes.state 174 -> 322, read_bytes.store 28288 -> 28299, read_calls.ipc 522 -> 517, read_calls.state 4 -> 8, write_bytes.ipc 4592 -> 4593, write_bytes.jobs 8402 -> 8396, write_bytes.store 94587 -> 69819, write_calls.store 57 -> 45
- claude suspend: cli_cpu_us 8019 -> 7796, cli_footprint_kib 3249 -> 3265, cli_maxrss_kib 5568 -> 5584, frees 14808 -> 14805, malloc_bytes 3695122 -> 3694507, mallocs 20430 -> 20427, opens 147 -> 145, read_bytes.state 235 -> 87, read_bytes.store 9504 -> 9530, read_calls.state 6 -> 2, write_bytes.jobs 5321 -> 5329, write_bytes.store 62986 -> 62990
- claude move: cli_cpu_us 9267 -> 8222, frees 6421 -> 6417, malloc_bytes 1966702 -> 1967677, read_bytes.jobs 4074 -> 4070, read_bytes.store 1208 -> 1209, write_bytes.ipc 2329 -> 2330
- claude resume: cli_cpu_us 11617 -> 12332, cli_footprint_kib 3473 -> 3457, cli_maxrss_kib 7408 -> 7392, frees 9522 -> 9506, malloc_bytes 4794724 -> 4793188, mallocs 11666 -> 11650, opens 260 -> 258, read_bytes.ipc 5344 -> 5345, read_bytes.jobs 10814 -> 10818, read_bytes.state 270 -> 122, read_bytes.store 16922 -> 16936, read_calls.ipc 205 -> 195, read_calls.state 8 -> 4, write_bytes.jobs 5271 -> 5266, write_bytes.store 6153 -> 6151
- claude pull: cli_cpu_us 12160 -> 11123, cli_footprint_kib 3505 -> 3521, cli_maxrss_kib 7456 -> 7488, frees 29527 -> 29542, malloc_bytes 9682299 -> 9682360, mallocs 38513 -> 38516, read_bytes.ipc 11496 -> 11494, read_bytes.jobs 18538 -> 18506, read_bytes.state 322 -> 174, read_bytes.store 29655 -> 29687, read_calls.ipc 534 -> 529, read_calls.state 8 -> 4, write_bytes.ipc 4246 -> 4247, write_bytes.jobs 12036 -> 11993, write_bytes.store 84865 -> 84864
- codex suspend: cli_cpu_us 8903 -> 8065, cli_footprint_kib 3217 -> 3201, cli_maxrss_kib 5536 -> 5520, frees 11335 -> 11310, malloc_bytes 3806569 -> 3681649, mallocs 16053 -> 15808, opens 154 -> 152, read_bytes.state 235 -> 87, read_bytes.store 9859 -> 9849, read_calls.ipc 190 -> 189, read_calls.state 6 -> 2, write_bytes.jobs 6435 -> 6407, write_bytes.store 96336 -> 96326
- codex move: cli_cpu_us 8274 -> 7769, cli_footprint_kib 3297 -> 3329, cli_maxrss_kib 5744 -> 5792, frees 6434 -> 6425, malloc_bytes 1967805 -> 1966935, mallocs 7866 -> 7858, read_bytes.jobs 4094 -> 4086, read_bytes.store 1238 -> 1235, write_bytes.ipc 2329 -> 2330
- codex resume: cli_cpu_us 10583 -> 12864, cli_footprint_kib 3473 -> 3457, frees 9646 -> 9650, malloc_bytes 4896009 -> 4896668, mallocs 11749 -> 11753, opens 302 -> 304, read_bytes.ipc 5494 -> 5493, read_bytes.state 270 -> 122, read_bytes.store 17488 -> 17461, read_calls.ipc 212 -> 213, read_calls.state 8 -> 4, write_bytes.jobs 6418 -> 6425, write_bytes.store 6153 -> 6152
- codex pull: cli_cpu_us 12694 -> 12670, cli_footprint_kib 3521 -> 3505, cli_maxrss_kib 7472 -> 7456, frees 25550 -> 25544, malloc_bytes 9731679 -> 9732751, mallocs 33149 -> 33151, read_bytes.ipc 11660 -> 11658, read_bytes.jobs 24064 -> 24072, read_bytes.state 322 -> 174, read_bytes.store 30656 -> 30596, read_calls.ipc 529 -> 542, read_calls.state 8 -> 4, write_bytes.ipc 4180 -> 4181, write_bytes.jobs 14250 -> 14267, write_bytes.store 103354 -> 98345
- cold start: cli_cpu_us 31778 -> 27435, cli_footprint_kib 3073 -> 3105, cli_maxrss_kib 5216 -> 5248, cli_processes 4 -> 3, frees 2644 -> 2315, malloc_bytes 1260553 -> 1051562, mallocs 3691 -> 3262, write_bytes.ipc 487 -> 335, write_calls.ipc 7 -> 5
- restart: cli_cpu_us 27438 -> 35296, cli_footprint_kib 3137 -> 3089, cli_maxrss_kib 5296 -> 5248, cli_processes 3 -> 5, frees 5626 -> 6099, malloc_bytes 2061304 -> 2269294, mallocs 8898 -> 9764, read_bytes.store 745756 -> 696316, read_calls.store 183 -> 171, write_bytes.ipc 1307 -> 1508, write_calls.ipc 20 -> 23

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13824 | 15872 | 13936 | 15728 | 0.998 | 0.997 to 1.001 |
| b | 130 | 15472 | 16048 | 15392 | 15888 | 1.000 | 0.999 to 1.000 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5177 | 6369 | 5305 | 6241 | 0.997 | 0.992 to 1.006 |
| b | 130 | 5953 | 6529 | 5873 | 6369 | 1.000 | 0.997 to 1.000 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10112 | 10432 | 10112 | 10448 | 1.000 | 0.997 to 1.002 |
| daemon b | 15568 | 16592 | 15536 | 16496 | 1.006 | 1.001 to 1.013 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3216 | 3569 | 3232 | 3617 | 1.000 | 0.990 to 1.014 |
| daemon b | 6001 | 7009 | 6001 | 6945 | 1.019 | 1.008 to 1.041 |
