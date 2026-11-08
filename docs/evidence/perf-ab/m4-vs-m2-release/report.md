### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 58.8 | 59.1 | 0.970 | 0.941 to 1.022 | not shown |
| acp launch | 20 | 29.5 | 28.6 | 1.011 | 0.967 to 1.031 | not shown, median above 1 |
| acp approve | 20 | 7.3 | 7.1 | 0.979 | 0.948 to 1.014 | not shown |
| claude new | 20 | 53.5 | 56.7 | 1.053 | 1.011 to 1.095 | slower |
| claude launch | 20 | 86.1 | 85.5 | 1.005 | 0.949 to 1.026 | not shown, median above 1 |
| claude approve | 20 | 8.0 | 8.0 | 1.016 | 0.927 to 1.063 | not shown, median above 1 |
| codex new | 20 | 55.2 | 54.4 | 0.991 | 0.934 to 1.038 | not shown |
| codex launch | 20 | 93.1 | 92.9 | 1.002 | 0.983 to 1.018 | not shown, median above 1 |
| codex approve | 20 | 7.5 | 7.4 | 0.980 | 0.949 to 1.033 | not shown |
| ledger append | 20 | 7.4 | 7.4 | 0.997 | 0.969 to 1.030 | not shown |
| cli version | 20 | 2.7 | 2.6 | 0.986 | 0.894 to 1.039 | not shown |
| cli ls | 20 | 3.0 | 2.9 | 0.969 | 0.910 to 1.035 | not shown |
| acp suspend | 20 | 304.7 | 308.2 | 1.020 | 0.993 to 1.063 | not shown, median above 1 |
| acp move | 20 | 100.8 | 100.5 | 1.004 | 0.983 to 1.019 | not shown, median above 1 |
| acp resume | 20 | 273.8 | 266.3 | 0.985 | 0.952 to 1.009 | not shown |
| acp pull | 20 | 688.2 | 659.2 | 0.972 | 0.958 to 1.006 | not shown |
| claude suspend | 20 | 308.7 | 309.7 | 0.995 | 0.961 to 1.067 | not shown |
| claude move | 20 | 103.4 | 107.8 | 1.028 | 0.997 to 1.053 | not shown, median above 1 |
| claude resume | 20 | 366.3 | 364.9 | 1.000 | 0.961 to 1.018 | not shown, median above 1 |
| claude pull | 20 | 791.1 | 814.4 | 1.003 | 0.972 to 1.050 | not shown, median above 1 |
| codex suspend | 20 | 514.1 | 503.1 | 1.014 | 0.967 to 1.037 | not shown, median above 1 |
| codex move | 20 | 106.9 | 105.1 | 0.993 | 0.969 to 1.007 | not shown |
| codex resume | 20 | 534.1 | 521.5 | 0.987 | 0.938 to 1.013 | not shown |
| codex pull | 20 | 1144.2 | 1114.2 | 0.993 | 0.969 to 1.030 | not shown |
| cold start | 20 | 52.1 | 52.0 | 1.011 | 0.991 to 1.025 | not shown, median above 1 |
| restart | 20 | 55.4 | 56.3 | 1.026 | 0.967 to 1.047 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 37861 -> 37861 | 6880 -> 7136 | 7.6 -> 9.1 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4057 -> 4064 | 5968 -> 6112 | 8.0 -> 7.5 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 784 -> 784 | 4112 -> 4240 | 5.7 -> 5.9 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1058 -> 1057 | 4160 -> 4288 | 5.8 -> 5.8 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 787 -> 786 | 3132 -> 3126 | 5616 -> 5776 | 7.4 -> 7.9 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 783 -> 783 | 4112 -> 4240 | 5.6 -> 5.7 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1056 -> 1056 | 4160 -> 4288 | 6.1 -> 6.1 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3478 -> 3478 | 5648 -> 5808 | 7.5 -> 7.6 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 784 -> 784 | 4112 -> 4256 | 6.7 -> 5.7 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 148 -> 0 | 789 -> 786 | 4096 -> 4256 | 5.4 -> 5.8 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 148 | 244 -> 248 | 3008 -> 3056 | 5.4 -> 5.5 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1131 -> 1131 | 4240 -> 4368 | 5.9 -> 8.8 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 40982 -> 40975 | 12287 -> 12293 | 4240 -> 4400 | 6.5 -> 6.2 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2455 -> 2447 | 4732 -> 4731 | 4496 -> 4656 | 5.9 -> 6.1 |
| acp resume | 14 -> 14 | 13 -> 13 | 61 -> 61 | 167 -> 167 | 21704 -> 21698 | 8203 -> 8112 | 6032 -> 6224 | 7.6 -> 7.7 |
| acp pull | 56 -> 56 | 41 -> 41 | 77 -> 77 | 1376 -> 1376 | 39707 -> 39555 | 21592 -> 21506 | 6208 -> 6288 | 7.9 -> 8.0 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 14163 -> 14185 | 15831 -> 15829 | 4240 -> 4400 | 6.5 -> 7.4 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2564 -> 2570 | 4938 -> 4936 | 4512 -> 4672 | 6.5 -> 6.0 |
| claude resume | 16 -> 16 | 13 -> 13 | 65 -> 65 | 148 -> 148 | 27016 -> 27035 | 8170 -> 8157 | 6016 -> 6176 | 8.5 -> 8.2 |
| claude pull | 61 -> 61 | 41 -> 41 | 83 -> 83 | 1338 -> 1338 | 47561 -> 47625 | 27043 -> 27519 | 6272 -> 6336 | 9.2 -> 8.4 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 15300 -> 15282 | 12033 -> 12033 | 4224 -> 4384 | 5.7 -> 8.4 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2640 -> 2644 | 4960 -> 4962 | 4544 -> 4688 | 6.2 -> 6.1 |
| codex resume | 17 -> 17 | 13 -> 13 | 69 -> 69 | 148 -> 148 | 31044 -> 31015 | 8278 -> 8286 | 6080 -> 6272 | 8.9 -> 9.5 |
| codex pull | 64 -> 64 | 40 -> 40 | 88 -> 88 | 1338 -> 1338 | 52725 -> 52829 | 22756 -> 22537 | 6224 -> 6384 | 10.4 -> 8.9 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4685 -> 4688 | 4128 -> 4224 | 29.5 -> 28.9 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 481999 -> 486119 | 9272 -> 9319 | 4288 -> 4384 | 23.8 -> 24.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 7584 -> 9076, cli_maxrss_kib 6880 -> 7136, malloc_bytes 13331126 -> 13331137
- acp launch: cli_cpu_us 7994 -> 7453, cli_maxrss_kib 5968 -> 6112, malloc_bytes 677513 -> 678004, mallocs 4057 -> 4064
- acp approve: cli_cpu_us 5672 -> 5939, cli_maxrss_kib 4112 -> 4240, malloc_bytes 254235 -> 254246
- claude new: cli_cpu_us 5782 -> 5843, cli_maxrss_kib 4160 -> 4288, frees 854 -> 853, malloc_bytes 294366 -> 294249, mallocs 1058 -> 1057
- claude launch: cli_cpu_us 7412 -> 7889, cli_maxrss_kib 5616 -> 5776, malloc_bytes 519255 -> 518634, mallocs 3132 -> 3126, read_bytes.jobs 613 -> 612, write_bytes.jobs 931 -> 930
- claude approve: cli_cpu_us 5594 -> 5739, cli_maxrss_kib 4112 -> 4240, malloc_bytes 253979 -> 253990
- codex new: cli_cpu_us 6144 -> 6113, cli_maxrss_kib 4160 -> 4288, malloc_bytes 293846 -> 293857
- codex launch: cli_cpu_us 7451 -> 7644, cli_maxrss_kib 5648 -> 5808, frees 3102 -> 3103, malloc_bytes 609744 -> 609787, read_calls.ipc 8 -> 7
- codex approve: cli_cpu_us 6708 -> 5680, cli_maxrss_kib 4112 -> 4256, frees 660 -> 659, malloc_bytes 254043 -> 254118
- ledger append: cli_cpu_us 5377 -> 5843, cli_maxrss_kib 4096 -> 4256, frees 663 -> 659, malloc_bytes 254959 -> 254970, mallocs 789 -> 786, opens 3 -> 1, read_bytes.state 148 -> 0, read_calls.state 4 -> 0
- cli version: cli_cpu_us 5425 -> 5550, cli_maxrss_kib 3008 -> 3056, frees 231 -> 235, malloc_bytes 105990 -> 106653, mallocs 244 -> 248, opens 0 -> 2, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli ls: cli_cpu_us 5892 -> 8804, cli_maxrss_kib 4240 -> 4368, malloc_bytes 299899 -> 299910
- acp suspend: cli_cpu_us 6545 -> 6177, cli_maxrss_kib 4240 -> 4400, frees 9257 -> 9259, malloc_bytes 3427080 -> 3443810, mallocs 12287 -> 12293, read_bytes.store 38714 -> 38707, write_bytes.jobs 3582 -> 3580, write_bytes.store 95921 -> 95917
- acp move: cli_cpu_us 5923 -> 6073, cli_maxrss_kib 4496 -> 4656, frees 3769 -> 3770, malloc_bytes 1614120 -> 1613836, mallocs 4732 -> 4731, read_bytes.store 2336 -> 2328
- acp resume: cli_cpu_us 7594 -> 7688, cli_maxrss_kib 6032 -> 6224, frees 6524 -> 6523, malloc_bytes 3568549 -> 3564265, mallocs 8203 -> 8112, read_bytes.jobs 3362 -> 3364, read_bytes.store 18220 -> 18212, read_calls.ipc 65 -> 67, write_bytes.jobs 3554 -> 3553
- acp pull: cli_cpu_us 7886 -> 7961, cli_maxrss_kib 6208 -> 6288, frees 17263 -> 17272, malloc_bytes 7436994 -> 7421442, mallocs 21592 -> 21506, opens 366 -> 368, read_bytes.jobs 6231 -> 6234, read_bytes.state 322 -> 174, read_bytes.store 33154 -> 33147, read_calls.state 8 -> 4, write_bytes.jobs 8379 -> 8389, write_bytes.store 57311 -> 57315
- claude suspend: cli_cpu_us 6486 -> 7416, cli_maxrss_kib 4240 -> 4400, frees 12581 -> 12579, malloc_bytes 3510948 -> 3510876, mallocs 15831 -> 15829, read_bytes.store 10521 -> 10543, write_bytes.jobs 4757 -> 4751, write_bytes.store 79647 -> 54951, write_calls.store 45 -> 33
- claude move: cli_cpu_us 6463 -> 6033, cli_maxrss_kib 4512 -> 4672, frees 3946 -> 3945, malloc_bytes 1648203 -> 1647836, mallocs 4938 -> 4936, read_bytes.store 2482 -> 2488
- claude resume: cli_cpu_us 8538 -> 8157, cli_maxrss_kib 6016 -> 6176, frees 6729 -> 6716, malloc_bytes 3584090 -> 3583153, mallocs 8170 -> 8157, read_bytes.jobs 8394 -> 8387, read_bytes.store 18500 -> 18526, read_calls.ipc 72 -> 73, write_bytes.jobs 4701 -> 4697, write_bytes.store 2479 -> 2478
- claude pull: cli_cpu_us 9250 -> 8351, cli_maxrss_kib 6272 -> 6336, frees 21664 -> 21717, malloc_bytes 7817981 -> 7889057, mallocs 27043 -> 27519, opens 407 -> 409, read_bytes.ipc 7226 -> 7227, read_bytes.jobs 13420 -> 13414, read_bytes.store 33967 -> 34037, write_bytes.jobs 10898 -> 10883, write_bytes.store 57455 -> 90432, write_calls.store 37 -> 53
- codex suspend: cli_cpu_us 5690 -> 8450, cli_maxrss_kib 4224 -> 4384, malloc_bytes 3593564 -> 3577425, read_bytes.store 10840 -> 10822, write_bytes.jobs 5638 -> 5644, write_bytes.store 63555 -> 55293, write_calls.store 37 -> 33
- codex move: cli_cpu_us 6209 -> 6108, cli_maxrss_kib 4544 -> 4688, frees 3966 -> 3962, malloc_bytes 1648904 -> 1649030, mallocs 4960 -> 4962, read_bytes.store 2534 -> 2538
- codex resume: cli_cpu_us 8895 -> 9501, cli_maxrss_kib 6080 -> 6272, frees 6859 -> 6867, malloc_bytes 3689330 -> 3690090, mallocs 8278 -> 8286, read_bytes.jobs 12001 -> 12002, read_bytes.store 18921 -> 18891, write_bytes.jobs 5625 -> 5621, write_bytes.store 2479 -> 2477
- codex pull: cli_cpu_us 10428 -> 8855, cli_maxrss_kib 6224 -> 6384, frees 18404 -> 18397, malloc_bytes 7952478 -> 7923709, mallocs 22756 -> 22537, opens 460 -> 458, read_bytes.ipc 7383 -> 7381, read_bytes.jobs 17799 -> 17805, read_bytes.state 174 -> 322, read_bytes.store 34752 -> 34702, read_calls.ipc 277 -> 276, read_calls.state 4 -> 8, write_bytes.jobs 12666 -> 12672, write_bytes.store 82549 -> 90783, write_calls.store 49 -> 53
- cold start: cli_cpu_us 29544 -> 28882, cli_maxrss_kib 4128 -> 4224, frees 3241 -> 3243, malloc_bytes 1620566 -> 1620792, mallocs 4685 -> 4688
- restart: cli_cpu_us 23833 -> 24651, cli_maxrss_kib 4288 -> 4384, frees 6213 -> 6247, malloc_bytes 2220235 -> 2226625, mallocs 9272 -> 9319, read_bytes.store 461476 -> 465596, read_calls.store 114 -> 115

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 12456 | 13280 | 12288 | 13776 | 0.990 | 0.987 to 0.994 |
| b | 78 | 12992 | 13312 | 12800 | 13072 | 0.988 | 0.981 to 0.993 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5496 | 6176 | 5464 | 6272 | 1.000 | 0.994 to 1.006 |
| b | 78 | 6304 | 6848 | 6240 | 6672 | 0.988 | 0.985 to 0.988 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 9912 | 10176 | 9944 | 10224 | 1.005 | 1.002 to 1.008 |
| daemon b | 13592 | 14224 | 13520 | 14336 | 0.992 | 0.988 to 0.997 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3904 | 4176 | 4016 | 4304 | 1.031 | 1.026 to 1.040 |
| daemon b | 7344 | 8112 | 7168 | 7776 | 0.988 | 0.964 to 0.999 |
