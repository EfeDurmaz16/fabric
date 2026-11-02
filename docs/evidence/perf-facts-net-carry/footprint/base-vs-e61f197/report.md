### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 21 | 153.0 | 153.9 | 0.995 | 0.990 to 1.019 | not shown |
| acp launch | 21 | 61.1 | 61.8 | 0.988 | 0.959 to 1.005 | not shown |
| acp approve | 21 | 15.7 | 15.7 | 0.994 | 0.975 to 1.013 | not shown |
| claude new | 21 | 138.0 | 136.2 | 0.999 | 0.985 to 1.019 | not shown |
| claude launch | 21 | 106.9 | 112.3 | 1.030 | 0.990 to 1.102 | not shown, median above 1 |
| claude approve | 21 | 17.5 | 17.2 | 1.005 | 0.961 to 1.047 | not shown, median above 1 |
| codex new | 21 | 137.3 | 136.0 | 0.991 | 0.980 to 1.005 | not shown |
| codex launch | 21 | 126.5 | 121.1 | 0.988 | 0.913 to 1.039 | not shown |
| codex approve | 21 | 17.2 | 17.6 | 0.993 | 0.971 to 1.033 | not shown |
| ledger append | 21 | 17.0 | 16.9 | 0.974 | 0.950 to 1.001 | not shown |
| cli version | 21 | 3.3 | 3.3 | 0.965 | 0.892 to 1.104 | not shown |
| cli ls | 21 | 5.4 | 5.3 | 0.997 | 0.975 to 1.021 | not shown |
| acp suspend | 21 | 350.6 | 354.4 | 1.011 | 0.987 to 1.021 | not shown, median above 1 |
| acp move | 21 | 156.7 | 156.8 | 0.996 | 0.983 to 1.040 | not shown |
| acp resume | 21 | 619.9 | 612.0 | 0.997 | 0.987 to 1.011 | not shown |
| acp pull | 21 | 1142.0 | 1109.8 | 1.005 | 0.988 to 1.012 | not shown, median above 1 |
| claude suspend | 21 | 431.2 | 431.0 | 1.000 | 0.986 to 1.013 | not shown |
| claude move | 21 | 157.4 | 156.9 | 1.013 | 0.978 to 1.030 | not shown, median above 1 |
| claude resume | 21 | 803.9 | 787.1 | 1.004 | 0.991 to 1.007 | not shown, median above 1 |
| claude pull | 21 | 1412.2 | 1390.0 | 0.998 | 0.994 to 1.017 | not shown |
| codex suspend | 21 | 653.8 | 649.3 | 1.002 | 0.977 to 1.023 | not shown, median above 1 |
| codex move | 21 | 156.2 | 158.7 | 1.011 | 0.985 to 1.031 | not shown, median above 1 |
| codex resume | 21 | 1022.6 | 1018.9 | 1.005 | 0.998 to 1.009 | not shown, median above 1 |
| codex pull | 21 | 1829.9 | 1843.0 | 0.997 | 0.991 to 1.011 | not shown |
| cold start | 21 | 61.9 | 61.8 | 0.996 | 0.971 to 1.036 | not shown |
| restart | 21 | 430.5 | 426.4 | 1.001 | 0.990 to 1.008 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38519 -> 38516 | 14848 -> 14752 | 20.1 -> 20.2 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 681 -> 681 | 4929 -> 4919 | 15392 -> 15536 | 12.5 -> 11.8 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1210 -> 1213 | 10976 -> 10784 | 9.3 -> 8.6 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1689 -> 1683 | 11056 -> 10944 | 9.6 -> 10.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1098 -> 1102 | 3625 -> 3620 | 14832 -> 14800 | 12.5 -> 12.1 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1217 | 10928 -> 10816 | 8.9 -> 8.9 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1687 -> 1681 | 11056 -> 10944 | 10.2 -> 9.5 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 683 -> 683 | 3984 -> 3961 | 14672 -> 14720 | 12.9 -> 12.3 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1216 -> 1215 | 10976 -> 10848 | 9.3 -> 9.3 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1214 -> 1222 | 10960 -> 10848 | 8.9 -> 9.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4912 | 5.6 -> 5.3 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1552 -> 1551 | 11008 -> 10976 | 8.0 -> 8.6 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40357 -> 40348 | 12872 -> 12857 | 11680 -> 11664 | 9.5 -> 10.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5279 -> 5284 | 8427 -> 8195 | 12304 -> 12304 | 10.1 -> 10.1 |
| acp resume | 33 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16404 -> 16408 | 11555 -> 11546 | 15664 -> 15632 | 13.0 -> 12.4 |
| acp pull | 112 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 34054 -> 34036 | 28684 -> 29361 | 15776 -> 15776 | 13.5 -> 13.4 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13548 -> 13541 | 16406 -> 16402 | 11648 -> 11664 | 9.9 -> 9.6 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5328 -> 5338 | 8279 -> 8060 | 12240 -> 12288 | 9.6 -> 10.3 |
| claude resume | 35 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21935 -> 21930 | 11480 -> 11439 | 15328 -> 15392 | 14.7 -> 14.4 |
| claude pull | 115 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 42216 -> 42219 | 33867 -> 33595 | 15456 -> 15488 | 15.4 -> 16.2 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14682 -> 14691 | 12593 -> 12599 | 11696 -> 11696 | 9.4 -> 10.2 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5375 -> 5390 | 8284 -> 8051 | 12256 -> 12272 | 10.3 -> 10.2 |
| codex resume | 36 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 26024 -> 26194 | 11593 -> 11579 | 15312 -> 15360 | 13.5 -> 14.1 |
| codex pull | 118 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47450 -> 47531 | 29367 -> 29083 | 15488 -> 15504 | 15.0 -> 14.9 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5964 -> 5949 | 10896 -> 10736 | 31.8 -> 33.7 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 476905 -> 472785 | 16555 -> 16481 | 11184 -> 11104 | 73.3 -> 75.3 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 20059 -> 20151, cli_maxrss_kib 14848 -> 14752, frees 27792 -> 27785, malloc_bytes 13431673 -> 13430704, mallocs 38519 -> 38516, opens 11 -> 10, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 12467 -> 11832, cli_maxrss_kib 15392 -> 15536, frees 3770 -> 3769, malloc_bytes 877716 -> 876330, mallocs 4929 -> 4919
- acp approve: cli_cpu_us 9327 -> 8571, cli_maxrss_kib 10976 -> 10784, frees 1050 -> 1053, malloc_bytes 325947 -> 326117, mallocs 1210 -> 1213
- claude new: cli_cpu_us 9620 -> 10032, cli_maxrss_kib 11056 -> 10944, frees 1423 -> 1416, malloc_bytes 393326 -> 392465, mallocs 1689 -> 1683, opens 11 -> 10, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 12477 -> 12067, cli_maxrss_kib 14832 -> 14800, frees 3218 -> 3215, malloc_bytes 614201 -> 613545, mallocs 3625 -> 3620, read_bytes.jobs 924 -> 928, write_bytes.jobs 927 -> 931
- claude approve: cli_cpu_us 8926 -> 8863, cli_maxrss_kib 10928 -> 10816, frees 1050 -> 1059, malloc_bytes 325354 -> 325733, mallocs 1208 -> 1217
- codex new: cli_cpu_us 10201 -> 9518, cli_maxrss_kib 11056 -> 10944, frees 1423 -> 1418, malloc_bytes 393125 -> 391829, mallocs 1687 -> 1681, opens 11 -> 10, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 12878 -> 12253, cli_maxrss_kib 14672 -> 14720, frees 3556 -> 3535, malloc_bytes 709565 -> 707324, mallocs 3984 -> 3961
- codex approve: cli_cpu_us 9348 -> 9332, cli_maxrss_kib 10976 -> 10848, frees 1058 -> 1057, malloc_bytes 325866 -> 325605, mallocs 1216 -> 1215
- ledger append: cli_cpu_us 8863 -> 9481, cli_maxrss_kib 10960 -> 10848, frees 1055 -> 1062, malloc_bytes 325930 -> 326693, mallocs 1214 -> 1222
- cli version: cli_cpu_us 5642 -> 5342, cli_maxrss_kib 4928 -> 4912, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 7970 -> 8629, cli_maxrss_kib 11008 -> 10976, malloc_bytes 371262 -> 370553, mallocs 1552 -> 1551
- acp suspend: cli_cpu_us 9474 -> 10028, cli_maxrss_kib 11680 -> 11664, frees 9771 -> 9765, malloc_bytes 3531008 -> 3528534, mallocs 12872 -> 12857, opens 142 -> 139, read_bytes.store 37968 -> 37959, read_calls.ipc 187 -> 183, write_bytes.jobs 3592 -> 3590
- acp move: cli_cpu_us 10147 -> 10143, frees 6928 -> 6742, malloc_bytes 2025532 -> 1998879, mallocs 8427 -> 8195, opens 73 -> 33, read_bytes.jobs 4107 -> 4111, read_bytes.store 1172 -> 1173, read_calls.ipc 124 -> 76
- acp resume: cli_cpu_us 13009 -> 12440, cli_maxrss_kib 15664 -> 15632, frees 9318 -> 9307, malloc_bytes 4399320 -> 4397666, mallocs 11555 -> 11546, read_bytes.ipc 4935 -> 4938, read_bytes.jobs 3559 -> 3561, read_bytes.store 12723 -> 12725, read_calls.ipc 175 -> 170, write_bytes.jobs 3551 -> 3555, write_bytes.store 4535 -> 4531
- acp pull: cli_cpu_us 13455 -> 13401, frees 23209 -> 23075, malloc_bytes 8830116 -> 9032952, mallocs 28684 -> 29361, opens 416 -> 371, read_bytes.jobs 8538 -> 8543, read_bytes.store 25342 -> 25319, read_calls.ipc 501 -> 445, write_bytes.jobs 8399 -> 8394, write_bytes.store 60215 -> 60203
- claude suspend: cli_cpu_us 9926 -> 9550, cli_maxrss_kib 11648 -> 11664, frees 13083 -> 13085, malloc_bytes 3596639 -> 3595884, mallocs 16406 -> 16402, opens 145 -> 142, read_bytes.store 9785 -> 9778, read_calls.ipc 185 -> 181, write_bytes.jobs 4763 -> 4762, write_bytes.store 54957 -> 54953
- claude move: cli_cpu_us 9576 -> 10320, cli_maxrss_kib 12240 -> 12288, frees 6817 -> 6641, malloc_bytes 2014178 -> 1987309, mallocs 8279 -> 8060, opens 73 -> 33, read_bytes.jobs 4078 -> 4086, read_bytes.store 1250 -> 1252, read_calls.ipc 124 -> 76
- claude resume: cli_cpu_us 14711 -> 14376, cli_maxrss_kib 15328 -> 15392, frees 9484 -> 9450, malloc_bytes 4412337 -> 4406992, mallocs 11480 -> 11439, opens 249 -> 247, read_bytes.jobs 8589 -> 8584, read_calls.ipc 180 -> 176, write_bytes.jobs 4716 -> 4709, write_bytes.store 4531 -> 4533
- claude pull: cli_cpu_us 15397 -> 16170, cli_maxrss_kib 15456 -> 15488, frees 27368 -> 27164, malloc_bytes 9151506 -> 9118326, mallocs 33867 -> 33595, opens 459 -> 415, read_bytes.ipc 10650 -> 10652, read_bytes.jobs 15731 -> 15723, read_bytes.store 26311 -> 26322, read_calls.ipc 509 -> 451, write_bytes.jobs 10884 -> 10898, write_bytes.store 60349 -> 60360
- codex suspend: cli_cpu_us 9418 -> 10248, frees 10292 -> 10297, malloc_bytes 3662039 -> 3661394, mallocs 12593 -> 12599, opens 152 -> 149, read_bytes.store 10101 -> 10110, read_calls.ipc 190 -> 186, write_bytes.jobs 5631 -> 5635, write_bytes.store 71809 -> 63581, write_calls.store 41 -> 37
- codex move: cli_cpu_us 10347 -> 10250, cli_maxrss_kib 12256 -> 12272, frees 6832 -> 6640, malloc_bytes 2012867 -> 1986304, mallocs 8284 -> 8051, opens 73 -> 33, read_bytes.jobs 4098 -> 4106, read_bytes.store 1277 -> 1284, read_calls.ipc 124 -> 76
- codex resume: cli_cpu_us 13500 -> 14124, cli_maxrss_kib 15312 -> 15360, frees 9624 -> 9612, malloc_bytes 4515915 -> 4514034, mallocs 11593 -> 11579, read_bytes.jobs 12198 -> 12195, read_bytes.state 122 -> 270, read_bytes.store 13704 -> 13729, read_calls.ipc 183 -> 180, read_calls.state 4 -> 8, write_bytes.jobs 5608 -> 5626, write_bytes.store 4533 -> 4532
- codex pull: cli_cpu_us 14970 -> 14946, cli_maxrss_kib 15488 -> 15504, frees 24117 -> 23891, malloc_bytes 9296777 -> 9265189, mallocs 29367 -> 29083, opens 510 -> 464, read_bytes.jobs 20116 -> 20121, read_bytes.store 27160 -> 27236, read_calls.ipc 517 -> 459, write_bytes.ipc 4056 -> 4055, write_bytes.jobs 12685 -> 12691, write_bytes.store 85430 -> 93685, write_calls.store 51 -> 55
- cold start: cli_cpu_us 31794 -> 33718, cli_maxrss_kib 10896 -> 10736, frees 4465 -> 4450, malloc_bytes 1672541 -> 1670703, mallocs 5964 -> 5949
- restart: cli_cpu_us 73311 -> 75293, cli_maxrss_kib 11184 -> 11104, frees 12399 -> 12361, malloc_bytes 4099801 -> 4093261, mallocs 16555 -> 16481, read_bytes.store 453236 -> 449116, read_calls.store 112 -> 111, write_bytes.ipc 3695 -> 3458, write_calls.ipc 53 -> 49

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5360 | 5840 | 5328 | 5808 | 1.006 | 0.985 to 1.014 |
| daemon a peak | 5360 | 5840 | 5328 | 5808 | 1.006 | 0.985 to 1.014 |
| daemon b | 13008 | 14064 | 12960 | 14352 | 1.012 | 1.005 to 1.019 |
| daemon b peak | 13312 | 14368 | 13184 | 14688 | 1.008 | 1.005 to 1.015 |
