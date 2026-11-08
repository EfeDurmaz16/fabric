### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 54.3 | 53.4 | 1.009 | 0.936 to 1.036 | not shown, median above 1 |
| acp launch | 20 | 26.7 | 27.1 | 1.016 | 0.992 to 1.041 | not shown, median above 1 |
| acp approve | 20 | 7.1 | 7.3 | 1.042 | 0.965 to 1.124 | not shown, median above 1 |
| claude new | 20 | 49.0 | 50.1 | 1.020 | 0.991 to 1.068 | not shown, median above 1 |
| claude launch | 20 | 90.5 | 90.7 | 0.994 | 0.942 to 1.024 | not shown |
| claude approve | 20 | 7.9 | 8.0 | 1.021 | 0.978 to 1.068 | not shown, median above 1 |
| codex new | 20 | 52.6 | 50.8 | 0.993 | 0.938 to 1.045 | not shown |
| codex launch | 20 | 95.1 | 95.3 | 0.997 | 0.929 to 1.022 | not shown |
| codex approve | 20 | 7.2 | 7.6 | 0.977 | 0.957 to 1.101 | not shown |
| ledger append | 20 | 7.2 | 7.4 | 1.012 | 0.970 to 1.046 | not shown, median above 1 |
| cli version | 20 | 2.6 | 2.7 | 1.071 | 0.978 to 1.138 | not shown, median above 1 |
| cli ls | 20 | 2.9 | 2.9 | 1.018 | 0.990 to 1.058 | not shown, median above 1 |
| acp suspend | 20 | 272.4 | 272.1 | 0.984 | 0.951 to 1.024 | not shown |
| acp move | 20 | 104.7 | 106.0 | 1.007 | 0.982 to 1.021 | not shown, median above 1 |
| acp resume | 20 | 262.2 | 263.0 | 1.005 | 0.982 to 1.053 | not shown, median above 1 |
| acp pull | 20 | 628.1 | 622.7 | 0.998 | 0.974 to 1.015 | not shown |
| claude suspend | 20 | 278.4 | 287.3 | 1.051 | 1.032 to 1.077 | slower |
| claude move | 20 | 106.8 | 109.4 | 1.001 | 0.991 to 1.048 | not shown, median above 1 |
| claude resume | 20 | 374.8 | 373.2 | 0.992 | 0.967 to 1.027 | not shown |
| claude pull | 20 | 764.7 | 781.8 | 1.022 | 0.984 to 1.054 | not shown, median above 1 |
| codex suspend | 20 | 480.8 | 466.9 | 1.000 | 0.965 to 1.026 | not shown |
| codex move | 20 | 106.9 | 108.0 | 0.993 | 0.984 to 1.022 | not shown |
| codex resume | 20 | 506.3 | 509.3 | 1.020 | 0.991 to 1.032 | not shown, median above 1 |
| codex pull | 20 | 1081.0 | 1086.8 | 0.996 | 0.985 to 1.002 | not shown |
| cold start | 20 | 50.4 | 55.8 | 1.053 | 0.999 to 1.196 | not shown, median above 1 |
| restart | 20 | 55.4 | 58.4 | 1.043 | 0.988 to 1.073 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 37868 -> 38211 | 6848 -> 7296 | 7.5 -> 7.5 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4055 -> 4548 | 6096 -> 6384 | 7.8 -> 8.0 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 784 -> 1131 | 4240 -> 4528 | 8.1 -> 6.4 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1057 -> 1401 | 4272 -> 4624 | 5.7 -> 5.7 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 781 -> 786 | 3130 -> 3580 | 5760 -> 6032 | 6.4 -> 6.3 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 784 -> 1129 | 4192 -> 4528 | 5.1 -> 7.3 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1055 -> 1403 | 4256 -> 4608 | 5.9 -> 5.1 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3478 -> 3968 | 5792 -> 6064 | 6.5 -> 7.3 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 783 -> 1129 | 4208 -> 4528 | 5.3 -> 5.2 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 785 -> 1130 | 4208 -> 4560 | 5.3 -> 5.1 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 244 -> 553 | 3056 -> 3312 | 4.8 -> 4.6 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1035 -> 1381 | 4272 -> 4560 | 5.4 -> 5.2 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 24632 -> 24622 | 12235 -> 12603 | 4368 -> 4752 | 5.8 -> 5.8 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2463 -> 2455 | 4732 -> 5079 | 4656 -> 5104 | 5.9 -> 5.8 |
| acp resume | 14 -> 14 | 13 -> 13 | 55 -> 55 | 167 -> 167 | 16931 -> 16922 | 7724 -> 8573 | 6112 -> 6512 | 7.4 -> 9.4 |
| acp pull | 56 -> 56 | 39 -> 41 | 71 -> 71 | 1376 -> 1376 | 34996 -> 34792 | 21487 -> 22863 | 6176 -> 6624 | 7.2 -> 8.4 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 13646 -> 13633 | 14089 -> 14423 | 4352 -> 4736 | 5.4 -> 5.5 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2574 -> 2570 | 4942 -> 5287 | 4688 -> 5072 | 6.2 -> 5.5 |
| claude resume | 16 -> 16 | 13 -> 13 | 59 -> 59 | 148 -> 148 | 20000 -> 20000 | 7970 -> 8429 | 6112 -> 6496 | 9.8 -> 7.9 |
| claude pull | 61 -> 61 | 41 -> 41 | 77 -> 77 | 1338 -> 1338 | 40214 -> 40036 | 25160 -> 25610 | 6336 -> 6624 | 8.8 -> 8.0 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 14547 -> 14570 | 11904 -> 12253 | 4368 -> 4688 | 5.5 -> 6.4 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2658 -> 2660 | 4954 -> 5300 | 4672 -> 5008 | 6.5 -> 6.8 |
| codex resume | 17 -> 17 | 13 -> 13 | 63 -> 63 | 148 -> 148 | 23101 -> 23097 | 8138 -> 8592 | 6208 -> 6496 | 7.4 -> 10.2 |
| codex pull | 64 -> 64 | 40 -> 40 | 82 -> 82 | 1338 -> 1338 | 44045 -> 44202 | 22301 -> 22776 | 6336 -> 6608 | 8.7 -> 8.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4224 -> 5853 | 4176 -> 4496 | 22.4 -> 22.8 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 270043 -> 282403 | 8342 -> 10014 | 4336 -> 4640 | 24.5 -> 21.3 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 7484 -> 7481, cli_maxrss_kib 6848 -> 7296, frees 27210 -> 27534, malloc_bytes 13331582 -> 13498198, mallocs 37868 -> 38211
- acp launch: cli_cpu_us 7787 -> 8000, cli_maxrss_kib 6096 -> 6384, frees 3320 -> 3778, malloc_bytes 677035 -> 875586, mallocs 4055 -> 4548
- acp approve: cli_cpu_us 8107 -> 6427, cli_maxrss_kib 4240 -> 4528, frees 659 -> 980, malloc_bytes 254243 -> 421115, mallocs 784 -> 1131
- claude new: cli_cpu_us 5683 -> 5671, cli_maxrss_kib 4272 -> 4624, frees 853 -> 1174, malloc_bytes 294246 -> 460442, mallocs 1057 -> 1401
- claude launch: cli_cpu_us 6406 -> 6301, cli_maxrss_kib 5760 -> 6032, frees 2776 -> 3205, malloc_bytes 518945 -> 708952, mallocs 3130 -> 3580, read_bytes.jobs 607 -> 612, write_bytes.jobs 925 -> 930
- claude approve: cli_cpu_us 5132 -> 7309, cli_maxrss_kib 4192 -> 4528, frees 659 -> 980, malloc_bytes 254323 -> 420523, mallocs 784 -> 1129
- codex new: cli_cpu_us 5874 -> 5125, cli_maxrss_kib 4256 -> 4608, frees 853 -> 1174, malloc_bytes 293850 -> 460726, mallocs 1055 -> 1403
- codex launch: cli_cpu_us 6513 -> 7257, cli_maxrss_kib 5792 -> 6064, frees 3101 -> 3567, malloc_bytes 609784 -> 807804, mallocs 3478 -> 3968
- codex approve: cli_cpu_us 5263 -> 5233, cli_maxrss_kib 4208 -> 4528, frees 659 -> 980, malloc_bytes 253987 -> 420523, mallocs 783 -> 1129
- ledger append: cli_cpu_us 5271 -> 5102, cli_maxrss_kib 4208 -> 4560, frees 660 -> 980, malloc_bytes 254307 -> 420779, mallocs 785 -> 1130
- cli version: cli_cpu_us 4796 -> 4587, cli_maxrss_kib 3056 -> 3312, frees 231 -> 516, malloc_bytes 105990 -> 264640, mallocs 244 -> 553
- cli ls: cli_cpu_us 5411 -> 5152, cli_maxrss_kib 4272 -> 4560, frees 880 -> 1201, malloc_bytes 290259 -> 456795, mallocs 1035 -> 1381
- acp suspend: cli_cpu_us 5849 -> 5831, cli_maxrss_kib 4368 -> 4752, frees 9212 -> 9535, malloc_bytes 3408162 -> 3593668, mallocs 12235 -> 12603, read_bytes.store 22364 -> 22354, write_bytes.jobs 3584 -> 3583, write_bytes.store 79457 -> 79445
- acp move: cli_cpu_us 5938 -> 5797, cli_maxrss_kib 4656 -> 5104, frees 3770 -> 4091, malloc_bytes 1614214 -> 1780825, mallocs 4732 -> 5079, read_bytes.store 2344 -> 2336
- acp resume: cli_cpu_us 7386 -> 9422, cli_maxrss_kib 6112 -> 6512, frees 6433 -> 6888, malloc_bytes 3462928 -> 3755144, mallocs 7724 -> 8573, read_bytes.jobs 3361 -> 3362, read_bytes.store 13448 -> 13438, write_bytes.store 2479 -> 2480
- acp pull: cli_cpu_us 7163 -> 8402, cli_maxrss_kib 6176 -> 6624, frees 17197 -> 17692, fullfsync 39 -> 41, malloc_bytes 7420362 -> 7843169, mallocs 21487 -> 22863, read_bytes.jobs 6224 -> 6230, read_bytes.state 322 -> 174, read_bytes.store 28450 -> 28388, read_calls.ipc 257 -> 256, read_calls.state 8 -> 4, syncs.store 16 -> 18, write_bytes.jobs 8386 -> 8403, write_bytes.store 49097 -> 49078
- claude suspend: cli_cpu_us 5392 -> 5501, cli_maxrss_kib 4352 -> 4736, frees 11241 -> 11564, malloc_bytes 3414438 -> 3580148, mallocs 14089 -> 14423, read_bytes.store 10565 -> 10552, write_bytes.jobs 4194 -> 4190, write_bytes.store 46699 -> 46693
- claude move: cli_cpu_us 6159 -> 5541, cli_maxrss_kib 4688 -> 5072, frees 3951 -> 4265, malloc_bytes 1648059 -> 1815041, mallocs 4942 -> 5287, read_bytes.store 2492 -> 2488
- claude resume: cli_cpu_us 9795 -> 7940, cli_maxrss_kib 6112 -> 6496, frees 6568 -> 6998, malloc_bytes 3568702 -> 3759463, mallocs 7970 -> 8429, read_bytes.jobs 6151 -> 6158, read_bytes.store 13727 -> 13720, write_bytes.jobs 4136 -> 4146
- claude pull: cli_cpu_us 8843 -> 8034, cli_maxrss_kib 6336 -> 6624, frees 20222 -> 20652, malloc_bytes 7711225 -> 7899760, mallocs 25160 -> 25610, opens 391 -> 389, read_bytes.jobs 10633 -> 10624, read_bytes.state 322 -> 174, read_bytes.store 29259 -> 29238, read_calls.state 8 -> 4, write_bytes.jobs 9793 -> 9751, write_bytes.store 49238 -> 49229
- codex suspend: cli_cpu_us 5494 -> 6392, cli_maxrss_kib 4368 -> 4688, frees 9672 -> 9997, malloc_bytes 3511643 -> 3661516, mallocs 11904 -> 12253, read_bytes.store 10879 -> 10902, write_bytes.jobs 4843 -> 4855
- codex move: cli_cpu_us 6471 -> 6760, cli_maxrss_kib 4672 -> 5008, frees 3960 -> 4281, malloc_bytes 1648283 -> 1814939, mallocs 4954 -> 5300, read_bytes.store 2552 -> 2554
- codex resume: cli_cpu_us 7366 -> 10150, cli_maxrss_kib 6208 -> 6496, frees 6747 -> 7173, malloc_bytes 3677670 -> 3868177, mallocs 8138 -> 8592, read_bytes.jobs 8840 -> 8831, read_bytes.store 14139 -> 14144, read_calls.ipc 77 -> 76, write_bytes.jobs 4854 -> 4828, write_bytes.store 2479 -> 2477
- codex pull: cli_cpu_us 8726 -> 7952, cli_maxrss_kib 6336 -> 6608, frees 18197 -> 18617, malloc_bytes 7847419 -> 8041137, mallocs 22301 -> 22776, opens 440 -> 442, read_bytes.jobs 13849 -> 13851, read_bytes.state 174 -> 322, read_bytes.store 30022 -> 30029, read_calls.ipc 278 -> 277, read_calls.state 4 -> 8, write_bytes.jobs 11101 -> 11095, write_bytes.store 49598 -> 74316, write_calls.store 33 -> 45
- cold start: cli_cpu_us 22384 -> 22777, cli_maxrss_kib 4176 -> 4496, frees 2881 -> 4381, malloc_bytes 1396803 -> 2209489, mallocs 4224 -> 5853
- restart: cli_cpu_us 24534 -> 21346, cli_maxrss_kib 4336 -> 4640, frees 5682 -> 7219, malloc_bytes 2118331 -> 2941406, mallocs 8342 -> 10014, read_bytes.store 255476 -> 267836, read_calls.store 64 -> 67

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 12136 | 13184 | 12976 | 13792 | 1.047 | 1.045 to 1.056 |
| b | 26 | 12688 | 12800 | 13072 | 13856 | 1.073 | 1.059 to 1.077 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 5488 | 6256 | 5592 | 6320 | 1.019 | 1.010 to 1.021 |
| b | 26 | 5832 | 6096 | 6096 | 6656 | 1.058 | 1.047 to 1.082 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 9928 | 10144 | 10496 | 10816 | 1.059 | 1.054 to 1.066 |
| daemon b | 13688 | 14320 | 14088 | 14912 | 1.030 | 1.025 to 1.039 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3992 | 4208 | 4152 | 4480 | 1.048 | 1.034 to 1.063 |
| daemon b | 7432 | 8240 | 7560 | 8448 | 1.013 | 1.008 to 1.026 |
