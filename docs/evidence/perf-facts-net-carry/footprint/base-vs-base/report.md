### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 153.7 | 155.7 | 0.999 | 0.981 to 1.026 | not shown |
| acp launch | 15 | 60.8 | 61.1 | 1.001 | 0.974 to 1.040 | not shown, median above 1 |
| acp approve | 15 | 15.3 | 16.0 | 1.036 | 0.992 to 1.057 | not shown, median above 1 |
| claude new | 15 | 137.4 | 136.3 | 0.997 | 0.981 to 1.017 | not shown |
| claude launch | 15 | 112.8 | 110.3 | 1.028 | 0.935 to 1.039 | not shown, median above 1 |
| claude approve | 15 | 17.2 | 17.5 | 1.018 | 0.968 to 1.059 | not shown, median above 1 |
| codex new | 15 | 137.5 | 137.2 | 0.993 | 0.990 to 1.026 | not shown |
| codex launch | 15 | 127.0 | 126.3 | 1.015 | 0.943 to 1.039 | not shown, median above 1 |
| codex approve | 15 | 17.5 | 17.4 | 1.012 | 0.962 to 1.024 | not shown, median above 1 |
| ledger append | 15 | 17.1 | 17.1 | 1.010 | 0.978 to 1.042 | not shown, median above 1 |
| cli version | 15 | 3.7 | 3.5 | 0.962 | 0.917 to 1.016 | not shown |
| cli ls | 15 | 5.0 | 5.4 | 0.962 | 0.833 to 1.071 | not shown |
| acp suspend | 15 | 353.8 | 356.2 | 1.007 | 0.992 to 1.018 | not shown, median above 1 |
| acp move | 15 | 164.6 | 166.1 | 0.990 | 0.981 to 1.055 | not shown |
| acp resume | 15 | 655.8 | 646.2 | 0.990 | 0.977 to 1.003 | not shown |
| acp pull | 15 | 1166.8 | 1148.5 | 0.998 | 0.976 to 1.036 | not shown |
| claude suspend | 15 | 431.7 | 437.4 | 1.002 | 0.994 to 1.013 | not shown, median above 1 |
| claude move | 15 | 166.0 | 164.9 | 0.983 | 0.973 to 1.011 | not shown |
| claude resume | 15 | 828.8 | 855.7 | 1.008 | 0.997 to 1.014 | not shown, median above 1 |
| claude pull | 15 | 1443.8 | 1459.6 | 0.995 | 0.967 to 1.006 | not shown |
| codex suspend | 15 | 647.9 | 648.8 | 0.999 | 0.977 to 1.021 | not shown |
| codex move | 15 | 163.0 | 167.2 | 1.019 | 0.989 to 1.037 | not shown, median above 1 |
| codex resume | 15 | 1025.8 | 1047.2 | 1.017 | 0.989 to 1.021 | not shown, median above 1 |
| codex pull | 15 | 1864.6 | 1869.8 | 1.003 | 0.980 to 1.007 | not shown, median above 1 |
| cold start | 15 | 62.7 | 57.7 | 0.986 | 0.921 to 1.013 | not shown |
| restart | 15 | 366.8 | 352.4 | 0.997 | 0.982 to 1.017 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38522 -> 38534 | 14888 -> 14912 | 20.3 -> 25.0 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4738 -> 4543 | 15392 -> 15408 | 12.8 -> 13.4 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1226 -> 1220 | 10936 -> 10944 | 9.9 -> 9.5 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1689 -> 1690 | 11064 -> 11032 | 9.8 -> 10.1 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1104 -> 1104 | 3630 -> 3622 | 14776 -> 14752 | 13.9 -> 13.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1212 -> 1210 | 10960 -> 10896 | 10.3 -> 9.5 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1689 -> 1688 | 11008 -> 11064 | 10.5 -> 11.4 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3964 -> 3974 | 14672 -> 14728 | 13.6 -> 13.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1210 -> 1222 | 10968 -> 10936 | 10.2 -> 9.4 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1212 -> 1216 | 10952 -> 10920 | 9.5 -> 9.1 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4928 | 6.2 -> 5.7 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1519 -> 1512 | 11032 -> 11008 | 9.7 -> 9.2 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 32128 -> 32146 | 12846 -> 12848 | 11664 -> 11672 | 10.1 -> 9.4 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5284 -> 5300 | 8366 -> 8374 | 12312 -> 12232 | 11.0 -> 10.2 |
| acp resume | 31 -> 31 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 14488 -> 14431 | 11054 -> 10848 | 15656 -> 15576 | 12.5 -> 12.4 |
| acp pull | 110 -> 110 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 31973 -> 32034 | 28432 -> 27782 | 15752 -> 15752 | 13.6 -> 13.4 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13244 -> 13309 | 15522 -> 15534 | 11704 -> 11640 | 10.4 -> 9.7 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5474 -> 5333 | 8230 -> 8224 | 12280 -> 12312 | 10.4 -> 10.5 |
| claude resume | 33 -> 33 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 18772 -> 18836 | 10898 -> 10909 | 15320 -> 15336 | 15.4 -> 15.1 |
| claude pull | 113 -> 113 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 38868 -> 38831 | 32438 -> 32453 | 15472 -> 15448 | 15.8 -> 16.2 |
| codex suspend | 37 -> 37 | 40 -> 39 | 8 -> 8 | 522 -> 522 | 14280 -> 14346 | 12544 -> 12578 | 11664 -> 11640 | 9.8 -> 9.7 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5335 -> 5391 | 8225 -> 8225 | 12280 -> 12272 | 10.4 -> 10.7 |
| codex resume | 34 -> 34 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 22398 -> 22466 | 11040 -> 11046 | 15352 -> 15336 | 14.9 -> 14.4 |
| codex pull | 116 -> 116 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 43455 -> 43609 | 28756 -> 28760 | 15472 -> 15480 | 15.1 -> 15.8 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45296 -> 45222 | 5958 -> 5984 | 10824 -> 10840 | 31.2 -> 32.4 |
| restart | 0 -> 0 | 35 -> 35 | 0 -> 0 | 463 -> 463 | 395284 -> 384984 | 14782 -> 15586 | 11176 -> 11160 | 67.7 -> 71.2 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 20322 -> 24986, cli_maxrss_kib 14888 -> 14912, frees 27794 -> 27804, malloc_bytes 13432086 -> 13433401, mallocs 38522 -> 38534
- acp launch: cli_cpu_us 12844 -> 13402, cli_maxrss_kib 15392 -> 15408, frees 3764 -> 3753, malloc_bytes 827375 -> 776710, mallocs 4738 -> 4543
- acp approve: cli_cpu_us 9913 -> 9470, cli_maxrss_kib 10936 -> 10944, frees 1066 -> 1060, malloc_bytes 326835 -> 326510, mallocs 1226 -> 1220
- claude new: cli_cpu_us 9780 -> 10098, cli_maxrss_kib 11064 -> 11032, frees 1424 -> 1424, malloc_bytes 393105 -> 393658, mallocs 1689 -> 1690
- claude launch: cli_cpu_us 13910 -> 12972, cli_maxrss_kib 14776 -> 14752, frees 3222 -> 3216, malloc_bytes 614722 -> 614052, mallocs 3630 -> 3622
- claude approve: cli_cpu_us 10320 -> 9502, cli_maxrss_kib 10960 -> 10896, frees 1054 -> 1052, malloc_bytes 325643 -> 325486, mallocs 1212 -> 1210
- codex new: cli_cpu_us 10456 -> 11372, cli_maxrss_kib 11008 -> 11064, frees 1426 -> 1424, malloc_bytes 393029 -> 392760, mallocs 1689 -> 1688
- codex launch: cli_cpu_us 13638 -> 13028, cli_maxrss_kib 14672 -> 14728, frees 3536 -> 3548, malloc_bytes 708394 -> 708974, mallocs 3964 -> 3974, read_calls.ipc 13 -> 12
- codex approve: cli_cpu_us 10188 -> 9364, cli_maxrss_kib 10968 -> 10936, frees 1052 -> 1064, malloc_bytes 325483 -> 326222, mallocs 1210 -> 1222
- ledger append: cli_cpu_us 9514 -> 9132, cli_maxrss_kib 10952 -> 10920, frees 1052 -> 1058, malloc_bytes 326283 -> 326094, mallocs 1212 -> 1216
- cli version: cli_cpu_us 6183 -> 5652, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9676 -> 9216, cli_maxrss_kib 11032 -> 11008, frees 1326 -> 1320, malloc_bytes 367174 -> 366985, mallocs 1519 -> 1512
- acp suspend: cli_cpu_us 10062 -> 9443, cli_maxrss_kib 11664 -> 11672, frees 9746 -> 9753, malloc_bytes 3521538 -> 3521376, mallocs 12846 -> 12848, read_bytes.ipc 5324 -> 5325, read_bytes.store 29740 -> 29756, write_bytes.jobs 3588 -> 3600, write_bytes.store 87698 -> 87702
- acp move: cli_cpu_us 11016 -> 10182, cli_maxrss_kib 12312 -> 12232, frees 6868 -> 6876, malloc_bytes 2008364 -> 2008970, mallocs 8366 -> 8374, read_bytes.jobs 4115 -> 4127, read_bytes.store 1168 -> 1173, write_bytes.ipc 2510 -> 2509
- acp resume: cli_cpu_us 12487 -> 12422, cli_maxrss_kib 15656 -> 15576, frees 8910 -> 8895, malloc_bytes 4144418 -> 4092560, mallocs 11054 -> 10848, opens 209 -> 208, read_bytes.ipc 4466 -> 4469, read_bytes.jobs 3558 -> 3560, read_bytes.state 196 -> 122, read_bytes.store 10734 -> 10750, read_calls.ipc 156 -> 157, read_calls.state 6 -> 4, write_bytes.jobs 3557 -> 3560, write_bytes.store 3722 -> 3722
- acp pull: cli_cpu_us 13638 -> 13442, frees 22802 -> 22764, malloc_bytes 8630286 -> 8463060, mallocs 28432 -> 27782, read_bytes.jobs 8550 -> 8546, read_bytes.store 23249 -> 23313, read_calls.ipc 483 -> 482, write_bytes.ipc 4616 -> 4615, write_bytes.jobs 8412 -> 8404, write_bytes.store 55265 -> 55280
- claude suspend: cli_cpu_us 10422 -> 9650, cli_maxrss_kib 11704 -> 11640, frees 12408 -> 12420, malloc_bytes 3547450 -> 3549015, mallocs 15522 -> 15534, opens 145 -> 146, read_bytes.state 87 -> 161, read_bytes.store 9762 -> 9752, read_calls.state 2 -> 4, write_bytes.store 50834 -> 50824
- claude move: cli_cpu_us 10423 -> 10462, cli_maxrss_kib 12280 -> 12312, frees 6772 -> 6764, malloc_bytes 1995754 -> 1994882, mallocs 8230 -> 8224, opens 75 -> 73, read_bytes.jobs 4078 -> 4086, read_bytes.state 148 -> 0, read_bytes.store 1248 -> 1247, read_calls.state 4 -> 0, write_bytes.ipc 2362 -> 2361
- claude resume: cli_cpu_us 15432 -> 15086, cli_maxrss_kib 15320 -> 15336, frees 9011 -> 9022, malloc_bytes 4148312 -> 4150770, mallocs 10898 -> 10909, opens 244 -> 245, read_bytes.jobs 7466 -> 7468, read_bytes.state 122 -> 196, read_bytes.store 11184 -> 11172, read_calls.state 4 -> 6, write_bytes.jobs 4414 -> 4424, write_bytes.store 3723 -> 3721
- claude pull: cli_cpu_us 15762 -> 16174, cli_maxrss_kib 15472 -> 15448, frees 26246 -> 26264, malloc_bytes 8852670 -> 8853676, mallocs 32438 -> 32453, read_bytes.ipc 10194 -> 10192, read_bytes.jobs 14340 -> 14330, read_bytes.store 24280 -> 24252, read_calls.ipc 490 -> 492, write_bytes.ipc 4122 -> 4122, write_bytes.jobs 10341 -> 10338, write_bytes.store 55424 -> 55414
- codex suspend: cli_cpu_us 9777 -> 9662, cli_maxrss_kib 11664 -> 11640, frees 10242 -> 10270, fullfsync 40 -> 39, malloc_bytes 3622034 -> 3625500, mallocs 12544 -> 12578, read_bytes.ipc 5307 -> 5306, read_bytes.state 87 -> 161, read_bytes.store 10096 -> 10088, read_calls.state 2 -> 4, syncs.store 18 -> 17, write_bytes.jobs 5227 -> 5250, write_bytes.store 75928 -> 63566, write_calls.store 43 -> 37
- codex move: cli_cpu_us 10423 -> 10660, cli_maxrss_kib 12280 -> 12272, frees 6773 -> 6774, malloc_bytes 1994632 -> 1994358, read_bytes.jobs 4058 -> 4112, read_bytes.store 1277 -> 1279, write_bytes.ipc 2362 -> 2361
- codex resume: cli_cpu_us 14859 -> 14450, cli_maxrss_kib 15352 -> 15336, frees 9172 -> 9180, malloc_bytes 4254676 -> 4255182, mallocs 11040 -> 11046, opens 288 -> 289, read_bytes.jobs 10618 -> 10616, read_bytes.state 122 -> 196, read_bytes.store 11658 -> 11653, read_calls.ipc 166 -> 166, read_calls.state 4 -> 6, write_bytes.jobs 5235 -> 5244, write_bytes.store 3722 -> 3719
- codex pull: cli_cpu_us 15057 -> 15758, cli_maxrss_kib 15472 -> 15480, frees 23610 -> 23610, malloc_bytes 8994244 -> 8994882, mallocs 28756 -> 28760, opens 505 -> 507, read_bytes.ipc 10356 -> 10354, read_bytes.jobs 18142 -> 18149, read_bytes.state 174 -> 322, read_bytes.store 25139 -> 25138, read_calls.ipc 500 -> 499, read_calls.state 4 -> 8, write_bytes.ipc 4056 -> 4056, write_bytes.jobs 11892 -> 11907, write_bytes.store 80516 -> 76386, write_calls.store 48 -> 46
- cold start: cli_cpu_us 31194 -> 32378, cli_maxrss_kib 10824 -> 10840, frees 4458 -> 4484, malloc_bytes 1671834 -> 1673902, mallocs 5958 -> 5984, opens 22 -> 21, read_bytes.state 124 -> 50, read_calls.state 4 -> 2, write_bytes.ipc 487 -> 502
- restart: cli_cpu_us 67667 -> 71204, cli_maxrss_kib 11176 -> 11160, cli_processes 8 -> 8, frees 11018 -> 11728, malloc_bytes 3704747 -> 3891608, mallocs 14782 -> 15586, opens 216 -> 221, read_bytes.store 374956 -> 364656, read_calls.store 93 -> 90, write_bytes.ipc 2933 -> 3316, write_calls.ipc 42 -> 48

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5088 | 5456 | 5152 | 5536 | 1.000 | 0.985 to 1.009 |
| daemon a peak | 5088 | 5456 | 5152 | 5536 | 1.000 | 0.985 to 1.009 |
| daemon b | 12704 | 13680 | 12608 | 13584 | 0.992 | 0.989 to 0.994 |
| daemon b peak | 12736 | 13856 | 12640 | 13632 | 0.992 | 0.989 to 0.994 |
