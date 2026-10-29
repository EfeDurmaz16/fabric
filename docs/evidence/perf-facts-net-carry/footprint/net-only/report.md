### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 154.8 | 153.0 | 0.985 | 0.970 to 0.995 | faster |
| acp launch | 15 | 60.6 | 60.1 | 0.992 | 0.953 to 1.025 | not shown |
| acp approve | 15 | 15.9 | 16.0 | 1.019 | 0.986 to 1.081 | not shown, median above 1 |
| claude new | 15 | 137.3 | 136.8 | 0.996 | 0.986 to 1.005 | not shown |
| claude launch | 15 | 109.1 | 112.8 | 1.046 | 0.958 to 1.084 | not shown, median above 1 |
| claude approve | 15 | 17.4 | 17.5 | 1.034 | 0.989 to 1.061 | not shown, median above 1 |
| codex new | 15 | 138.6 | 137.2 | 0.995 | 0.978 to 1.046 | not shown |
| codex launch | 15 | 121.7 | 121.1 | 0.983 | 0.930 to 1.057 | not shown |
| codex approve | 15 | 17.4 | 17.2 | 0.997 | 0.969 to 1.032 | not shown |
| ledger append | 15 | 17.1 | 17.0 | 0.960 | 0.898 to 0.991 | faster |
| cli version | 15 | 3.4 | 3.5 | 0.942 | 0.918 to 1.054 | not shown |
| cli ls | 15 | 5.1 | 5.2 | 1.015 | 0.923 to 1.097 | not shown, median above 1 |
| acp suspend | 15 | 359.3 | 356.2 | 0.994 | 0.974 to 1.016 | not shown |
| acp move | 15 | 162.0 | 165.8 | 0.994 | 0.934 to 1.075 | not shown |
| acp resume | 15 | 640.0 | 650.9 | 0.994 | 0.984 to 1.017 | not shown |
| acp pull | 15 | 1179.0 | 1174.1 | 1.000 | 0.994 to 1.007 | not shown |
| claude suspend | 15 | 431.3 | 434.1 | 0.998 | 0.980 to 1.018 | not shown |
| claude move | 15 | 163.7 | 163.1 | 1.008 | 0.963 to 1.020 | not shown, median above 1 |
| claude resume | 15 | 852.8 | 866.5 | 1.010 | 1.002 to 1.016 | slower |
| claude pull | 15 | 1460.5 | 1436.7 | 1.008 | 0.995 to 1.023 | not shown, median above 1 |
| codex suspend | 15 | 647.1 | 653.1 | 1.000 | 0.983 to 1.012 | not shown |
| codex move | 15 | 159.6 | 158.4 | 1.001 | 0.964 to 1.034 | not shown, median above 1 |
| codex resume | 15 | 1064.4 | 1056.7 | 1.007 | 0.980 to 1.015 | not shown, median above 1 |
| codex pull | 15 | 1870.0 | 1852.3 | 1.002 | 0.991 to 1.007 | not shown, median above 1 |
| cold start | 15 | 58.1 | 48.0 | 0.990 | 0.959 to 1.137 | not shown |
| restart | 15 | 372.4 | 364.6 | 0.996 | 0.979 to 1.020 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38531 -> 38525 | 14864 -> 14592 | 20.3 -> 24.7 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4922 -> 4734 | 15448 -> 15272 | 14.4 -> 12.8 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1224 -> 1224 | 10960 -> 10712 | 9.3 -> 9.6 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1688 -> 1687 | 11080 -> 10864 | 10.1 -> 9.9 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1100 | 3622 -> 3627 | 14736 -> 14768 | 13.2 -> 13.1 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1216 -> 1219 | 10952 -> 10696 | 9.9 -> 10.2 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1694 -> 1694 | 11040 -> 10880 | 10.4 -> 10.1 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3992 -> 3964 | 14680 -> 14712 | 13.2 -> 13.4 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1216 -> 1221 | 10928 -> 10688 | 9.6 -> 9.6 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1220 -> 1212 | 10976 -> 10664 | 9.1 -> 9.4 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4880 | 5.6 -> 5.8 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1529 -> 1511 | 11056 -> 10880 | 10.0 -> 9.8 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 32144 -> 32108 | 12838 -> 12841 | 11672 -> 11560 | 9.6 -> 9.8 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5270 -> 5260 | 8376 -> 8364 | 12272 -> 12184 | 11.4 -> 10.3 |
| acp resume | 31 -> 31 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 14496 -> 14396 | 11059 -> 11044 | 15656 -> 15512 | 13.0 -> 12.3 |
| acp pull | 110 -> 110 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 32016 -> 31958 | 28428 -> 28401 | 15792 -> 15520 | 13.5 -> 13.8 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13262 -> 13228 | 15525 -> 15542 | 11680 -> 11528 | 9.9 -> 9.8 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5466 -> 5326 | 8229 -> 8224 | 12240 -> 12192 | 11.4 -> 10.7 |
| claude resume | 33 -> 33 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 18780 -> 18838 | 10893 -> 10896 | 15352 -> 15128 | 14.9 -> 14.7 |
| claude pull | 113 -> 113 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 38890 -> 38819 | 32439 -> 32439 | 15448 -> 15312 | 16.0 -> 16.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14298 -> 14375 | 12541 -> 12558 | 11664 -> 11496 | 10.1 -> 9.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5382 -> 5396 | 8240 -> 8225 | 12280 -> 12160 | 10.5 -> 10.3 |
| codex resume | 34 -> 34 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 22404 -> 22488 | 11044 -> 11044 | 15360 -> 15280 | 14.5 -> 14.2 |
| codex pull | 116 -> 116 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 43456 -> 43662 | 28758 -> 28754 | 15464 -> 15456 | 15.8 -> 15.4 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45296 -> 45222 | 5965 -> 5974 | 10848 -> 10704 | 32.2 -> 32.4 |
| restart | 0 -> 0 | 35 -> 35 | 0 -> 0 | 463 -> 463 | 376744 -> 382924 | 14796 -> 14757 | 11192 -> 11088 | 68.7 -> 68.6 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 20306 -> 24684, cli_maxrss_kib 14864 -> 14592, frees 27799 -> 27794, malloc_bytes 13432544 -> 13432539, mallocs 38531 -> 38525
- acp launch: cli_cpu_us 14380 -> 12833, cli_maxrss_kib 15448 -> 15272, frees 3770 -> 3764, malloc_bytes 877381 -> 827148, mallocs 4922 -> 4734
- acp approve: cli_cpu_us 9332 -> 9643, cli_maxrss_kib 10960 -> 10712, frees 1065 -> 1064, malloc_bytes 326827 -> 326542, mallocs 1224 -> 1224
- claude new: cli_cpu_us 10070 -> 9916, cli_maxrss_kib 11080 -> 10864, frees 1424 -> 1422, malloc_bytes 393045 -> 393184, mallocs 1688 -> 1687
- claude launch: cli_cpu_us 13243 -> 13060, cli_maxrss_kib 14736 -> 14768, frees 3215 -> 3222, malloc_bytes 613984 -> 614590, mallocs 3622 -> 3627
- claude approve: cli_cpu_us 9938 -> 10156, cli_maxrss_kib 10952 -> 10696, frees 1058 -> 1061, malloc_bytes 325867 -> 326062, mallocs 1216 -> 1219
- codex new: cli_cpu_us 10439 -> 10118, cli_maxrss_kib 11040 -> 10880, frees 1429 -> 1431, malloc_bytes 393261 -> 393176, mallocs 1694 -> 1694
- codex launch: cli_cpu_us 13218 -> 13398, cli_maxrss_kib 14680 -> 14712, frees 3564 -> 3539, malloc_bytes 710045 -> 708569, mallocs 3992 -> 3964
- codex approve: cli_cpu_us 9582 -> 9561, cli_maxrss_kib 10928 -> 10688, frees 1058 -> 1062, malloc_bytes 325739 -> 326414, mallocs 1216 -> 1221
- ledger append: cli_cpu_us 9136 -> 9368, cli_maxrss_kib 10976 -> 10664, frees 1060 -> 1054, malloc_bytes 326283 -> 325838, mallocs 1220 -> 1212
- cli version: cli_cpu_us 5604 -> 5805, cli_maxrss_kib 4928 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9969 -> 9822, cli_maxrss_kib 11056 -> 10880, frees 1336 -> 1319, malloc_bytes 367814 -> 366441, mallocs 1529 -> 1511
- acp suspend: cli_cpu_us 9618 -> 9767, cli_maxrss_kib 11672 -> 11560, frees 9748 -> 9750, malloc_bytes 3512442 -> 3512498, mallocs 12838 -> 12841, read_bytes.ipc 5324 -> 5325, read_bytes.store 29756 -> 29720, write_bytes.store 87703 -> 87677
- acp move: cli_cpu_us 11422 -> 10324, cli_maxrss_kib 12272 -> 12184, frees 6878 -> 6864, malloc_bytes 2008464 -> 2008070, mallocs 8376 -> 8364, read_bytes.jobs 4097 -> 4093, read_bytes.store 1172 -> 1168
- acp resume: cli_cpu_us 12962 -> 12340, cli_maxrss_kib 15656 -> 15512, frees 8915 -> 8908, malloc_bytes 4144676 -> 4143436, mallocs 11059 -> 11044, opens 209 -> 208, read_bytes.ipc 4472 -> 4473, read_bytes.jobs 3554 -> 3563, read_bytes.state 196 -> 122, read_bytes.store 10746 -> 10712, read_calls.ipc 156 -> 156, read_calls.state 6 -> 4, write_bytes.jobs 3542 -> 3554, write_bytes.store 3720 -> 3718
- acp pull: cli_cpu_us 13472 -> 13760, cli_maxrss_kib 15792 -> 15520, frees 22808 -> 22793, malloc_bytes 8631006 -> 8627970, mallocs 28428 -> 28401, read_bytes.jobs 8540 -> 8542, read_bytes.store 23302 -> 23242, read_calls.ipc 482 -> 482, write_bytes.jobs 8394 -> 8394, write_bytes.store 55274 -> 55259
- claude suspend: cli_cpu_us 9869 -> 9780, cli_maxrss_kib 11680 -> 11528, frees 12409 -> 12428, malloc_bytes 3547743 -> 3550608, mallocs 15525 -> 15542, read_bytes.ipc 5292 -> 5293, read_bytes.store 9780 -> 9746, write_bytes.jobs 4480 -> 4482, write_bytes.store 63186 -> 50822, write_calls.store 37 -> 31
- claude move: cli_cpu_us 11384 -> 10744, cli_maxrss_kib 12240 -> 12192, frees 6768 -> 6766, malloc_bytes 1995798 -> 1994643, mallocs 8229 -> 8224, opens 75 -> 73, read_bytes.jobs 4072 -> 4080, read_bytes.state 148 -> 0, read_calls.ipc 124 -> 124, read_calls.state 4 -> 0
- claude resume: cli_cpu_us 14889 -> 14724, cli_maxrss_kib 15352 -> 15128, frees 9006 -> 9012, malloc_bytes 4148418 -> 4148492, mallocs 10893 -> 10896, opens 244 -> 245, read_bytes.jobs 7468 -> 7470, read_bytes.state 122 -> 196, read_bytes.store 11190 -> 11172, read_calls.state 4 -> 6, write_bytes.jobs 4422 -> 4429, write_bytes.store 3724 -> 3720
- claude pull: cli_cpu_us 15996 -> 15984, cli_maxrss_kib 15448 -> 15312, frees 26253 -> 26252, malloc_bytes 8852768 -> 8853485, read_bytes.ipc 10194 -> 10194, read_bytes.jobs 14348 -> 14334, read_bytes.store 24294 -> 24236, write_bytes.jobs 10354 -> 10336, write_bytes.store 55432 -> 67764, write_calls.store 36 -> 42
- codex suspend: cli_cpu_us 10092 -> 9298, cli_maxrss_kib 11664 -> 11496, frees 10235 -> 10258, malloc_bytes 3630586 -> 3631643, mallocs 12541 -> 12558, opens 152 -> 153, read_bytes.ipc 5307 -> 5306, read_bytes.state 87 -> 161, read_bytes.store 10112 -> 10116, read_calls.state 2 -> 4, write_bytes.jobs 5252 -> 5254, write_bytes.store 55338 -> 55345
- codex move: cli_cpu_us 10486 -> 10257, cli_maxrss_kib 12280 -> 12160, frees 6784 -> 6772, malloc_bytes 1996126 -> 1994712, mallocs 8240 -> 8225, read_bytes.jobs 4104 -> 4108, read_bytes.store 1278 -> 1288
- codex resume: cli_cpu_us 14518 -> 14243, cli_maxrss_kib 15360 -> 15280, frees 9177 -> 9178, malloc_bytes 4254660 -> 4255379, mallocs 11044 -> 11044, opens 288 -> 289, read_bytes.ipc 4101 -> 4102, read_bytes.jobs 10612 -> 10616, read_bytes.state 122 -> 196, read_bytes.store 11671 -> 11676, read_calls.state 4 -> 6, write_bytes.jobs 5232 -> 5240, write_bytes.store 3722 -> 3720
- codex pull: cli_cpu_us 15816 -> 15393, cli_maxrss_kib 15464 -> 15456, frees 23614 -> 23616, malloc_bytes 8993660 -> 8994880, mallocs 28758 -> 28754, opens 505 -> 507, read_bytes.jobs 18138 -> 18141, read_bytes.state 174 -> 322, read_bytes.store 25144 -> 25200, read_calls.ipc 500 -> 500, read_calls.state 4 -> 8, write_bytes.jobs 11890 -> 11896, write_bytes.store 72278 -> 76393, write_calls.store 44 -> 46
- cold start: cli_cpu_us 32192 -> 32376, cli_maxrss_kib 10848 -> 10704, frees 4466 -> 4476, malloc_bytes 1672538 -> 1672415, mallocs 5965 -> 5974, opens 22 -> 21, read_bytes.state 124 -> 50, read_calls.state 4 -> 2
- restart: cli_cpu_us 68726 -> 68630, cli_maxrss_kib 11192 -> 11088, frees 11034 -> 11036, malloc_bytes 3703064 -> 3701606, mallocs 14796 -> 14757, read_bytes.store 356416 -> 362596, read_calls.store 88 -> 90

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5200 | 5520 | 5008 | 5488 | 0.997 | 0.977 to 1.007 |
| daemon a peak | 5200 | 5520 | 5008 | 5488 | 0.997 | 0.977 to 1.007 |
| daemon b | 12624 | 13568 | 12352 | 13520 | 0.982 | 0.974 to 0.996 |
| daemon b peak | 12688 | 13696 | 12416 | 13728 | 0.984 | 0.977 to 0.999 |
