### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 21 | 156.0 | 153.8 | 0.986 | 0.971 to 1.015 | not shown |
| acp launch | 21 | 60.6 | 63.1 | 0.999 | 0.989 to 1.043 | not shown |
| acp approve | 21 | 15.9 | 15.8 | 0.994 | 0.973 to 1.054 | not shown |
| claude new | 21 | 137.4 | 137.9 | 1.012 | 0.998 to 1.027 | not shown, median above 1 |
| claude launch | 21 | 110.1 | 111.4 | 1.031 | 0.978 to 1.069 | not shown, median above 1 |
| claude approve | 21 | 17.2 | 17.1 | 0.996 | 0.968 to 1.011 | not shown |
| codex new | 21 | 138.4 | 139.6 | 0.994 | 0.983 to 1.006 | not shown |
| codex launch | 21 | 129.3 | 124.4 | 0.958 | 0.938 to 1.047 | not shown |
| codex approve | 21 | 17.7 | 17.5 | 1.006 | 0.981 to 1.020 | not shown, median above 1 |
| ledger append | 21 | 17.4 | 17.2 | 0.984 | 0.947 to 1.014 | not shown |
| cli version | 21 | 3.4 | 3.6 | 1.030 | 0.902 to 1.087 | not shown, median above 1 |
| cli ls | 21 | 5.3 | 5.4 | 0.961 | 0.904 to 1.078 | not shown |
| acp suspend | 21 | 354.8 | 357.2 | 1.001 | 0.994 to 1.010 | not shown, median above 1 |
| acp move | 21 | 165.1 | 165.3 | 1.003 | 0.958 to 1.027 | not shown, median above 1 |
| acp resume | 21 | 649.9 | 611.7 | 0.992 | 0.975 to 1.019 | not shown |
| acp pull | 21 | 1152.9 | 1150.8 | 0.995 | 0.991 to 1.007 | not shown |
| claude suspend | 21 | 434.8 | 436.8 | 1.000 | 0.983 to 1.014 | not shown, median above 1 |
| claude move | 21 | 165.0 | 160.9 | 0.994 | 0.954 to 1.007 | not shown |
| claude resume | 21 | 794.8 | 801.2 | 0.994 | 0.990 to 0.998 | faster |
| claude pull | 21 | 1384.3 | 1425.0 | 1.001 | 0.991 to 1.006 | not shown, median above 1 |
| codex suspend | 21 | 653.3 | 653.6 | 1.005 | 0.992 to 1.009 | not shown, median above 1 |
| codex move | 21 | 164.8 | 164.0 | 0.990 | 0.970 to 1.040 | not shown |
| codex resume | 21 | 1032.5 | 1030.0 | 1.001 | 0.972 to 1.010 | not shown, median above 1 |
| codex pull | 21 | 1832.1 | 1848.8 | 1.002 | 0.997 to 1.012 | not shown, median above 1 |
| cold start | 21 | 59.5 | 59.3 | 0.991 | 0.937 to 1.048 | not shown |
| restart | 21 | 448.2 | 430.5 | 0.998 | 0.987 to 1.004 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38525 -> 38514 | 14800 -> 14576 | 18.0 -> 18.1 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4543 -> 4540 | 15424 -> 15536 | 12.5 -> 11.5 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1213 -> 1210 | 10944 -> 10864 | 7.6 -> 8.4 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1688 -> 1682 | 11056 -> 10992 | 8.5 -> 8.8 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1100 | 3631 -> 3630 | 14800 -> 14848 | 10.8 -> 11.3 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1209 -> 1207 | 10976 -> 10848 | 9.5 -> 11.2 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1686 -> 1681 | 11024 -> 10976 | 9.7 -> 8.9 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3960 -> 3960 | 14672 -> 14752 | 11.9 -> 14.2 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1210 -> 1212 | 10944 -> 10848 | 9.1 -> 8.5 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1210 -> 1209 | 10880 -> 10864 | 8.6 -> 9.8 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4880 | 5.8 -> 5.7 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1555 -> 1588 | 11040 -> 11072 | 9.4 -> 9.7 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40331 -> 40316 | 12864 -> 12834 | 11712 -> 11696 | 9.8 -> 9.6 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5273 -> 5261 | 8416 -> 8212 | 12288 -> 12304 | 10.4 -> 11.1 |
| acp resume | 33 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16378 -> 16347 | 11569 -> 11564 | 15632 -> 15648 | 11.2 -> 11.7 |
| acp pull | 112 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 34005 -> 33929 | 28702 -> 28897 | 15664 -> 15856 | 14.3 -> 13.3 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13537 -> 13513 | 16402 -> 16392 | 11648 -> 11664 | 10.1 -> 9.7 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5310 -> 5328 | 8275 -> 8054 | 12288 -> 12352 | 10.7 -> 10.1 |
| claude resume | 35 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21927 -> 21923 | 11463 -> 11432 | 15296 -> 15312 | 14.8 -> 14.9 |
| claude pull | 115 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 42368 -> 42329 | 33859 -> 33588 | 15392 -> 15472 | 16.8 -> 15.2 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14691 -> 14724 | 12599 -> 12702 | 11664 -> 11696 | 10.1 -> 9.4 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5361 -> 5391 | 8262 -> 8048 | 12352 -> 12336 | 11.2 -> 10.7 |
| codex resume | 36 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 26046 -> 26065 | 11597 -> 11565 | 15296 -> 15312 | 13.5 -> 14.9 |
| codex pull | 118 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47493 -> 47715 | 29385 -> 29085 | 15440 -> 15456 | 14.3 -> 14.3 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5945 -> 6822 | 10848 -> 10736 | 37.4 -> 38.6 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 468665 -> 485145 | 15827 -> 16489 | 11168 -> 11216 | 78.0 -> 77.5 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 17961 -> 18073, cli_maxrss_kib 14800 -> 14576, frees 27794 -> 27782, malloc_bytes 13431888 -> 13432275, mallocs 38525 -> 38514, opens 11 -> 10, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 12505 -> 11490, cli_maxrss_kib 15424 -> 15536, frees 3757 -> 3756, malloc_bytes 776550 -> 778200, mallocs 4543 -> 4540
- acp approve: cli_cpu_us 7565 -> 8387, cli_maxrss_kib 10944 -> 10864, frees 1053 -> 1050, malloc_bytes 326314 -> 326293, mallocs 1213 -> 1210
- claude new: cli_cpu_us 8468 -> 8832, cli_maxrss_kib 11056 -> 10992, frees 1423 -> 1417, malloc_bytes 393505 -> 393665, mallocs 1688 -> 1682, opens 11 -> 10, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 10752 -> 11347, cli_maxrss_kib 14800 -> 14848, frees 3224 -> 3225, malloc_bytes 614591 -> 615938, mallocs 3631 -> 3630
- claude approve: cli_cpu_us 9451 -> 11210, cli_maxrss_kib 10976 -> 10848, frees 1051 -> 1049, malloc_bytes 325418 -> 325638, mallocs 1209 -> 1207
- codex new: cli_cpu_us 9674 -> 8862, cli_maxrss_kib 11024 -> 10976, frees 1423 -> 1417, malloc_bytes 392613 -> 393144, mallocs 1686 -> 1681, opens 11 -> 10, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 11914 -> 14210, cli_maxrss_kib 14672 -> 14752, frees 3532 -> 3536, malloc_bytes 708227 -> 710168
- codex approve: cli_cpu_us 9123 -> 8486, cli_maxrss_kib 10944 -> 10848, frees 1051 -> 1053, malloc_bytes 325931 -> 326661, mallocs 1210 -> 1212
- ledger append: cli_cpu_us 8572 -> 9801, cli_maxrss_kib 10880 -> 10864, frees 1051 -> 1050, malloc_bytes 326123 -> 326406, mallocs 1210 -> 1209
- cli version: cli_cpu_us 5762 -> 5693, cli_maxrss_kib 4928 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9357 -> 9668, cli_maxrss_kib 11040 -> 11072, frees 1357 -> 1390, malloc_bytes 371006 -> 373465, mallocs 1555 -> 1588
- acp suspend: cli_cpu_us 9843 -> 9598, cli_maxrss_kib 11712 -> 11696, frees 9768 -> 9749, malloc_bytes 3530801 -> 3512772, mallocs 12864 -> 12834, opens 142 -> 139, read_bytes.store 37942 -> 37927, read_calls.ipc 187 -> 183, write_bytes.jobs 3580 -> 3584, write_bytes.store 95941 -> 95931
- acp move: cli_cpu_us 10374 -> 11076, cli_maxrss_kib 12288 -> 12304, frees 6917 -> 6754, malloc_bytes 2024198 -> 2022421, mallocs 8416 -> 8212, opens 73 -> 33, read_bytes.jobs 4099 -> 4091, read_bytes.store 1174 -> 1170, read_calls.ipc 124 -> 76, write_bytes.ipc 2509 -> 2510
- acp resume: cli_cpu_us 11236 -> 11672, cli_maxrss_kib 15632 -> 15648, frees 9330 -> 9331, malloc_bytes 4400785 -> 4403141, mallocs 11569 -> 11564, read_bytes.ipc 4938 -> 4936, read_bytes.store 12697 -> 12666, read_calls.ipc 174 -> 171, write_bytes.jobs 3550 -> 3544, write_bytes.store 4533 -> 4528
- acp pull: cli_cpu_us 14291 -> 13256, cli_maxrss_kib 15664 -> 15856, frees 23220 -> 23036, malloc_bytes 8831109 -> 8952822, mallocs 28702 -> 28897, opens 416 -> 371, read_bytes.ipc 11151 -> 11150, read_bytes.jobs 8546 -> 8545, read_bytes.store 25285 -> 25210, read_calls.ipc 501 -> 444, write_bytes.ipc 4615 -> 4616, write_bytes.jobs 8397 -> 8403, write_bytes.store 60216 -> 60202
- claude suspend: cli_cpu_us 10132 -> 9664, cli_maxrss_kib 11648 -> 11664, frees 13082 -> 13072, malloc_bytes 3597199 -> 3597968, mallocs 16402 -> 16392, opens 145 -> 142, read_bytes.ipc 5293 -> 5292, read_bytes.store 9774 -> 9750, read_calls.ipc 185 -> 181, write_bytes.jobs 4767 -> 4771, write_bytes.store 54961 -> 79683, write_calls.store 33 -> 45
- claude move: cli_cpu_us 10747 -> 10130, cli_maxrss_kib 12288 -> 12352, frees 6814 -> 6636, malloc_bytes 2013761 -> 2009726, mallocs 8275 -> 8054, opens 73 -> 33, read_bytes.jobs 4062 -> 4078, read_bytes.store 1248 -> 1250, read_calls.ipc 124 -> 76, write_bytes.ipc 2361 -> 2362
- claude resume: cli_cpu_us 14827 -> 14921, cli_maxrss_kib 15296 -> 15312, frees 9468 -> 9443, malloc_bytes 4410755 -> 4410803, mallocs 11463 -> 11432, opens 249 -> 247, read_bytes.jobs 8580 -> 8583, read_bytes.store 13225 -> 13218, read_calls.ipc 180 -> 176, write_bytes.jobs 4704 -> 4703, write_bytes.store 4536 -> 4530
- claude pull: cli_cpu_us 16804 -> 15169, cli_maxrss_kib 15392 -> 15472, frees 27360 -> 27144, malloc_bytes 9149645 -> 9147952, mallocs 33859 -> 33588, opens 459 -> 415, read_bytes.ipc 10652 -> 10651, read_bytes.jobs 15735 -> 15723, read_bytes.store 26311 -> 26284, read_calls.ipc 509 -> 451, write_bytes.ipc 4122 -> 4123, write_bytes.jobs 10895 -> 10890, write_bytes.store 60358 -> 60341
- codex suspend: cli_cpu_us 10126 -> 9418, cli_maxrss_kib 11664 -> 11696, frees 10300 -> 10329, malloc_bytes 3662597 -> 3761331, mallocs 12599 -> 12702, opens 152 -> 149, read_bytes.ipc 5306 -> 5307, read_bytes.store 10110 -> 10143, read_calls.ipc 190 -> 186, write_bytes.jobs 5634 -> 5642, write_bytes.store 63575 -> 63599
- codex move: cli_cpu_us 11179 -> 10650, cli_maxrss_kib 12352 -> 12336, frees 6808 -> 6638, malloc_bytes 2011743 -> 2008849, mallocs 8262 -> 8048, opens 73 -> 33, read_bytes.jobs 4082 -> 4106, read_bytes.store 1279 -> 1285, read_calls.ipc 124 -> 76, write_bytes.ipc 2361 -> 2362
- codex resume: cli_cpu_us 13499 -> 14913, cli_maxrss_kib 15296 -> 15312, frees 9626 -> 9599, malloc_bytes 4516391 -> 4516407, mallocs 11597 -> 11565, opens 293 -> 291, read_bytes.ipc 4564 -> 4567, read_bytes.jobs 12195 -> 12189, read_bytes.store 13729 -> 13754, read_calls.ipc 183 -> 179, write_bytes.jobs 5631 -> 5627, write_bytes.store 4533 -> 4530
- codex pull: cli_cpu_us 14271 -> 14340, cli_maxrss_kib 15440 -> 15456, frees 24129 -> 23893, malloc_bytes 9299480 -> 9294816, mallocs 29385 -> 29085, opens 510 -> 466, read_bytes.jobs 20110 -> 20121, read_bytes.state 174 -> 322, read_bytes.store 27209 -> 27272, read_calls.ipc 517 -> 460, read_calls.state 4 -> 8, write_bytes.ipc 4056 -> 4057, write_bytes.jobs 12676 -> 12690, write_bytes.store 85446 -> 85458
- cold start: cli_cpu_us 37362 -> 38629, cli_maxrss_kib 10848 -> 10736, cli_processes 4 -> 5, frees 4447 -> 5192, malloc_bytes 1670832 -> 1961777, mallocs 5945 -> 6822, write_bytes.ipc 487 -> 668, write_calls.ipc 7 -> 9
- restart: cli_cpu_us 78018 -> 77533, cli_maxrss_kib 11168 -> 11216, frees 11737 -> 12371, malloc_bytes 4012170 -> 4095366, mallocs 15827 -> 16489, opens 232 -> 245, read_bytes.store 444996 -> 461476, read_calls.store 110 -> 114, write_bytes.ipc 3101 -> 3695, write_calls.ipc 44 -> 53

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5264 | 5664 | 5280 | 5776 | 1.006 | 1.003 to 1.022 |
| daemon a peak | 5264 | 5664 | 5280 | 5776 | 1.006 | 1.003 to 1.022 |
| daemon b | 12784 | 13888 | 13264 | 14272 | 1.026 | 1.017 to 1.031 |
| daemon b peak | 13040 | 14160 | 13504 | 14592 | 1.026 | 1.019 to 1.030 |
