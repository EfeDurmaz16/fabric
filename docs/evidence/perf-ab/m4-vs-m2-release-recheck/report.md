### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 30 | 49.1 | 49.0 | 1.005 | 0.994 to 1.036 | not shown, median above 1 |
| acp launch | 30 | 26.1 | 26.3 | 1.005 | 0.985 to 1.020 | not shown, median above 1 |
| acp approve | 30 | 6.9 | 6.9 | 1.015 | 0.968 to 1.031 | not shown, median above 1 |
| claude new | 30 | 46.1 | 46.9 | 1.012 | 0.995 to 1.030 | not shown, median above 1 |
| claude launch | 30 | 84.4 | 85.3 | 1.003 | 0.989 to 1.019 | not shown, median above 1 |
| claude approve | 30 | 7.2 | 7.2 | 0.999 | 0.944 to 1.020 | not shown |
| codex new | 30 | 48.4 | 48.5 | 1.002 | 0.970 to 1.035 | not shown, median above 1 |
| codex launch | 30 | 90.1 | 91.1 | 1.007 | 0.991 to 1.025 | not shown, median above 1 |
| codex approve | 30 | 7.2 | 7.1 | 0.989 | 0.964 to 1.028 | not shown |
| ledger append | 30 | 7.2 | 7.1 | 0.992 | 0.963 to 1.028 | not shown |
| cli version | 30 | 2.5 | 2.5 | 1.048 | 0.981 to 1.077 | not shown, median above 1 |
| cli ls | 30 | 2.8 | 2.7 | 0.999 | 0.976 to 1.034 | not shown |
| acp suspend | 30 | 254.6 | 255.1 | 1.001 | 0.989 to 1.007 | not shown, median above 1 |
| acp move | 30 | 99.8 | 99.5 | 0.997 | 0.985 to 1.005 | not shown |
| acp resume | 30 | 248.1 | 252.7 | 1.014 | 1.009 to 1.027 | slower |
| acp pull | 30 | 594.9 | 592.9 | 0.998 | 0.986 to 1.011 | not shown |
| claude suspend | 30 | 263.4 | 263.5 | 0.999 | 0.981 to 1.015 | not shown |
| claude move | 30 | 104.5 | 102.8 | 0.994 | 0.973 to 1.001 | not shown |
| claude resume | 30 | 358.4 | 350.6 | 0.990 | 0.975 to 1.006 | not shown |
| claude pull | 30 | 714.6 | 713.1 | 1.001 | 0.989 to 1.008 | not shown, median above 1 |
| codex suspend | 30 | 417.5 | 426.7 | 1.018 | 0.990 to 1.034 | not shown, median above 1 |
| codex move | 30 | 103.3 | 104.2 | 0.996 | 0.984 to 1.014 | not shown |
| codex resume | 30 | 460.9 | 465.1 | 1.005 | 0.987 to 1.016 | not shown, median above 1 |
| codex pull | 30 | 976.9 | 982.4 | 1.007 | 1.001 to 1.014 | slower |
| cold start | 30 | 50.9 | 50.3 | 1.000 | 0.981 to 1.014 | not shown |
| restart | 30 | 52.4 | 52.1 | 1.009 | 0.973 to 1.034 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 37861 -> 37861 | 6848 -> 7104 | 7.4 -> 12.1 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4447 -> 4453 | 6016 -> 6064 | 6.9 -> 7.0 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 785 -> 789 | 4112 -> 4208 | 6.1 -> 5.6 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1056 -> 1056 | 4160 -> 4288 | 5.8 -> 7.0 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 783 -> 784 | 3125 -> 3130 | 5664 -> 5728 | 7.4 -> 9.2 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 783 -> 784 | 4096 -> 4224 | 5.9 -> 6.0 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1056 -> 1056 | 4176 -> 4272 | 7.0 -> 6.3 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3471 -> 3475 | 5680 -> 5840 | 6.9 -> 7.2 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 783 -> 783 | 4080 -> 4224 | 6.3 -> 6.0 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 87 -> 0 | 786 -> 785 | 4096 -> 4224 | 4.9 -> 5.6 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 148 | 244 -> 248 | 3008 -> 3072 | 5.6 -> 5.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1036 -> 1036 | 4256 -> 4304 | 5.9 -> 5.9 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 24606 -> 24588 | 12237 -> 12248 | 4240 -> 4416 | 6.0 -> 7.0 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2467 -> 2453 | 4730 -> 4732 | 4512 -> 4656 | 7.6 -> 7.0 |
| acp resume | 14 -> 14 | 13 -> 13 | 55 -> 55 | 167 -> 167 | 16928 -> 16916 | 8115 -> 8116 | 5952 -> 6224 | 7.6 -> 8.4 |
| acp pull | 56 -> 56 | 41 -> 41 | 71 -> 71 | 1376 -> 1376 | 34998 -> 34812 | 21528 -> 21099 | 6240 -> 6272 | 8.0 -> 7.0 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 13578 -> 13619 | 14096 -> 14079 | 4224 -> 4384 | 5.5 -> 5.9 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2566 -> 2572 | 4936 -> 4936 | 4496 -> 4672 | 5.4 -> 6.0 |
| claude resume | 16 -> 16 | 13 -> 13 | 59 -> 59 | 148 -> 148 | 19982 -> 20005 | 7981 -> 7983 | 6000 -> 6192 | 9.4 -> 6.9 |
| claude pull | 61 -> 61 | 41 -> 41 | 77 -> 77 | 1338 -> 1338 | 40132 -> 40044 | 25158 -> 25149 | 6320 -> 6336 | 9.6 -> 8.5 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 14507 -> 14692 | 12806 -> 11936 | 4256 -> 4384 | 6.5 -> 5.9 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2650 -> 2654 | 4954 -> 4970 | 4496 -> 4704 | 7.1 -> 6.6 |
| codex resume | 17 -> 17 | 13 -> 13 | 63 -> 63 | 148 -> 148 | 23082 -> 23101 | 8134 -> 8135 | 6096 -> 6224 | 9.4 -> 8.7 |
| codex pull | 64 -> 64 | 40 -> 40 | 82 -> 82 | 1338 -> 1338 | 44168 -> 44073 | 22315 -> 22299 | 6192 -> 6384 | 8.2 -> 8.1 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4224 -> 4225 | 4128 -> 4208 | 22.6 -> 21.4 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 278283 -> 278283 | 8357 -> 8356 | 4240 -> 4320 | 22.0 -> 21.4 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 7443 -> 12107, cli_maxrss_kib 6848 -> 7104, malloc_bytes 13331123 -> 13331134
- acp launch: cli_cpu_us 6937 -> 6956, cli_maxrss_kib 6016 -> 6064, frees 3341 -> 3353, malloc_bytes 778697 -> 779501, mallocs 4447 -> 4453
- acp approve: cli_cpu_us 6059 -> 5630, cli_maxrss_kib 4112 -> 4208, frees 659 -> 664, malloc_bytes 254570 -> 254565, mallocs 785 -> 789
- claude new: cli_cpu_us 5798 -> 7026, cli_maxrss_kib 4160 -> 4288, frees 854 -> 853, malloc_bytes 293959 -> 293970
- claude launch: cli_cpu_us 7413 -> 9185, cli_maxrss_kib 5664 -> 5728, malloc_bytes 518580 -> 518936, mallocs 3125 -> 3130, read_bytes.jobs 609 -> 610, write_bytes.jobs 926 -> 927
- claude approve: cli_cpu_us 5929 -> 6046, cli_maxrss_kib 4096 -> 4224, frees 659 -> 660, malloc_bytes 253978 -> 254053, mallocs 783 -> 784
- codex new: cli_cpu_us 7028 -> 6310, cli_maxrss_kib 4176 -> 4272, frees 853 -> 854, malloc_bytes 293843 -> 293914
- codex launch: cli_cpu_us 6885 -> 7216, cli_maxrss_kib 5680 -> 5840, frees 3102 -> 3104, malloc_bytes 609279 -> 609728, mallocs 3471 -> 3475
- codex approve: cli_cpu_us 6313 -> 6016, cli_maxrss_kib 4080 -> 4224, malloc_bytes 253978 -> 253989
- ledger append: cli_cpu_us 4927 -> 5576, cli_maxrss_kib 4096 -> 4224, frees 661 -> 660, malloc_bytes 254577 -> 254309, mallocs 786 -> 785, opens 2 -> 1, read_bytes.state 87 -> 0, read_calls.state 2 -> 0
- cli version: cli_cpu_us 5609 -> 4980, cli_maxrss_kib 3008 -> 3072, frees 231 -> 235, malloc_bytes 105990 -> 106653, mallocs 244 -> 248, opens 0 -> 2, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli ls: cli_cpu_us 5856 -> 5857, cli_maxrss_kib 4256 -> 4304, malloc_bytes 290315 -> 290326
- acp suspend: cli_cpu_us 6036 -> 7031, cli_maxrss_kib 4240 -> 4416, frees 9212 -> 9213, malloc_bytes 3408489 -> 3425436, mallocs 12237 -> 12248, read_bytes.store 22338 -> 22320, write_bytes.jobs 3591 -> 3601, write_bytes.store 79457 -> 79451
- acp move: cli_cpu_us 7613 -> 7027, cli_maxrss_kib 4512 -> 4656, frees 3769 -> 3771, malloc_bytes 1613791 -> 1613873, mallocs 4730 -> 4732, read_bytes.store 2348 -> 2334
- acp resume: cli_cpu_us 7613 -> 8396, cli_maxrss_kib 5952 -> 6224, malloc_bytes 3564413 -> 3564477, mallocs 8115 -> 8116, read_bytes.jobs 3366 -> 3361, read_bytes.store 13440 -> 13433, write_bytes.jobs 3550 -> 3548, write_bytes.store 2477 -> 2479
- acp pull: cli_cpu_us 8048 -> 6951, cli_maxrss_kib 6240 -> 6272, frees 17215 -> 17175, malloc_bytes 7423056 -> 7318448, mallocs 21528 -> 21099, opens 350 -> 348, read_bytes.jobs 6231 -> 6229, read_bytes.state 322 -> 174, read_bytes.store 28445 -> 28409, read_calls.state 8 -> 4, write_bytes.jobs 8385 -> 8367, write_bytes.store 49088 -> 49091
- claude suspend: cli_cpu_us 5511 -> 5897, cli_maxrss_kib 4224 -> 4384, frees 11254 -> 11244, malloc_bytes 3417783 -> 3413790, mallocs 14096 -> 14079, read_bytes.store 10497 -> 10538, write_bytes.jobs 4199 -> 4201, write_bytes.store 46683 -> 46703
- claude move: cli_cpu_us 5440 -> 5966, cli_maxrss_kib 4496 -> 4672, malloc_bytes 1647823 -> 1647834, read_bytes.store 2484 -> 2490
- claude resume: cli_cpu_us 9422 -> 6911, cli_maxrss_kib 6000 -> 6192, malloc_bytes 3569506 -> 3569937, mallocs 7981 -> 7983, read_bytes.jobs 6160 -> 6158, read_bytes.store 13700 -> 13725, write_bytes.jobs 4131 -> 4153, write_bytes.store 2480 -> 2478
- claude pull: cli_cpu_us 9572 -> 8538, cli_maxrss_kib 6320 -> 6336, malloc_bytes 7710235 -> 7709707, mallocs 25158 -> 25149, opens 391 -> 389, read_bytes.jobs 10633 -> 10629, read_bytes.state 322 -> 174, read_bytes.store 29177 -> 29241, read_calls.ipc 268 -> 267, read_calls.state 8 -> 4, write_bytes.jobs 9785 -> 9768, write_bytes.store 49208 -> 49232
- codex suspend: cli_cpu_us 6518 -> 5901, cli_maxrss_kib 4256 -> 4384, frees 9726 -> 9685, malloc_bytes 3726502 -> 3498596, mallocs 12806 -> 11936, opens 138 -> 140, read_bytes.ipc 5180 -> 5179, read_bytes.state 87 -> 235, read_bytes.store 10839 -> 10876, read_calls.state 2 -> 6, write_bytes.jobs 4856 -> 4845, write_bytes.store 63551 -> 47085, write_calls.store 37 -> 29
- codex move: cli_cpu_us 7111 -> 6627, cli_maxrss_kib 4496 -> 4704, frees 3960 -> 3976, malloc_bytes 1648273 -> 1652124, mallocs 4954 -> 4970, read_bytes.store 2544 -> 2548
- codex resume: cli_cpu_us 9380 -> 8650, cli_maxrss_kib 6096 -> 6224, frees 6744 -> 6745, malloc_bytes 3677458 -> 3677523, mallocs 8134 -> 8135, read_bytes.jobs 8842 -> 8840, read_bytes.store 14118 -> 14139, write_bytes.jobs 4847 -> 4824
- codex pull: cli_cpu_us 8188 -> 8127, cli_maxrss_kib 6192 -> 6384, frees 18202 -> 18195, malloc_bytes 7849318 -> 7847731, mallocs 22315 -> 22299, opens 442 -> 440, read_bytes.jobs 13849 -> 13848, read_bytes.state 322 -> 174, read_bytes.store 29997 -> 30051, read_calls.ipc 277 -> 278, read_calls.state 8 -> 4, write_bytes.jobs 11106 -> 11092, write_bytes.store 49595 -> 66094, write_calls.store 33 -> 41
- cold start: cli_cpu_us 22607 -> 21364, cli_maxrss_kib 4128 -> 4208, frees 2882 -> 2883, malloc_bytes 1396832 -> 1396927, mallocs 4224 -> 4225
- restart: cli_cpu_us 22006 -> 21363, cli_maxrss_kib 4240 -> 4320, frees 5686 -> 5691, malloc_bytes 2120434 -> 2119920, mallocs 8357 -> 8356

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 12424 | 13696 | 12408 | 13296 | 0.988 | 0.982 to 0.997 |
| b | 26 | 13008 | 13360 | 12904 | 13168 | 0.987 | 0.986 to 0.994 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 5616 | 6704 | 5720 | 6256 | 1.010 | 0.964 to 1.030 |
| b | 26 | 6288 | 6608 | 6112 | 6336 | 0.972 | 0.960 to 0.977 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 9984 | 10528 | 10008 | 10384 | 1.000 | 0.998 to 1.002 |
| daemon b | 14040 | 15216 | 13984 | 15088 | 0.986 | 0.980 to 0.992 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3984 | 4528 | 4088 | 4464 | 1.020 | 1.014 to 1.024 |
| daemon b | 7792 | 8912 | 7848 | 8928 | 0.980 | 0.954 to 1.003 |
