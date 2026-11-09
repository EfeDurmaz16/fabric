### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 198.3 | 261.6 | 1.291 | 1.005 to 1.584 | slower |
| acp launch | 15 | 32.9 | 33.0 | 0.995 | 0.968 to 1.166 | not shown |
| acp approve | 15 | 7.8 | 7.3 | 0.994 | 0.930 to 1.058 | not shown |
| claude new | 15 | 255.1 | 195.3 | 0.811 | 0.783 to 1.042 | not shown |
| claude launch | 15 | 142.9 | 161.3 | 0.995 | 0.846 to 1.331 | not shown |
| claude approve | 15 | 9.4 | 8.5 | 1.006 | 0.799 to 1.275 | not shown, median above 1 |
| codex new | 15 | 266.7 | 266.7 | 1.068 | 0.664 to 1.106 | not shown, median above 1 |
| codex launch | 15 | 195.7 | 207.3 | 1.008 | 0.674 to 1.761 | not shown, median above 1 |
| codex approve | 15 | 7.9 | 8.2 | 0.982 | 0.900 to 1.118 | not shown |
| ledger append | 15 | 8.6 | 8.9 | 1.031 | 0.963 to 1.051 | not shown, median above 1 |
| cli version | 15 | 3.7 | 3.7 | 1.023 | 0.909 to 1.139 | not shown, median above 1 |
| cli ls | 15 | 4.4 | 4.7 | 0.983 | 0.932 to 1.027 | not shown |
| acp suspend | 15 | 516.4 | 533.0 | 1.058 | 0.983 to 1.098 | not shown, median above 1 |
| acp move | 15 | 329.1 | 336.6 | 1.023 | 0.924 to 1.119 | not shown, median above 1 |
| acp resume | 15 | 968.5 | 933.4 | 0.979 | 0.842 to 1.070 | not shown |
| acp pull | 15 | 1746.6 | 1766.9 | 1.023 | 0.971 to 1.068 | not shown, median above 1 |
| claude suspend | 15 | 731.4 | 664.7 | 0.947 | 0.843 to 1.081 | not shown |
| claude move | 15 | 326.0 | 335.5 | 1.008 | 0.929 to 1.138 | not shown, median above 1 |
| claude resume | 15 | 1425.2 | 1384.3 | 0.953 | 0.838 to 1.091 | not shown |
| claude pull | 15 | 2406.4 | 2240.8 | 0.990 | 0.895 to 1.009 | not shown |
| codex suspend | 15 | 708.6 | 672.4 | 0.973 | 0.865 to 1.027 | not shown |
| codex move | 15 | 325.7 | 323.5 | 1.049 | 0.954 to 1.113 | not shown, median above 1 |
| codex resume | 15 | 1415.3 | 1516.9 | 1.051 | 0.951 to 1.163 | not shown, median above 1 |
| codex pull | 15 | 2553.7 | 2421.1 | 0.949 | 0.880 to 0.997 | faster |
| cold start | 15 | 105.8 | 94.7 | 0.764 | 0.666 to 1.107 | not shown |
| restart | 15 | 159.1 | 151.7 | 1.009 | 0.837 to 1.115 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38479 | 7040 -> 7152 | 4033 -> 4145 | 13.1 -> 15.5 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1184 -> 1185 | 3656 -> 3657 | 6960 -> 6976 | 3377 -> 3393 | 16.2 -> 15.2 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5136 -> 5200 | 3025 -> 3121 | 11.4 -> 10.2 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1205 -> 1205 | 5296 -> 5328 | 3153 -> 3153 | 11.5 -> 9.4 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1102 | 3055 -> 3052 | 7024 -> 7024 | 3425 -> 3441 | 11.4 -> 12.7 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 731 | 5200 -> 5184 | 3105 -> 3073 | 10.2 -> 15.1 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5344 -> 5344 | 3185 -> 3169 | 11.5 -> 9.3 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3367 -> 3372 | 7104 -> 7104 | 3425 -> 3409 | 14.3 -> 13.1 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 731 | 5184 -> 5168 | 3089 -> 3089 | 9.2 -> 9.6 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5232 -> 5184 | 3137 -> 3089 | 13.7 -> 9.7 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2673 -> 2673 | 8.9 -> 11.2 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5296 -> 5184 | 3185 -> 3041 | 11.7 -> 11.8 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48213 -> 48164 | 14506 -> 14486 | 5584 -> 5584 | 3313 -> 3313 | 13.8 -> 11.9 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5226 -> 5258 | 7934 -> 7931 | 5776 -> 5792 | 3345 -> 3345 | 12.4 -> 11.0 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19865 -> 19779 | 11225 -> 11228 | 7072 -> 7088 | 3441 -> 3457 | 13.4 -> 11.4 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37222 -> 37022 | 30353 -> 30376 | 7184 -> 7136 | 3521 -> 3489 | 14.5 -> 13.8 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13831 -> 13816 | 20426 -> 20868 | 5552 -> 5584 | 3281 -> 3297 | 8.7 -> 9.3 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5275 -> 5291 | 7860 -> 7858 | 5760 -> 5808 | 3345 -> 3377 | 9.8 -> 9.4 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27853 -> 27837 | 11658 -> 11656 | 7392 -> 7424 | 3521 -> 3537 | 14.0 -> 14.2 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48550 -> 48338 | 38509 -> 38499 | 7504 -> 7456 | 3585 -> 3553 | 12.3 -> 12.2 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15372 -> 15203 | 15917 -> 16041 | 5552 -> 5488 | 3281 -> 3217 | 8.3 -> 10.3 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5331 -> 5327 | 7868 -> 7871 | 5776 -> 5792 | 3361 -> 3393 | 9.5 -> 9.6 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 33091 -> 32927 | 11773 -> 11754 | 7392 -> 7376 | 3521 -> 3521 | 12.8 -> 12.9 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 55014 -> 54827 | 33168 -> 33150 | 7440 -> 7408 | 3537 -> 3505 | 14.2 -> 14.1 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3260 -> 3263 | 5200 -> 5184 | 3153 -> 3089 | 29.6 -> 28.3 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 788467 -> 743147 | 9321 -> 9747 | 5216 -> 5248 | 3121 -> 3137 | 42.3 -> 43.5 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 13126 -> 15533, cli_footprint_kib 4033 -> 4145, cli_maxrss_kib 7040 -> 7152, malloc_bytes 13401480 -> 13401483
- acp launch: cli_cpu_us 16168 -> 15192, cli_footprint_kib 3377 -> 3393, cli_maxrss_kib 6960 -> 6976, frees 2930 -> 2931, malloc_bytes 635271 -> 635324, mallocs 3656 -> 3657, read_bytes.jobs 1010 -> 1011, read_calls.ipc 11 -> 12, write_bytes.jobs 1032 -> 1033
- acp approve: cli_cpu_us 11447 -> 10247, cli_footprint_kib 3025 -> 3121, cli_maxrss_kib 5136 -> 5200, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 11455 -> 9418, cli_maxrss_kib 5296 -> 5328, malloc_bytes 308841 -> 308876
- claude launch: cli_cpu_us 11399 -> 12700, cli_footprint_kib 3425 -> 3441, frees 2700 -> 2695, malloc_bytes 521578 -> 521693, mallocs 3055 -> 3052, read_bytes.jobs 926 -> 928, write_bytes.jobs 929 -> 931
- claude approve: cli_cpu_us 10235 -> 15137, cli_footprint_kib 3105 -> 3073, cli_maxrss_kib 5200 -> 5184, frees 607 -> 606, malloc_bytes 242048 -> 241987, mallocs 732 -> 731
- codex new: cli_cpu_us 11474 -> 9329, cli_footprint_kib 3185 -> 3169, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 14331 -> 13144, cli_footprint_kib 3425 -> 3409, frees 2994 -> 2999, malloc_bytes 613084 -> 613351, mallocs 3367 -> 3372
- codex approve: cli_cpu_us 9206 -> 9602, cli_maxrss_kib 5184 -> 5168, frees 607 -> 606, malloc_bytes 242048 -> 241987, mallocs 732 -> 731
- ledger append: cli_cpu_us 13653 -> 9719, cli_footprint_kib 3137 -> 3089, cli_maxrss_kib 5232 -> 5184, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 8921 -> 11156, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 11749 -> 11818, cli_footprint_kib 3185 -> 3041, cli_maxrss_kib 5296 -> 5184, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 13815 -> 11938, frees 9884 -> 9871, malloc_bytes 3478829 -> 3464049, mallocs 14506 -> 14486, read_bytes.store 45824 -> 45775, write_bytes.jobs 3580 -> 3590, write_bytes.store 95696 -> 112172, write_calls.store 53 -> 61
- acp move: cli_cpu_us 12395 -> 11024, cli_maxrss_kib 5776 -> 5792, frees 6495 -> 6493, malloc_bytes 1976769 -> 1975540, mallocs 7934 -> 7931, read_bytes.jobs 4099 -> 4131, write_bytes.ipc 2665 -> 2666
- acp resume: cli_cpu_us 13417 -> 11392, cli_footprint_kib 3441 -> 3457, cli_maxrss_kib 7072 -> 7088, frees 9219 -> 9222, malloc_bytes 4660044 -> 4660633, mallocs 11225 -> 11228, opens 222 -> 224, read_bytes.ipc 5867 -> 5866, read_bytes.jobs 3559 -> 3558, read_bytes.store 16184 -> 16099, read_calls.ipc 202 -> 196, write_bytes.jobs 3549 -> 3564, write_bytes.store 6152 -> 6149
- acp pull: cli_cpu_us 14499 -> 13824, cli_footprint_kib 3521 -> 3489, cli_maxrss_kib 7184 -> 7136, frees 23561 -> 23569, malloc_bytes 9091147 -> 9093295, mallocs 30353 -> 30376, opens 425 -> 427, read_bytes.ipc 11993 -> 11994, read_bytes.jobs 8547 -> 8537, read_bytes.store 28353 -> 28163, read_calls.ipc 529 -> 519, write_bytes.ipc 4592 -> 4593, write_bytes.jobs 8403 -> 8392, write_bytes.store 94567 -> 69813, write_calls.store 57 -> 45
- claude suspend: cli_cpu_us 8720 -> 9253, cli_footprint_kib 3281 -> 3297, cli_maxrss_kib 5552 -> 5584, frees 14804 -> 14835, malloc_bytes 3694497 -> 3752278, mallocs 20426 -> 20868, read_bytes.store 9510 -> 9495, write_bytes.jobs 5316 -> 5321, write_bytes.store 87720 -> 62992, write_calls.store 49 -> 37
- claude move: cli_cpu_us 9839 -> 9433, cli_footprint_kib 3345 -> 3377, cli_maxrss_kib 5760 -> 5808, frees 6432 -> 6428, malloc_bytes 1967277 -> 1966854, mallocs 7860 -> 7858, read_bytes.jobs 4070 -> 4086, write_bytes.ipc 2329 -> 2330
- claude resume: cli_cpu_us 14043 -> 14169, cli_footprint_kib 3521 -> 3537, cli_maxrss_kib 7392 -> 7424, frees 9520 -> 9518, malloc_bytes 4794272 -> 4793688, mallocs 11658 -> 11656, opens 260 -> 258, read_bytes.store 16917 -> 16901, read_calls.ipc 202 -> 210, write_bytes.jobs 5267 -> 5260, write_bytes.store 6152 -> 6149
- claude pull: cli_cpu_us 12278 -> 12169, cli_footprint_kib 3585 -> 3553, cli_maxrss_kib 7504 -> 7456, frees 29542 -> 29529, malloc_bytes 9681078 -> 9680227, mallocs 38509 -> 38499, opens 470 -> 468, read_bytes.jobs 18534 -> 18509, read_bytes.state 322 -> 174, read_bytes.store 29694 -> 29655, read_calls.ipc 533 -> 535, read_calls.state 8 -> 4, write_bytes.ipc 4246 -> 4247, write_bytes.jobs 12027 -> 11999, write_bytes.store 96346 -> 93102, write_calls.store 59 -> 55
- codex suspend: cli_cpu_us 8324 -> 10345, cli_footprint_kib 3281 -> 3217, cli_maxrss_kib 5552 -> 5488, frees 11313 -> 11309, malloc_bytes 3721414 -> 3774003, mallocs 15917 -> 16041, opens 154 -> 152, read_bytes.state 235 -> 87, read_bytes.store 9852 -> 9831, read_calls.state 6 -> 2, write_bytes.jobs 6448 -> 6418, write_bytes.store 112800 -> 79840, write_calls.store 61 -> 45
- codex move: cli_cpu_us 9494 -> 9580, cli_footprint_kib 3361 -> 3393, cli_maxrss_kib 5776 -> 5792, malloc_bytes 1966982 -> 1967714, mallocs 7868 -> 7871, read_bytes.store 1237 -> 1233, write_bytes.ipc 2329 -> 2330
- codex resume: cli_cpu_us 12764 -> 12948, cli_maxrss_kib 7392 -> 7376, frees 9654 -> 9645, malloc_bytes 4898075 -> 4896374, mallocs 11773 -> 11754, opens 304 -> 302, read_bytes.jobs 15362 -> 15361, read_bytes.state 270 -> 122, read_bytes.store 17459 -> 17444, read_calls.ipc 203 -> 211, read_calls.state 8 -> 4
- codex pull: cli_cpu_us 14182 -> 14148, cli_footprint_kib 3537 -> 3505, cli_maxrss_kib 7440 -> 7408, frees 25571 -> 25556, malloc_bytes 9734528 -> 9729938, mallocs 33168 -> 33150, opens 521 -> 519, read_bytes.ipc 11656 -> 11658, read_bytes.jobs 24073 -> 24071, read_bytes.state 322 -> 174, read_bytes.store 30619 -> 30582, read_calls.ipc 537 -> 533, read_calls.state 8 -> 4, write_bytes.ipc 4180 -> 4181, write_bytes.jobs 14274 -> 14261, write_bytes.store 103338 -> 128062, write_calls.store 61 -> 73
- cold start: cli_cpu_us 29585 -> 28266, cli_footprint_kib 3153 -> 3089, cli_maxrss_kib 5200 -> 5184, frees 2313 -> 2315, malloc_bytes 1050642 -> 1051581, mallocs 3260 -> 3263, write_bytes.ipc 306 -> 335
- restart: cli_cpu_us 42292 -> 43490, cli_footprint_kib 3121 -> 3137, cli_maxrss_kib 5216 -> 5248, frees 6078 -> 6274, malloc_bytes 2270200 -> 2479671, mallocs 9321 -> 9747, read_bytes.store 758116 -> 712796, read_calls.store 186 -> 175, write_bytes.ipc 1673 -> 2036, write_calls.ipc 25 -> 30

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13840 | 15600 | 14040 | 15792 | 1.004 | 1.002 to 1.008 |
| b | 130 | 15280 | 16048 | 15856 | 16112 | 1.027 | 1.026 to 1.029 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5249 | 6145 | 5489 | 6369 | 1.019 | 1.011 to 1.024 |
| b | 130 | 5825 | 6577 | 6401 | 6657 | 1.075 | 1.071 to 1.076 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10112 | 10528 | 10064 | 10480 | 0.998 | 0.995 to 1.002 |
| daemon b | 15376 | 16496 | 15360 | 16208 | 0.996 | 0.988 to 0.998 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3248 | 3697 | 3200 | 3617 | 0.995 | 0.975 to 1.005 |
| daemon b | 5792 | 6896 | 5793 | 6625 | 0.992 | 0.974 to 1.000 |
