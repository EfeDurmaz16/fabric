### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 21 | 568.1 | 430.6 | 0.855 | 0.670 to 1.124 | not shown |
| acp launch | 21 | 166.1 | 161.0 | 0.853 | 0.747 to 1.024 | not shown |
| acp approve | 21 | 40.5 | 38.0 | 1.040 | 0.944 to 1.202 | not shown, median above 1 |
| claude new | 21 | 333.9 | 414.0 | 1.089 | 0.951 to 1.246 | not shown, median above 1 |
| claude launch | 21 | 186.7 | 228.8 | 1.174 | 1.061 to 1.240 | slower |
| claude approve | 21 | 42.9 | 43.2 | 1.065 | 0.921 to 1.196 | not shown, median above 1 |
| codex new | 21 | 443.7 | 479.2 | 1.113 | 0.820 to 1.294 | not shown, median above 1 |
| codex launch | 21 | 231.6 | 260.1 | 1.170 | 0.936 to 1.202 | not shown, median above 1 |
| codex approve | 21 | 43.0 | 35.6 | 0.885 | 0.717 to 1.184 | not shown |
| ledger append | 21 | 40.8 | 36.5 | 0.928 | 0.817 to 0.973 | faster |
| cli version | 21 | 4.6 | 4.5 | 0.968 | 0.898 to 1.022 | not shown |
| cli ls | 21 | 9.0 | 8.5 | 1.033 | 0.968 to 1.100 | not shown, median above 1 |
| acp suspend | 21 | 1009.2 | 1027.1 | 0.997 | 0.886 to 1.153 | not shown |
| acp move | 21 | 330.7 | 310.1 | 1.039 | 0.836 to 1.083 | not shown, median above 1 |
| acp resume | 21 | 1722.3 | 1860.6 | 0.976 | 0.909 to 1.066 | not shown |
| acp pull | 21 | 3126.7 | 3256.4 | 1.061 | 0.907 to 1.124 | not shown, median above 1 |
| claude suspend | 21 | 1128.7 | 1060.8 | 0.952 | 0.802 to 1.020 | not shown |
| claude move | 21 | 288.1 | 292.9 | 0.968 | 0.821 to 1.097 | not shown |
| claude resume | 21 | 1988.0 | 2004.9 | 1.056 | 0.961 to 1.113 | not shown, median above 1 |
| claude pull | 21 | 3147.1 | 3304.6 | 1.049 | 0.936 to 1.119 | not shown, median above 1 |
| codex suspend | 21 | 3395.2 | 3204.7 | 1.056 | 0.914 to 1.154 | not shown, median above 1 |
| codex move | 21 | 246.7 | 280.2 | 1.022 | 0.922 to 1.193 | not shown, median above 1 |
| codex resume | 21 | 3873.5 | 4112.1 | 1.066 | 1.032 to 1.166 | slower |
| codex pull | 21 | 7614.2 | 7132.6 | 0.922 | 0.847 to 0.976 | faster |
| cold start | 21 | 329.1 | 350.5 | 1.090 | 0.944 to 1.329 | not shown, median above 1 |
| restart | 21 | 1161.6 | 1309.7 | 1.059 | 0.938 to 1.147 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38954 -> 38524 | 14496 -> 14736 | 22.8 -> 18.6 |
| acp launch | 4 -> 4 | 15 -> 12 | 0 -> 0 | 135 -> 135 | 1186 -> 683 | 4141 -> 4536 | 14368 -> 15520 | 15.1 -> 19.7 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1193 -> 1209 | 10528 -> 10832 | 11.0 -> 11.8 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1671 -> 1684 | 10784 -> 11008 | 11.8 -> 11.5 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1103 -> 1103 | 3524 -> 3620 | 14192 -> 14752 | 16.6 -> 14.1 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1208 | 10528 -> 10848 | 11.2 -> 14.1 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1672 -> 1683 | 10768 -> 10992 | 11.7 -> 9.2 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3848 -> 3968 | 14208 -> 14672 | 16.0 -> 14.1 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1217 | 10544 -> 10816 | 10.3 -> 12.5 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1211 | 10576 -> 10864 | 10.1 -> 11.1 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4896 | 5.7 -> 6.5 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1535 -> 1551 | 10816 -> 11008 | 11.2 -> 9.9 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40241 -> 40397 | 15009 -> 12855 | 11248 -> 11712 | 10.5 -> 10.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5238 -> 5277 | 8340 -> 8264 | 11856 -> 12272 | 11.7 -> 12.4 |
| acp resume | 32 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16116 -> 16432 | 11174 -> 11528 | 14640 -> 15520 | 15.0 -> 15.8 |
| acp pull | 111 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 33532 -> 34264 | 30409 -> 28492 | 14608 -> 14288 | 14.5 -> 16.2 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13336 -> 13619 | 19231 -> 16810 | 11312 -> 11664 | 10.9 -> 10.3 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5263 -> 5337 | 8248 -> 8113 | 11872 -> 12320 | 12.2 -> 11.1 |
| claude resume | 34 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21875 -> 22009 | 11121 -> 11449 | 14672 -> 15232 | 15.4 -> 15.4 |
| claude pull | 114 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 41884 -> 42573 | 36776 -> 33664 | 13616 -> 13952 | 19.8 -> 19.3 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14682 -> 14774 | 16815 -> 13045 | 11248 -> 11648 | 10.5 -> 12.5 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5364 -> 5399 | 8244 -> 8097 | 11920 -> 12336 | 11.6 -> 11.8 |
| codex resume | 35 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 26015 -> 26260 | 11238 -> 11584 | 14752 -> 15232 | 18.4 -> 16.2 |
| codex pull | 117 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47293 -> 47794 | 32632 -> 29144 | 13632 -> 15344 | 18.3 -> 23.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 29214 -> 19315 | 10480 -> 10768 | 285.1 -> 176.7 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 481025 -> 501625 | 28263 -> 27070 | 11072 -> 11248 | 201.9 -> 206.6 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 22802 -> 18617, cli_maxrss_kib 14496 -> 14736, frees 28126 -> 27791, malloc_bytes 13539992 -> 13444347, mallocs 38954 -> 38524, opens 11 -> 13, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 15058 -> 19699, cli_maxrss_kib 14368 -> 15520, cmd:fabric-fake-agent 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 3372 -> 3751, fullfsync 15 -> 12, malloc_bytes 719428 -> 796142, mallocs 4141 -> 4536, opens 40 -> 37, read_bytes.jobs 1012 -> 509, read_calls.jobs 12 -> 10, renames 6 -> 5, spawn:fabric-fake-agent 1 -> 0, spawn:sh 0 -> 1, syncs.jobs 14 -> 11, write_bytes.ipc 969 -> 1025, write_bytes.jobs 1034 -> 577, write_calls.ipc 11 -> 14, write_calls.jobs 6 -> 5
- acp approve: cli_cpu_us 10998 -> 11819, cli_maxrss_kib 10528 -> 10832, frees 1034 -> 1049, malloc_bytes 323703 -> 329974, mallocs 1193 -> 1209
- claude new: cli_cpu_us 11818 -> 11528, cli_maxrss_kib 10784 -> 11008, frees 1407 -> 1418, malloc_bytes 390905 -> 405384, mallocs 1671 -> 1684, opens 11 -> 13, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 16550 -> 14102, cli_maxrss_kib 14192 -> 14752, cmd:claude 1 -> 0, cmd:sh holding claude 0 -> 1, frees 3119 -> 3215, malloc_bytes 604646 -> 629570, mallocs 3524 -> 3620, spawn:claude 1 -> 0, spawn:sh 0 -> 1
- claude approve: cli_cpu_us 11170 -> 14081, cli_maxrss_kib 10528 -> 10848, frees 1033 -> 1049, malloc_bytes 323895 -> 329206, mallocs 1192 -> 1208
- codex new: cli_cpu_us 11747 -> 9182, cli_maxrss_kib 10768 -> 10992, frees 1409 -> 1419, malloc_bytes 391217 -> 403984, mallocs 1672 -> 1683, opens 11 -> 13, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 16024 -> 14053, cli_maxrss_kib 14208 -> 14672, cmd:codex 1 -> 0, cmd:sh holding codex 0 -> 1, frees 3424 -> 3544, malloc_bytes 696543 -> 728275, mallocs 3848 -> 3968, spawn:codex 1 -> 0, spawn:sh 0 -> 1
- codex approve: cli_cpu_us 10319 -> 12526, cli_maxrss_kib 10544 -> 10816, frees 1033 -> 1059, malloc_bytes 323895 -> 329846, mallocs 1192 -> 1217
- ledger append: cli_cpu_us 10141 -> 11070, cli_maxrss_kib 10576 -> 10864, frees 1033 -> 1050, malloc_bytes 323639 -> 329718, mallocs 1192 -> 1211
- cli version: cli_cpu_us 5693 -> 6532, cli_maxrss_kib 4928 -> 4896, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 11199 -> 9932, cli_maxrss_kib 10816 -> 11008, frees 1337 -> 1353, malloc_bytes 368842 -> 374665, mallocs 1535 -> 1551
- acp suspend: cli_cpu_us 10536 -> 9991, cli_maxrss_kib 11248 -> 11712, frees 10355 -> 9762, malloc_bytes 3551087 -> 3549629, mallocs 15009 -> 12855, opens 144 -> 146, read_bytes.ipc 5269 -> 5325, read_bytes.state 235 -> 87, read_bytes.store 37704 -> 38008, read_calls.ipc 186 -> 183, read_calls.state 6 -> 2, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3589 -> 3586, write_bytes.store 95756 -> 95955
- acp move: cli_cpu_us 11690 -> 12383, cli_maxrss_kib 11856 -> 12272, frees 6850 -> 6811, malloc_bytes 2017018 -> 2184152, mallocs 8340 -> 8264, opens 73 -> 117, read_bytes.jobs 4115 -> 4095, read_bytes.store 1123 -> 1182, read_calls.ipc 124 -> 76, write_bytes.ipc 2698 -> 2510
- acp resume: cli_cpu_us 15001 -> 15779, cli_maxrss_kib 14640 -> 15520, cmd:fabric-fake-agent 1 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:sh holding fabric-fake-agent 0 -> 1, frees 8968 -> 9294, malloc_bytes 4355165 -> 4432600, mallocs 11174 -> 11528, opens 212 -> 217, read_bytes.jobs 3558 -> 3560, read_bytes.store 12436 -> 12750, read_calls.ipc 165 -> 169, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 32 -> 33, write_bytes.ipc 2046 -> 2076, write_bytes.jobs 3558 -> 3549, write_bytes.store 4530 -> 4531, write_calls.ipc 28 -> 29
- acp pull: cli_cpu_us 14483 -> 16169, cli_maxrss_kib 14608 -> 14288, cmd:fabric-fake-agent 2 -> 1, cmd:git symbolic-ref 3 -> 4, cmd:sh holding fabric-fake-agent 0 -> 1, frees 23395 -> 23058, malloc_bytes 8816342 -> 9037814, mallocs 30409 -> 28492, opens 417 -> 470, read_bytes.ipc 11102 -> 11157, read_bytes.jobs 8531 -> 8539, read_bytes.store 24679 -> 25403, read_calls.ipc 493 -> 441, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 111 -> 112, write_bytes.ipc 4625 -> 4616, write_bytes.jobs 8384 -> 8383, write_bytes.store 59992 -> 60231, write_calls.ipc 84 -> 85
- claude suspend: cli_cpu_us 10862 -> 10313, cli_maxrss_kib 11312 -> 11664, frees 13990 -> 13088, malloc_bytes 3669623 -> 3721256, mallocs 19231 -> 16810, opens 145 -> 149, read_bytes.ipc 5237 -> 5293, read_bytes.store 9573 -> 9856, read_calls.ipc 185 -> 180, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4756 -> 4746, write_bytes.store 54760 -> 63215, write_calls.store 33 -> 37
- claude move: cli_cpu_us 12166 -> 11102, cli_maxrss_kib 11872 -> 12320, frees 6782 -> 6687, malloc_bytes 2009367 -> 2172020, mallocs 8248 -> 8113, opens 73 -> 117, read_bytes.jobs 4046 -> 4074, read_bytes.store 1217 -> 1263, read_calls.ipc 124 -> 76
- claude resume: cli_maxrss_kib 14672 -> 15232, cmd:claude 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding claude 0 -> 1, frees 9145 -> 9461, malloc_bytes 4374525 -> 4444177, mallocs 11121 -> 11449, opens 250 -> 253, read_bytes.jobs 8589 -> 8586, read_bytes.state 270 -> 122, read_bytes.store 13016 -> 13301, read_calls.ipc 178 -> 176, read_calls.state 8 -> 4, spawn:claude 3 -> 2, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 34 -> 35, write_bytes.ipc 1852 -> 1779, write_bytes.jobs 4699 -> 4690, write_bytes.store 4532 -> 4531
- claude pull: cli_cpu_us 19800 -> 19332, cli_maxrss_kib 13616 -> 13952, cmd:claude 8 -> 7, cmd:git symbolic-ref 3 -> 4, cmd:sh holding claude 0 -> 1, frees 27975 -> 27220, malloc_bytes 9293393 -> 9361416, mallocs 36776 -> 33664, opens 460 -> 517, read_bytes.ipc 10601 -> 10658, read_bytes.jobs 15711 -> 15733, read_bytes.store 25851 -> 26518, read_calls.ipc 505 -> 446, spawn:claude 8 -> 7, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 114 -> 115, write_bytes.ipc 4279 -> 4123, write_bytes.jobs 10863 -> 10898, write_bytes.store 60175 -> 68616, write_calls.store 39 -> 43
- codex suspend: cli_cpu_us 10538 -> 12519, cli_maxrss_kib 11248 -> 11648, frees 11779 -> 10306, malloc_bytes 3961105 -> 3804781, mallocs 16815 -> 13045, opens 154 -> 156, read_bytes.ipc 5251 -> 5307, read_bytes.state 235 -> 87, read_bytes.store 9953 -> 10193, read_calls.ipc 190 -> 186, read_calls.state 6 -> 2, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5649 -> 5655, write_bytes.store 63410 -> 80069, write_calls.store 37 -> 45
- codex move: cli_cpu_us 11600 -> 11778, cli_maxrss_kib 11920 -> 12336, frees 6791 -> 6687, malloc_bytes 2007995 -> 2169310, mallocs 8244 -> 8097, opens 73 -> 117, read_bytes.jobs 4114 -> 4102, read_bytes.store 1250 -> 1297, read_calls.ipc 124 -> 76
- codex resume: cli_cpu_us 18432 -> 16227, cli_maxrss_kib 14752 -> 15232, cmd:codex 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding codex 0 -> 1, frees 9299 -> 9603, malloc_bytes 4478843 -> 4550518, mallocs 11238 -> 11584, opens 294 -> 299, read_bytes.jobs 12199 -> 12201, read_bytes.store 13546 -> 13789, read_calls.ipc 180 -> 178, spawn:codex 3 -> 2, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 35 -> 36, write_bytes.ipc 1786 -> 1713, write_bytes.jobs 5640 -> 5634, write_bytes.store 4532 -> 4530
- codex pull: cli_cpu_us 18307 -> 22994, cli_maxrss_kib 13632 -> 15344, cmd:codex 9 -> 8, cmd:git symbolic-ref 3 -> 4, cmd:sh holding codex 0 -> 1, frees 25204 -> 23952, malloc_bytes 9331771 -> 9505447, mallocs 32632 -> 29144, opens 509 -> 566, read_bytes.ipc 10766 -> 10821, read_bytes.jobs 20109 -> 20108, read_bytes.store 26862 -> 27364, read_calls.ipc 514 -> 456, spawn:codex 9 -> 8, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 117 -> 118, write_bytes.ipc 4213 -> 4057, write_bytes.jobs 12672 -> 12684, write_bytes.store 85285 -> 93714, write_calls.store 51 -> 55
- cold start: cli_cpu_us 285116 -> 176691, cli_maxrss_kib 10480 -> 10768, cli_processes 30 -> 19, frees 24332 -> 15850, malloc_bytes 9228006 -> 6038890, mallocs 29214 -> 19315, write_bytes.ipc 5193 -> 3202, write_calls.ipc 59 -> 37
- restart: cli_cpu_us 201942 -> 206569, cli_maxrss_kib 11072 -> 11248, frees 22803 -> 21241, malloc_bytes 7595653 -> 7398096, mallocs 28263 -> 27070, opens 297 -> 258, read_bytes.store 457356 -> 477956, read_calls.store 113 -> 118, write_bytes.ipc 7493 -> 6986, write_calls.ipc 104 -> 98

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 19776 | 20784 | 20272 | 21376 | 1.030 | 1.019 to 1.039 |
| daemon b | 22208 | 23744 | 22848 | 26528 | 1.033 | 1.020 to 1.088 |
