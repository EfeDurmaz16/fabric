### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 21 | 169.6 | 167.2 | 1.031 | 0.998 to 1.058 | not shown, median above 1 |
| acp launch | 21 | 72.7 | 74.1 | 0.997 | 0.980 to 1.032 | not shown |
| acp approve | 21 | 16.9 | 17.6 | 1.012 | 0.999 to 1.073 | not shown, median above 1 |
| claude new | 21 | 152.3 | 143.3 | 0.978 | 0.933 to 1.035 | not shown |
| claude launch | 21 | 113.4 | 116.8 | 0.991 | 0.972 to 1.046 | not shown |
| claude approve | 21 | 17.1 | 17.0 | 1.014 | 0.986 to 1.061 | not shown, median above 1 |
| codex new | 21 | 143.4 | 157.4 | 1.048 | 0.973 to 1.072 | not shown, median above 1 |
| codex launch | 21 | 128.2 | 130.4 | 1.037 | 0.959 to 1.075 | not shown, median above 1 |
| codex approve | 21 | 17.6 | 17.9 | 1.006 | 0.986 to 1.080 | not shown, median above 1 |
| ledger append | 21 | 17.3 | 17.1 | 0.982 | 0.909 to 1.015 | not shown |
| cli version | 21 | 3.4 | 3.3 | 0.984 | 0.929 to 1.074 | not shown |
| cli ls | 21 | 5.6 | 5.6 | 1.020 | 0.978 to 1.039 | not shown, median above 1 |
| acp suspend | 21 | 454.6 | 456.2 | 1.004 | 0.944 to 1.030 | not shown, median above 1 |
| acp move | 21 | 185.7 | 186.2 | 1.009 | 0.951 to 1.145 | not shown, median above 1 |
| acp resume | 21 | 781.2 | 776.6 | 0.965 | 0.919 to 0.986 | faster |
| acp pull | 21 | 1371.7 | 1450.5 | 1.028 | 0.978 to 1.076 | not shown, median above 1 |
| claude suspend | 21 | 538.5 | 539.5 | 0.979 | 0.927 to 1.010 | not shown |
| claude move | 21 | 178.3 | 177.0 | 0.992 | 0.937 to 1.044 | not shown |
| claude resume | 21 | 959.9 | 996.1 | 1.003 | 0.966 to 1.026 | not shown, median above 1 |
| claude pull | 21 | 1681.6 | 1655.8 | 0.998 | 0.947 to 1.035 | not shown |
| codex suspend | 21 | 844.3 | 796.3 | 0.997 | 0.932 to 1.036 | not shown |
| codex move | 21 | 179.1 | 179.0 | 0.995 | 0.945 to 1.045 | not shown |
| codex resume | 21 | 1234.3 | 1249.4 | 1.000 | 0.990 to 1.031 | not shown |
| codex pull | 21 | 2298.9 | 2182.6 | 0.992 | 0.942 to 1.037 | not shown |
| cold start | 21 | 61.7 | 61.7 | 1.030 | 0.960 to 1.066 | not shown, median above 1 |
| restart | 21 | 557.7 | 504.3 | 0.989 | 0.955 to 1.076 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38529 -> 38514 | 14976 -> 14688 | 20.9 -> 20.3 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4545 -> 4919 | 15424 -> 15536 | 13.1 -> 13.6 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1212 -> 1209 | 10880 -> 10816 | 9.4 -> 8.2 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1690 -> 1689 | 11056 -> 11008 | 11.0 -> 10.3 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1103 -> 1097 | 3635 -> 3635 | 14784 -> 14800 | 13.2 -> 13.6 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1237 -> 1210 | 10928 -> 10912 | 10.0 -> 9.7 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1707 -> 1680 | 11056 -> 10960 | 9.7 -> 9.3 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3966 -> 3964 | 14720 -> 14752 | 14.8 -> 13.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1209 | 10912 -> 10864 | 9.2 -> 9.3 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 148 | 1212 -> 1220 | 10928 -> 10864 | 9.9 -> 10.7 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4880 | 6.8 -> 8.2 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1557 -> 1551 | 10960 -> 11056 | 9.7 -> 10.5 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40320 -> 40376 | 12858 -> 12837 | 11664 -> 11680 | 11.0 -> 10.2 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5265 -> 5269 | 8421 -> 8211 | 12256 -> 12320 | 10.4 -> 11.1 |
| acp resume | 33 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16369 -> 16397 | 11543 -> 11529 | 15632 -> 15680 | 13.1 -> 14.4 |
| acp pull | 112 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 33957 -> 34172 | 28696 -> 28898 | 15792 -> 15856 | 14.6 -> 14.4 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13526 -> 13523 | 16851 -> 16390 | 11632 -> 11680 | 10.2 -> 10.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5305 -> 5304 | 8277 -> 8073 | 12272 -> 12336 | 10.7 -> 11.0 |
| claude resume | 35 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21911 -> 21912 | 11451 -> 11434 | 15344 -> 15328 | 13.8 -> 14.8 |
| claude pull | 115 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 42336 -> 42179 | 33879 -> 33610 | 15408 -> 15440 | 15.0 -> 15.1 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14712 -> 14674 | 12724 -> 12722 | 11664 -> 11744 | 9.8 -> 10.2 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5373 -> 5364 | 8273 -> 8054 | 12272 -> 12320 | 11.2 -> 10.3 |
| codex resume | 36 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 26216 -> 26030 | 11617 -> 11579 | 15312 -> 15360 | 13.9 -> 14.0 |
| codex pull | 118 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47675 -> 47605 | 29363 -> 29094 | 15456 -> 15440 | 17.6 -> 16.7 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5941 -> 5947 | 10832 -> 10816 | 37.6 -> 40.3 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 509865 -> 497505 | 16840 -> 16476 | 11200 -> 11232 | 90.7 -> 76.2 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 20937 -> 20323, cli_maxrss_kib 14976 -> 14688, frees 27798 -> 27781, malloc_bytes 13432144 -> 13432275, mallocs 38529 -> 38514, opens 11 -> 10, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 13089 -> 13587, cli_maxrss_kib 15424 -> 15536, frees 3758 -> 3769, malloc_bytes 776730 -> 878751, mallocs 4545 -> 4919
- acp approve: cli_cpu_us 9434 -> 8195, cli_maxrss_kib 10880 -> 10816, frees 1053 -> 1049, malloc_bytes 326059 -> 326406, mallocs 1212 -> 1209
- claude new: cli_cpu_us 10966 -> 10264, cli_maxrss_kib 11056 -> 11008, frees 1425 -> 1424, malloc_bytes 393665 -> 393892, mallocs 1690 -> 1689, opens 11 -> 10, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 13189 -> 13632, cli_maxrss_kib 14784 -> 14800, frees 3227 -> 3231, malloc_bytes 615077 -> 616298, read_bytes.jobs 929 -> 923, write_bytes.jobs 932 -> 926
- claude approve: cli_cpu_us 9973 -> 9713, cli_maxrss_kib 10928 -> 10912, frees 1078 -> 1052, malloc_bytes 327531 -> 325958, mallocs 1237 -> 1210
- codex new: cli_cpu_us 9715 -> 9322, cli_maxrss_kib 11056 -> 10960, frees 1444 -> 1417, malloc_bytes 393957 -> 393048, mallocs 1707 -> 1680, opens 11 -> 10, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 14850 -> 13005, cli_maxrss_kib 14720 -> 14752, frees 3539 -> 3538, malloc_bytes 708632 -> 710453, mallocs 3966 -> 3964, read_calls.ipc 13 -> 12
- codex approve: cli_cpu_us 9243 -> 9259, cli_maxrss_kib 10912 -> 10864, malloc_bytes 325355 -> 326102, mallocs 1208 -> 1209
- ledger append: cli_cpu_us 9927 -> 10707, cli_maxrss_kib 10928 -> 10864, frees 1053 -> 1059, malloc_bytes 325803 -> 327642, mallocs 1212 -> 1220, opens 3 -> 5, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli version: cli_cpu_us 6751 -> 8165, cli_maxrss_kib 4928 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9724 -> 10520, cli_maxrss_kib 10960 -> 11056, frees 1359 -> 1353, malloc_bytes 371134 -> 371097, mallocs 1557 -> 1551
- acp suspend: cli_cpu_us 11023 -> 10236, cli_maxrss_kib 11664 -> 11680, frees 9762 -> 9750, malloc_bytes 3529362 -> 3513531, mallocs 12858 -> 12837, opens 142 -> 139, read_bytes.ipc 5325 -> 5324, read_bytes.store 37931 -> 37987, read_calls.ipc 187 -> 183, write_bytes.jobs 3583 -> 3590, write_bytes.store 95931 -> 95977
- acp move: cli_cpu_us 10376 -> 11106, cli_maxrss_kib 12256 -> 12320, frees 6920 -> 6759, malloc_bytes 2024696 -> 2025311, mallocs 8421 -> 8211, opens 73 -> 33, read_bytes.jobs 4095 -> 4099, read_calls.ipc 124 -> 76, write_bytes.ipc 2510 -> 2509
- acp resume: cli_cpu_us 13127 -> 14354, cli_maxrss_kib 15632 -> 15680, frees 9307 -> 9296, malloc_bytes 4398405 -> 4400390, mallocs 11543 -> 11529, read_bytes.jobs 3559 -> 3554, read_bytes.store 12688 -> 12721, read_calls.ipc 173 -> 168, write_bytes.jobs 3552 -> 3550, write_bytes.store 4530 -> 4536
- acp pull: cli_cpu_us 14571 -> 14372, cli_maxrss_kib 15792 -> 15856, frees 23213 -> 23038, malloc_bytes 8830986 -> 8952585, mallocs 28696 -> 28898, opens 416 -> 369, read_bytes.ipc 11156 -> 11157, read_bytes.jobs 8552 -> 8539, read_bytes.state 174 -> 322, read_bytes.store 25231 -> 25311, read_calls.ipc 498 -> 444, read_calls.state 4 -> 8, write_bytes.ipc 4616 -> 4615, write_bytes.jobs 8405 -> 8399, write_bytes.store 60203 -> 60202
- claude suspend: cli_cpu_us 10221 -> 9995, cli_maxrss_kib 11632 -> 11680, frees 13100 -> 13070, malloc_bytes 3704646 -> 3597862, mallocs 16851 -> 16390, opens 145 -> 142, read_bytes.store 9763 -> 9760, read_calls.ipc 185 -> 181, write_bytes.jobs 4749 -> 4751, write_bytes.store 54951 -> 54959
- claude move: cli_cpu_us 10743 -> 11036, cli_maxrss_kib 12272 -> 12336, frees 6811 -> 6648, malloc_bytes 2013052 -> 2011379, mallocs 8277 -> 8073, opens 73 -> 33, read_bytes.store 1247 -> 1246, read_calls.ipc 124 -> 76, write_bytes.ipc 2362 -> 2361
- claude resume: cli_cpu_us 13850 -> 14828, cli_maxrss_kib 15344 -> 15328, frees 9456 -> 9443, malloc_bytes 4410048 -> 4411409, mallocs 11451 -> 11434, opens 249 -> 247, read_bytes.ipc 4417 -> 4416, read_bytes.jobs 8579 -> 8585, read_bytes.store 13210 -> 13205, read_calls.ipc 180 -> 176, write_bytes.jobs 4670 -> 4707, write_bytes.store 4533 -> 4532
- claude pull: cli_cpu_us 14985 -> 15053, cli_maxrss_kib 15408 -> 15440, frees 27370 -> 27161, malloc_bytes 9151475 -> 9148255, mallocs 33879 -> 33610, opens 459 -> 413, read_bytes.jobs 15725 -> 15734, read_bytes.state 322 -> 174, read_bytes.store 26289 -> 26271, read_calls.ipc 508 -> 451, read_calls.state 8 -> 4, write_bytes.ipc 4123 -> 4122, write_bytes.jobs 10887 -> 10903, write_bytes.store 93306 -> 85070, write_calls.store 55 -> 51
- codex suspend: cli_cpu_us 9817 -> 10192, cli_maxrss_kib 11664 -> 11744, frees 10336 -> 10344, malloc_bytes 3762002 -> 3763844, mallocs 12724 -> 12722, opens 152 -> 149, read_bytes.store 10131 -> 10093, read_calls.ipc 190 -> 186, write_bytes.jobs 5647 -> 5636, write_bytes.store 80067 -> 88293, write_calls.store 45 -> 49
- codex move: cli_cpu_us 11216 -> 10309, cli_maxrss_kib 12272 -> 12320, frees 6820 -> 6643, malloc_bytes 2012419 -> 2009623, mallocs 8273 -> 8054, opens 73 -> 33, read_bytes.store 1283 -> 1274, read_calls.ipc 124 -> 76, write_bytes.ipc 2362 -> 2361
- codex resume: cli_cpu_us 13895 -> 14008, cli_maxrss_kib 15312 -> 15360, frees 9647 -> 9613, malloc_bytes 4518318 -> 4517479, mallocs 11617 -> 11579, opens 295 -> 291, read_bytes.jobs 12202 -> 12197, read_bytes.state 270 -> 122, read_bytes.store 13744 -> 13711, read_calls.ipc 182 -> 179, read_calls.state 8 -> 4, write_bytes.store 4532 -> 4535
- codex pull: cli_cpu_us 17593 -> 16694, cli_maxrss_kib 15456 -> 15440, frees 24113 -> 23911, malloc_bytes 9296841 -> 9294658, mallocs 29363 -> 29094, opens 510 -> 466, read_bytes.ipc 10821 -> 10822, read_bytes.jobs 20094 -> 20093, read_bytes.store 27259 -> 27190, read_calls.ipc 517 -> 459, write_bytes.ipc 4057 -> 4056, write_bytes.jobs 12654 -> 12661, write_bytes.store 85459 -> 75599, write_calls.store 51 -> 45
- cold start: cli_cpu_us 37637 -> 40326, cli_maxrss_kib 10832 -> 10816, frees 4442 -> 4449, malloc_bytes 1670832 -> 1671215, mallocs 5941 -> 5947
- restart: cli_cpu_us 90712 -> 76201, cli_maxrss_kib 11200 -> 11232, cli_processes 10 -> 9, frees 12964 -> 12355, malloc_bytes 4324935 -> 4094739, mallocs 16840 -> 16476, read_bytes.store 486196 -> 473836, read_calls.store 120 -> 117

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20752 | 21296 | 20784 | 21136 | 0.999 | 0.995 to 1.000 |
| daemon b | 27584 | 29760 | 27824 | 30448 | 1.022 | 1.002 to 1.023 |
