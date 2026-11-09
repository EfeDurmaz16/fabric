### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 272.3 | 254.4 | 0.895 | 0.717 to 1.016 | not shown |
| acp launch | 15 | 50.0 | 51.9 | 0.986 | 0.921 to 1.027 | not shown |
| acp approve | 15 | 10.6 | 9.6 | 0.908 | 0.842 to 1.053 | not shown |
| claude new | 15 | 265.9 | 238.1 | 1.012 | 0.713 to 1.072 | not shown, median above 1 |
| claude launch | 15 | 130.9 | 250.8 | 1.322 | 0.954 to 2.139 | not shown, median above 1 |
| claude approve | 15 | 12.2 | 11.5 | 0.975 | 0.865 to 1.097 | not shown |
| codex new | 15 | 261.2 | 267.7 | 1.008 | 0.948 to 1.223 | not shown, median above 1 |
| codex launch | 15 | 251.5 | 216.3 | 0.973 | 0.675 to 1.096 | not shown |
| codex approve | 15 | 10.7 | 11.8 | 1.032 | 0.830 to 1.123 | not shown, median above 1 |
| ledger append | 15 | 11.3 | 11.6 | 0.957 | 0.932 to 1.111 | not shown |
| cli version | 15 | 4.6 | 5.1 | 0.984 | 0.908 to 1.163 | not shown |
| cli ls | 15 | 6.8 | 5.6 | 0.941 | 0.741 to 0.962 | faster |
| acp suspend | 15 | 672.4 | 661.4 | 0.975 | 0.925 to 1.046 | not shown |
| acp move | 15 | 432.7 | 446.1 | 1.028 | 0.896 to 1.116 | not shown, median above 1 |
| acp resume | 15 | 1153.6 | 1206.8 | 1.038 | 0.955 to 1.146 | not shown, median above 1 |
| acp pull | 15 | 2293.5 | 2213.3 | 0.969 | 0.909 to 1.035 | not shown |
| claude suspend | 15 | 863.1 | 826.3 | 0.988 | 0.860 to 1.071 | not shown |
| claude move | 15 | 445.7 | 438.8 | 1.047 | 0.942 to 1.107 | not shown, median above 1 |
| claude resume | 15 | 1739.9 | 1777.5 | 0.988 | 0.899 to 1.068 | not shown |
| claude pull | 15 | 2692.7 | 2866.3 | 1.039 | 0.979 to 1.110 | not shown, median above 1 |
| codex suspend | 15 | 907.2 | 927.1 | 0.995 | 0.853 to 1.081 | not shown |
| codex move | 15 | 462.4 | 447.1 | 1.004 | 0.949 to 1.037 | not shown, median above 1 |
| codex resume | 15 | 1834.2 | 1713.5 | 0.948 | 0.903 to 1.082 | not shown |
| codex pull | 15 | 3024.3 | 3067.4 | 0.963 | 0.933 to 1.038 | not shown |
| cold start | 15 | 87.0 | 96.8 | 1.249 | 0.831 to 1.376 | not shown, median above 1 |
| restart | 15 | 190.4 | 202.1 | 1.035 | 0.867 to 1.179 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 7088 -> 7072 | 4033 -> 4001 | 10.8 -> 17.7 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1188 -> 1185 | 3657 -> 3648 | 6944 -> 7040 | 3329 -> 3409 | 8.9 -> 10.0 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5136 -> 5168 | 2977 -> 3025 | 8.1 -> 7.9 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5296 -> 5328 | 3073 -> 3121 | 9.3 -> 8.4 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1102 | 3057 -> 3049 | 7008 -> 7056 | 3377 -> 3409 | 11.2 -> 9.2 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5184 | 3057 -> 3057 | 7.1 -> 7.1 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5344 -> 5328 | 3137 -> 3121 | 8.7 -> 8.0 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3367 -> 3372 | 7104 -> 7088 | 3361 -> 3345 | 12.4 -> 10.3 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5184 -> 5200 | 3025 -> 3041 | 7.2 -> 7.2 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5232 -> 5216 | 3105 -> 3073 | 10.1 -> 8.8 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2609 -> 2609 | 6.9 -> 7.0 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1177 | 5248 -> 5248 | 3089 -> 3089 | 8.8 -> 7.9 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48169 -> 48225 | 14488 -> 14508 | 5520 -> 5584 | 3185 -> 3249 | 9.7 -> 10.6 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5188 -> 5225 | 7929 -> 7928 | 5808 -> 5824 | 3361 -> 3361 | 9.2 -> 10.8 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19749 -> 19948 | 11224 -> 11224 | 7072 -> 7104 | 3393 -> 3425 | 11.9 -> 12.8 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 36852 -> 36986 | 30762 -> 30360 | 7168 -> 7168 | 3473 -> 3457 | 11.5 -> 11.6 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 14044 -> 13839 | 20429 -> 20431 | 5536 -> 5520 | 3233 -> 3185 | 9.5 -> 11.3 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5290 -> 5288 | 7863 -> 7857 | 5808 -> 5792 | 3345 -> 3329 | 11.5 -> 10.1 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27945 -> 27842 | 11658 -> 11655 | 7408 -> 7376 | 3489 -> 3441 | 15.8 -> 13.1 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48686 -> 48460 | 38510 -> 38521 | 7472 -> 7440 | 3521 -> 3489 | 14.4 -> 16.6 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15188 -> 15186 | 15805 -> 15802 | 5488 -> 5488 | 3169 -> 3153 | 7.8 -> 9.1 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5459 -> 5320 | 7871 -> 7869 | 5760 -> 5792 | 3281 -> 3345 | 9.1 -> 8.9 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32904 -> 33045 | 11754 -> 11753 | 7376 -> 7408 | 3473 -> 3489 | 13.8 -> 12.8 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54929 -> 54902 | 33166 -> 33153 | 7440 -> 7440 | 3505 -> 3489 | 14.9 -> 17.0 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3692 -> 3258 | 5216 -> 5184 | 3073 -> 3057 | 34.4 -> 28.2 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 747267 -> 776107 | 9310 -> 9756 | 5248 -> 5248 | 3105 -> 3089 | 31.1 -> 35.4 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 10763 -> 17744, cli_footprint_kib 4033 -> 4001, cli_maxrss_kib 7088 -> 7072, malloc_bytes 13401272 -> 13401211
- acp launch: cli_cpu_us 8928 -> 10010, cli_footprint_kib 3329 -> 3409, cli_maxrss_kib 6944 -> 7040, frees 2931 -> 2922, malloc_bytes 635322 -> 634968, mallocs 3657 -> 3648, read_bytes.jobs 1014 -> 1011, write_bytes.jobs 1036 -> 1033
- acp approve: cli_cpu_us 8128 -> 7885, cli_footprint_kib 2977 -> 3025, cli_maxrss_kib 5136 -> 5168, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 9321 -> 8396, cli_footprint_kib 3073 -> 3121, cli_maxrss_kib 5296 -> 5328, malloc_bytes 308809 -> 308812
- claude launch: cli_cpu_us 11198 -> 9151, cli_footprint_kib 3377 -> 3409, cli_maxrss_kib 7008 -> 7056, frees 2702 -> 2692, malloc_bytes 522084 -> 521388, mallocs 3057 -> 3049, read_bytes.jobs 926 -> 928, write_bytes.jobs 929 -> 931
- claude approve: cli_cpu_us 7080 -> 7073, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 8696 -> 7987, cli_footprint_kib 3137 -> 3121, cli_maxrss_kib 5344 -> 5328, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 12420 -> 10325, cli_footprint_kib 3361 -> 3345, cli_maxrss_kib 7104 -> 7088, frees 2993 -> 2992, malloc_bytes 613030 -> 613324, mallocs 3367 -> 3372
- codex approve: cli_cpu_us 7180 -> 7243, cli_footprint_kib 3025 -> 3041, cli_maxrss_kib 5184 -> 5200, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 10068 -> 8831, cli_footprint_kib 3105 -> 3073, cli_maxrss_kib 5232 -> 5216, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 6922 -> 6997, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 8755 -> 7854, frees 995 -> 999, malloc_bytes 297439 -> 298102, mallocs 1173 -> 1177, opens 16 -> 18
- acp suspend: cli_cpu_us 9692 -> 10620, cli_footprint_kib 3185 -> 3249, cli_maxrss_kib 5520 -> 5584, frees 9871 -> 9889, malloc_bytes 3476464 -> 3481887, mallocs 14488 -> 14508, read_bytes.store 45780 -> 45836, read_calls.ipc 182 -> 183, write_bytes.jobs 3577 -> 3587, write_bytes.store 112156 -> 95686, write_calls.store 61 -> 53
- acp move: cli_cpu_us 9154 -> 10845, cli_maxrss_kib 5808 -> 5824, frees 6493 -> 6489, malloc_bytes 1975377 -> 1975584, mallocs 7929 -> 7928, read_bytes.jobs 4079 -> 4115, read_bytes.store 1109 -> 1110
- acp resume: cli_cpu_us 11889 -> 12843, cli_footprint_kib 3393 -> 3425, cli_maxrss_kib 7072 -> 7104, malloc_bytes 4659928 -> 4660059, opens 222 -> 224, read_bytes.ipc 5851 -> 5867, read_bytes.jobs 3558 -> 3559, read_bytes.state 122 -> 270, read_bytes.store 16069 -> 16119, read_calls.ipc 200 -> 201, read_calls.state 4 -> 8, write_bytes.jobs 3549 -> 3557, write_bytes.store 6151 -> 6147
- acp pull: cli_cpu_us 11499 -> 11632, cli_footprint_kib 3473 -> 3457, frees 23600 -> 23561, malloc_bytes 9194224 -> 9091425, mallocs 30762 -> 30360, read_bytes.ipc 11993 -> 11995, read_bytes.jobs 8548 -> 8546, read_bytes.store 28130 -> 28266, read_calls.ipc 522 -> 521, write_bytes.jobs 8414 -> 8399, write_bytes.store 86302 -> 86332
- claude suspend: cli_cpu_us 9483 -> 11285, cli_footprint_kib 3233 -> 3185, cli_maxrss_kib 5536 -> 5520, frees 14809 -> 14811, malloc_bytes 3695097 -> 3694530, mallocs 20429 -> 20431, opens 147 -> 145, read_bytes.state 235 -> 87, read_bytes.store 9575 -> 9518, read_calls.state 6 -> 2, write_bytes.jobs 5311 -> 5329, write_bytes.store 63030 -> 79470, write_calls.store 37 -> 45
- claude move: cli_cpu_us 11504 -> 10097, cli_footprint_kib 3345 -> 3329, cli_maxrss_kib 5808 -> 5792, frees 6437 -> 6430, malloc_bytes 1966742 -> 1966661, mallocs 7863 -> 7857, read_bytes.jobs 4070 -> 4082, read_bytes.store 1220 -> 1206
- claude resume: cli_cpu_us 15781 -> 13113, cli_footprint_kib 3489 -> 3441, cli_maxrss_kib 7408 -> 7376, frees 9514 -> 9517, malloc_bytes 4793879 -> 4793669, mallocs 11658 -> 11655, read_bytes.ipc 5331 -> 5343, read_bytes.jobs 10810 -> 10816, read_bytes.store 17013 -> 16904, read_calls.ipc 203 -> 209, write_bytes.jobs 5257 -> 5260, write_bytes.store 6153 -> 6145
- claude pull: cli_cpu_us 14379 -> 16557, cli_footprint_kib 3521 -> 3489, cli_maxrss_kib 7472 -> 7440, frees 29546 -> 29544, malloc_bytes 9680855 -> 9682192, mallocs 38510 -> 38521, read_bytes.ipc 11492 -> 11494, read_bytes.jobs 18524 -> 18488, read_bytes.store 29840 -> 29650, read_calls.ipc 521 -> 529, write_bytes.ipc 4246 -> 4247, write_bytes.jobs 12018 -> 11974, write_bytes.store 78261 -> 89722, write_calls.store 49 -> 57
- codex suspend: cli_cpu_us 7759 -> 9110, cli_footprint_kib 3169 -> 3153, frees 11311 -> 11310, malloc_bytes 3677063 -> 3675896, mallocs 15805 -> 15802, read_bytes.ipc 5250 -> 5251, read_bytes.store 9816 -> 9814, read_calls.ipc 190 -> 189, write_bytes.jobs 6426 -> 6432, write_bytes.store 88080 -> 79818, write_calls.store 49 -> 45
- codex move: cli_cpu_us 9099 -> 8857, cli_footprint_kib 3281 -> 3345, cli_maxrss_kib 5760 -> 5792, frees 6445 -> 6443, malloc_bytes 1967546 -> 1966992, mallocs 7871 -> 7869, opens 75 -> 73, read_bytes.jobs 4078 -> 4090, read_bytes.state 148 -> 0, read_bytes.store 1233 -> 1230, read_calls.state 4 -> 0
- codex resume: cli_cpu_us 13764 -> 12791, cli_footprint_kib 3473 -> 3489, cli_maxrss_kib 7376 -> 7408, frees 9645 -> 9650, malloc_bytes 4896301 -> 4896685, mallocs 11754 -> 11753, opens 302 -> 304, read_bytes.ipc 5481 -> 5493, read_bytes.jobs 15359 -> 15370, read_bytes.state 122 -> 270, read_bytes.store 17423 -> 17405, read_calls.ipc 212 -> 203, read_calls.state 4 -> 8, write_bytes.jobs 6416 -> 6423, write_bytes.store 6150 -> 6149
- codex pull: cli_cpu_us 14902 -> 17005, cli_footprint_kib 3505 -> 3489, frees 25566 -> 25554, malloc_bytes 9735130 -> 9730836, mallocs 33166 -> 33153, read_bytes.ipc 11658 -> 11660, read_bytes.jobs 24071 -> 24073, read_bytes.store 30536 -> 30507, read_calls.ipc 543 -> 530, write_bytes.ipc 4180 -> 4181, write_bytes.store 106570 -> 121424, write_calls.store 65 -> 71
- cold start: cli_cpu_us 34373 -> 28213, cli_footprint_kib 3073 -> 3057, cli_maxrss_kib 5216 -> 5184, cli_processes 4 -> 3, frees 2643 -> 2311, malloc_bytes 1260601 -> 1050526, mallocs 3692 -> 3258, write_bytes.ipc 487 -> 306, write_calls.ipc 7 -> 5
- restart: cli_cpu_us 31081 -> 35363, cli_footprint_kib 3105 -> 3089, cli_processes 3 -> 5, frees 5947 -> 6284, malloc_bytes 2174525 -> 2383750, mallocs 9310 -> 9756, read_bytes.store 716916 -> 745756, read_calls.store 176 -> 183, write_bytes.ipc 1472 -> 1637, write_calls.ipc 22 -> 24

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13816 | 15712 | 13800 | 15648 | 1.000 | 0.998 to 1.000 |
| b | 130 | 15904 | 16208 | 15632 | 15840 | 0.980 | 0.979 to 0.980 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5177 | 6225 | 5161 | 6161 | 1.000 | 0.995 to 1.006 |
| b | 130 | 6369 | 6673 | 6129 | 6321 | 0.955 | 0.951 to 0.955 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10528 | 10112 | 10480 | 1.000 | 0.995 to 1.005 |
| daemon b | 15456 | 16464 | 15504 | 16608 | 0.997 | 0.994 to 1.004 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3248 | 3665 | 3232 | 3617 | 1.000 | 0.980 to 1.014 |
| daemon b | 5889 | 6897 | 5969 | 7057 | 0.995 | 0.988 to 1.018 |
