### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 63.1 | 62.6 | 0.993 | 0.931 to 1.017 | not shown |
| acp launch | 20 | 30.9 | 30.5 | 0.984 | 0.953 to 1.022 | not shown |
| acp approve | 20 | 7.9 | 7.9 | 1.027 | 0.974 to 1.062 | not shown, median above 1 |
| claude new | 20 | 60.1 | 57.9 | 0.984 | 0.955 to 1.042 | not shown |
| claude launch | 20 | 89.2 | 87.6 | 0.991 | 0.955 to 1.027 | not shown |
| claude approve | 20 | 8.0 | 8.3 | 1.044 | 0.985 to 1.084 | not shown, median above 1 |
| codex new | 20 | 61.0 | 62.1 | 0.993 | 0.955 to 1.060 | not shown |
| codex launch | 20 | 95.4 | 95.6 | 0.987 | 0.975 to 1.004 | not shown |
| codex approve | 20 | 8.0 | 8.5 | 1.032 | 0.991 to 1.069 | not shown, median above 1 |
| ledger append | 20 | 8.0 | 7.9 | 1.004 | 0.971 to 1.069 | not shown, median above 1 |
| cli version | 20 | 3.0 | 2.8 | 1.041 | 0.920 to 1.123 | not shown, median above 1 |
| cli ls | 20 | 3.4 | 3.3 | 1.004 | 0.940 to 1.037 | not shown, median above 1 |
| acp suspend | 20 | 334.0 | 330.9 | 0.987 | 0.965 to 1.065 | not shown |
| acp move | 20 | 102.1 | 102.5 | 1.009 | 0.981 to 1.021 | not shown, median above 1 |
| acp resume | 20 | 300.1 | 301.3 | 1.001 | 0.933 to 1.073 | not shown, median above 1 |
| acp pull | 20 | 705.1 | 732.5 | 1.032 | 1.000 to 1.081 | slower |
| claude suspend | 20 | 331.9 | 343.0 | 1.011 | 0.992 to 1.030 | not shown, median above 1 |
| claude move | 20 | 108.3 | 106.6 | 0.992 | 0.960 to 1.003 | not shown |
| claude resume | 20 | 381.9 | 389.7 | 1.030 | 0.980 to 1.048 | not shown, median above 1 |
| claude pull | 20 | 830.4 | 831.8 | 0.993 | 0.953 to 1.043 | not shown |
| codex suspend | 20 | 588.4 | 581.4 | 1.018 | 0.922 to 1.046 | not shown, median above 1 |
| codex move | 20 | 105.7 | 105.0 | 1.010 | 0.976 to 1.029 | not shown, median above 1 |
| codex resume | 20 | 597.5 | 605.7 | 0.998 | 0.972 to 1.023 | not shown |
| codex pull | 20 | 1301.7 | 1295.3 | 0.982 | 0.952 to 1.047 | not shown |
| cold start | 20 | 53.8 | 53.6 | 0.979 | 0.969 to 1.092 | not shown |
| restart | 20 | 65.9 | 60.1 | 0.978 | 0.939 to 0.995 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38208 -> 38209 | 7424 -> 7344 | 8.0 -> 7.8 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4947 -> 4949 | 6384 -> 6400 | 8.9 -> 8.2 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1131 -> 1130 | 4592 -> 4576 | 8.2 -> 6.2 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1403 -> 1403 | 4624 -> 4640 | 5.8 -> 6.1 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 787 -> 789 | 3583 -> 3580 | 5984 -> 6048 | 7.5 -> 7.6 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1129 -> 1129 | 4560 -> 4576 | 5.9 -> 6.4 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1402 -> 1401 | 4640 -> 4624 | 5.8 -> 5.9 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3972 -> 3966 | 6048 -> 6144 | 7.6 -> 8.2 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1129 -> 1129 | 4576 -> 4576 | 6.5 -> 6.0 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1130 -> 1131 | 4544 -> 4576 | 6.0 -> 6.4 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 553 -> 553 | 3312 -> 3296 | 5.7 -> 5.6 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1477 -> 1477 | 4640 -> 4672 | 5.8 -> 6.0 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 40983 -> 41014 | 12650 -> 12647 | 4768 -> 4736 | 6.2 -> 6.9 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2459 -> 2455 | 5078 -> 5077 | 5056 -> 5088 | 6.1 -> 6.9 |
| acp resume | 14 -> 14 | 13 -> 13 | 61 -> 61 | 167 -> 167 | 21712 -> 21726 | 8657 -> 8570 | 6512 -> 6576 | 8.2 -> 8.4 |
| acp pull | 56 -> 56 | 41 -> 41 | 77 -> 77 | 1376 -> 1376 | 39848 -> 39629 | 21987 -> 22051 | 6640 -> 6672 | 10.4 -> 8.3 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 14180 -> 14167 | 16181 -> 16179 | 4752 -> 4720 | 6.8 -> 6.4 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2568 -> 2570 | 5282 -> 5282 | 5072 -> 5072 | 6.3 -> 6.6 |
| claude resume | 16 -> 16 | 13 -> 13 | 65 -> 65 | 148 -> 148 | 27026 -> 27030 | 8625 -> 8627 | 6496 -> 6576 | 8.6 -> 8.8 |
| claude pull | 61 -> 61 | 41 -> 41 | 83 -> 83 | 1338 -> 1338 | 47608 -> 47602 | 27510 -> 27489 | 6624 -> 6656 | 10.4 -> 9.5 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 15318 -> 15338 | 12363 -> 12379 | 4752 -> 4736 | 6.6 -> 6.7 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2644 -> 2652 | 5300 -> 5300 | 5056 -> 5072 | 6.9 -> 7.0 |
| codex resume | 17 -> 17 | 13 -> 13 | 69 -> 69 | 148 -> 148 | 31040 -> 31043 | 8745 -> 8748 | 6512 -> 6560 | 8.7 -> 9.0 |
| codex pull | 64 -> 64 | 40 -> 40 | 88 -> 88 | 1338 -> 1338 | 52770 -> 52720 | 22990 -> 23006 | 6624 -> 6672 | 8.9 -> 9.7 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5849 -> 5853 | 4528 -> 4512 | 26.7 -> 24.2 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 502599 -> 514959 | 10957 -> 10970 | 4720 -> 4688 | 26.7 -> 26.4 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 7996 -> 7774, cli_maxrss_kib 7424 -> 7344, malloc_bytes 13498003 -> 13498214, mallocs 38208 -> 38209
- acp launch: cli_cpu_us 8916 -> 8247, cli_maxrss_kib 6384 -> 6400, frees 3816 -> 3814, malloc_bytes 977450 -> 980615, mallocs 4947 -> 4949
- acp approve: cli_cpu_us 8220 -> 6202, cli_maxrss_kib 4592 -> 4576, malloc_bytes 420840 -> 421195, mallocs 1131 -> 1130
- claude new: cli_cpu_us 5800 -> 6117, cli_maxrss_kib 4624 -> 4640, frees 1175 -> 1174, malloc_bytes 460507 -> 461198
- claude launch: cli_cpu_us 7481 -> 7645, cli_maxrss_kib 5984 -> 6048, frees 3205 -> 3204, malloc_bytes 709357 -> 711774, mallocs 3583 -> 3580, read_bytes.jobs 613 -> 615, write_bytes.jobs 931 -> 933
- claude approve: cli_cpu_us 5894 -> 6399, cli_maxrss_kib 4560 -> 4576, malloc_bytes 420520 -> 420939
- codex new: cli_cpu_us 5773 -> 5940, cli_maxrss_kib 4640 -> 4624, malloc_bytes 460387 -> 460802, mallocs 1402 -> 1401
- codex launch: cli_cpu_us 7611 -> 8174, cli_maxrss_kib 6048 -> 6144, frees 3567 -> 3569, malloc_bytes 808117 -> 810910, mallocs 3972 -> 3966
- codex approve: cli_cpu_us 6456 -> 6037, malloc_bytes 420520 -> 420939
- ledger append: cli_cpu_us 5980 -> 6371, cli_maxrss_kib 4544 -> 4576, frees 980 -> 981, malloc_bytes 420776 -> 421259, mallocs 1130 -> 1131
- cli version: cli_cpu_us 5688 -> 5566, cli_maxrss_kib 3312 -> 3296, malloc_bytes 264637 -> 264640
- cli ls: cli_cpu_us 5840 -> 6009, cli_maxrss_kib 4640 -> 4672, malloc_bytes 466440 -> 466859
- acp suspend: cli_cpu_us 6194 -> 6936, cli_maxrss_kib 4768 -> 4736, frees 9584 -> 9581, malloc_bytes 3611912 -> 3614998, mallocs 12650 -> 12647, read_bytes.store 38715 -> 38746, write_bytes.jobs 3584 -> 3585, write_bytes.store 95927 -> 95935
- acp move: cli_cpu_us 6073 -> 6936, cli_maxrss_kib 5056 -> 5088, malloc_bytes 1780688 -> 1785017, mallocs 5078 -> 5077, read_bytes.store 2340 -> 2336, write_bytes.ipc 1914 -> 1913
- acp resume: cli_cpu_us 8162 -> 8409, cli_maxrss_kib 6512 -> 6576, frees 6949 -> 6952, malloc_bytes 3758720 -> 3757767, mallocs 8657 -> 8570, read_bytes.jobs 3363 -> 3366, read_bytes.store 18227 -> 18238, read_calls.ipc 65 -> 66, write_bytes.jobs 3556 -> 3552
- acp pull: cli_cpu_us 10388 -> 8299, cli_maxrss_kib 6640 -> 6672, frees 17702 -> 17703, malloc_bytes 7615702 -> 7627900, mallocs 21987 -> 22051, opens 366 -> 368, read_bytes.jobs 6233 -> 6232, read_bytes.state 322 -> 174, read_bytes.store 33293 -> 33223, read_calls.state 8 -> 4, write_bytes.ipc 4085 -> 4084, write_bytes.jobs 8400 -> 8401, write_bytes.store 57369 -> 57324
- claude suspend: cli_cpu_us 6814 -> 6404, cli_maxrss_kib 4752 -> 4720, malloc_bytes 3677733 -> 3681203, mallocs 16181 -> 16179, read_bytes.ipc 5166 -> 5167, read_bytes.store 10538 -> 10525, write_bytes.jobs 4767 -> 4764, write_bytes.store 79659 -> 63187, write_calls.store 45 -> 37
- claude move: cli_cpu_us 6335 -> 6596, malloc_bytes 1814494 -> 1819001, read_bytes.store 2486 -> 2488, write_bytes.ipc 1897 -> 1896
- claude resume: cli_cpu_us 8551 -> 8819, cli_maxrss_kib 6496 -> 6576, frees 7156 -> 7157, malloc_bytes 3774647 -> 3777591, mallocs 8625 -> 8627, read_bytes.jobs 8391 -> 8388, read_bytes.store 18513 -> 18520, read_calls.ipc 73 -> 72, write_bytes.jobs 4708 -> 4693, write_bytes.store 2478 -> 2479
- claude pull: cli_cpu_us 10389 -> 9537, cli_maxrss_kib 6624 -> 6656, frees 22096 -> 22082, malloc_bytes 7968082 -> 7977205, mallocs 27510 -> 27489, read_bytes.ipc 7227 -> 7226, read_bytes.jobs 13422 -> 13421, read_bytes.store 34012 -> 34007, read_calls.ipc 266 -> 268, write_bytes.ipc 3726 -> 3725, write_bytes.jobs 10887 -> 10907, write_bytes.store 73948 -> 90425, write_calls.store 45 -> 53
- codex suspend: cli_cpu_us 6590 -> 6730, cli_maxrss_kib 4752 -> 4736, frees 10104 -> 10117, malloc_bytes 3742364 -> 3746830, mallocs 12363 -> 12379, read_bytes.store 10858 -> 10878, write_bytes.jobs 5649 -> 5640, write_bytes.store 88271 -> 80029, write_calls.store 49 -> 45
- codex move: cli_cpu_us 6948 -> 7015, cli_maxrss_kib 5056 -> 5072, malloc_bytes 1814944 -> 1819525, read_bytes.store 2538 -> 2546, write_bytes.ipc 1889 -> 1888
- codex resume: cli_cpu_us 8685 -> 8998, cli_maxrss_kib 6512 -> 6560, frees 7298 -> 7302, malloc_bytes 3880822 -> 3884237, mallocs 8745 -> 8748, opens 296 -> 298, read_bytes.store 18916 -> 18919, write_bytes.jobs 5633 -> 5627, write_bytes.store 2478 -> 2477
- codex pull: cli_cpu_us 8924 -> 9701, cli_maxrss_kib 6624 -> 6672, frees 18829 -> 18845, malloc_bytes 8114275 -> 8129005, mallocs 22990 -> 23006, opens 460 -> 458, read_bytes.jobs 17803 -> 17799, read_bytes.store 34793 -> 34747, write_bytes.ipc 3652 -> 3651, write_bytes.jobs 12680 -> 12660, write_bytes.store 66078 -> 90783, write_calls.store 41 -> 53
- cold start: cli_cpu_us 26738 -> 24239, cli_maxrss_kib 4528 -> 4512, malloc_bytes 2208778 -> 2212425, mallocs 5849 -> 5853
- restart: cli_cpu_us 26671 -> 26386, cli_maxrss_kib 4720 -> 4688, frees 7756 -> 7765, malloc_bytes 3042476 -> 3080135, mallocs 10957 -> 10970, read_bytes.store 482076 -> 494436, read_calls.store 119 -> 122

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 13280 | 14464 | 13216 | 14320 | 1.008 | 1.002 to 1.014 |
| b | 78 | 13488 | 13872 | 13480 | 14016 | 0.995 | 0.989 to 0.997 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 5984 | 7008 | 5792 | 6784 | 0.974 | 0.967 to 0.980 |
| b | 78 | 6848 | 7200 | 6672 | 7120 | 0.977 | 0.975 to 0.977 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10576 | 10816 | 10592 | 10848 | 1.005 | 1.001 to 1.009 |
| daemon b | 13856 | 14624 | 14256 | 15184 | 1.024 | 1.015 to 1.035 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 4248 | 4496 | 4136 | 4400 | 0.979 | 0.974 to 0.990 |
| daemon b | 7376 | 8192 | 7488 | 8240 | 1.019 | 1.008 to 1.024 |
