### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 21 | 441.0 | 591.1 | 1.163 | 1.032 to 1.464 | slower |
| acp launch | 21 | 160.6 | 180.4 | 1.072 | 0.845 to 1.287 | not shown, median above 1 |
| acp approve | 21 | 41.9 | 43.8 | 1.008 | 0.880 to 1.265 | not shown, median above 1 |
| claude new | 21 | 479.7 | 482.6 | 1.024 | 0.710 to 1.151 | not shown, median above 1 |
| claude launch | 21 | 178.4 | 195.8 | 1.084 | 0.974 to 1.180 | not shown, median above 1 |
| claude approve | 21 | 40.2 | 43.3 | 0.975 | 0.767 to 1.133 | not shown |
| codex new | 21 | 490.2 | 418.7 | 0.919 | 0.777 to 1.146 | not shown |
| codex launch | 21 | 270.7 | 271.3 | 1.080 | 0.804 to 1.103 | not shown, median above 1 |
| codex approve | 21 | 36.9 | 37.5 | 1.068 | 0.950 to 1.175 | not shown, median above 1 |
| ledger append | 21 | 38.0 | 36.0 | 0.884 | 0.814 to 1.014 | not shown |
| cli version | 21 | 3.2 | 3.3 | 1.017 | 0.981 to 1.103 | not shown, median above 1 |
| cli ls | 21 | 6.2 | 6.4 | 1.006 | 0.985 to 1.053 | not shown, median above 1 |
| acp suspend | 21 | 1050.8 | 1029.2 | 0.994 | 0.939 to 1.063 | not shown |
| acp move | 21 | 299.0 | 301.1 | 1.005 | 0.941 to 1.099 | not shown, median above 1 |
| acp resume | 21 | 1907.7 | 1896.2 | 1.075 | 0.887 to 1.104 | not shown, median above 1 |
| acp pull | 21 | 3197.7 | 3318.7 | 0.997 | 0.948 to 1.071 | not shown |
| claude suspend | 21 | 1113.5 | 1004.0 | 0.979 | 0.848 to 1.047 | not shown |
| claude move | 21 | 293.3 | 294.9 | 1.004 | 0.931 to 1.037 | not shown, median above 1 |
| claude resume | 21 | 1849.5 | 2053.0 | 1.039 | 0.949 to 1.161 | not shown, median above 1 |
| claude pull | 21 | 3356.1 | 3395.6 | 1.017 | 0.956 to 1.066 | not shown, median above 1 |
| codex suspend | 21 | 1317.7 | 1390.2 | 1.034 | 0.983 to 1.072 | not shown, median above 1 |
| codex move | 21 | 288.0 | 324.2 | 0.975 | 0.873 to 1.139 | not shown |
| codex resume | 21 | 2342.7 | 2276.0 | 1.064 | 0.896 to 1.101 | not shown, median above 1 |
| codex pull | 21 | 4513.1 | 4536.8 | 1.056 | 0.982 to 1.080 | not shown, median above 1 |
| cold start | 21 | 347.6 | 364.7 | 0.993 | 0.672 to 1.307 | not shown |
| restart | 21 | 1382.0 | 1370.1 | 1.022 | 0.945 to 1.052 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38522 -> 38515 | 14800 -> 14928 | 21.8 -> 23.2 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4920 -> 4532 | 15424 -> 15552 | 20.0 -> 14.9 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1209 | 10896 -> 10880 | 11.6 -> 11.5 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1688 -> 1680 | 11008 -> 10976 | 12.8 -> 10.8 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1100 | 3627 -> 3620 | 14784 -> 14880 | 14.0 -> 15.6 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1213 -> 1209 | 10928 -> 10912 | 11.1 -> 12.2 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1703 -> 1680 | 11056 -> 11008 | 10.6 -> 12.1 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3960 -> 3963 | 14736 -> 14752 | 17.8 -> 15.6 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1207 -> 1208 | 10928 -> 10816 | 11.3 -> 13.6 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1209 -> 1211 | 10880 -> 10880 | 11.7 -> 11.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4880 | 8.6 -> 7.7 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1567 -> 1559 | 11040 -> 11040 | 12.0 -> 11.3 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40391 -> 40408 | 12866 -> 12843 | 11728 -> 11728 | 10.3 -> 11.4 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5293 -> 5287 | 8412 -> 8206 | 12320 -> 12384 | 11.3 -> 11.1 |
| acp resume | 33 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16437 -> 16591 | 11543 -> 11153 | 15472 -> 15680 | 18.7 -> 14.8 |
| acp pull | 112 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 34255 -> 34312 | 28734 -> 28437 | 15792 -> 14464 | 17.2 -> 18.3 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13616 -> 13606 | 16394 -> 16372 | 11632 -> 11680 | 12.0 -> 11.1 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5360 -> 5340 | 8267 -> 8051 | 12272 -> 12320 | 12.3 -> 14.1 |
| claude resume | 35 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 22019 -> 22005 | 11448 -> 11431 | 15328 -> 15376 | 19.0 -> 17.6 |
| claude pull | 115 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 42551 -> 42576 | 33868 -> 33588 | 15440 -> 14000 | 18.7 -> 16.5 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14899 -> 14781 | 12936 -> 12563 | 11696 -> 11728 | 11.1 -> 12.4 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5388 -> 5396 | 8270 -> 8055 | 12272 -> 12352 | 10.8 -> 11.2 |
| codex resume | 36 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 26251 -> 26115 | 11585 -> 11564 | 15328 -> 15392 | 18.8 -> 15.4 |
| codex pull | 118 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47797 -> 47812 | 29412 -> 29103 | 14144 -> 15360 | 18.6 -> 19.3 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 13888 -> 17568 | 10896 -> 10752 | 134.1 -> 154.3 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 493385 -> 489265 | 26642 -> 28208 | 11168 -> 11264 | 167.4 -> 183.1 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 21798 -> 23172, cli_maxrss_kib 14800 -> 14928, frees 27789 -> 27787, malloc_bytes 13432416 -> 13432787, mallocs 38522 -> 38515, opens 11 -> 10, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 20007 -> 14919, cli_maxrss_kib 15424 -> 15552, frees 3768 -> 3742, malloc_bytes 876887 -> 777725, mallocs 4920 -> 4532
- acp approve: cli_cpu_us 11607 -> 11502, cli_maxrss_kib 10896 -> 10880, frees 1049 -> 1050, malloc_bytes 325547 -> 326406, mallocs 1208 -> 1209
- claude new: cli_cpu_us 12754 -> 10780, cli_maxrss_kib 11008 -> 10976, frees 1423 -> 1416, malloc_bytes 393293 -> 393044, mallocs 1688 -> 1680, opens 11 -> 10, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 14037 -> 15638, cli_maxrss_kib 14784 -> 14880, frees 3220 -> 3215, malloc_bytes 614591 -> 615235, mallocs 3627 -> 3620, read_bytes.jobs 927 -> 926, write_bytes.jobs 930 -> 929
- claude approve: cli_cpu_us 11071 -> 12176, cli_maxrss_kib 10928 -> 10912, frees 1055 -> 1050, malloc_bytes 325675 -> 325830, mallocs 1213 -> 1209
- codex new: cli_cpu_us 10644 -> 12120, cli_maxrss_kib 11056 -> 11008, frees 1439 -> 1416, malloc_bytes 393977 -> 393496, mallocs 1703 -> 1680, opens 11 -> 10, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 17759 -> 15610, cli_maxrss_kib 14736 -> 14752, frees 3532 -> 3536, malloc_bytes 708252 -> 710465, mallocs 3960 -> 3963
- codex approve: cli_cpu_us 11280 -> 13610, cli_maxrss_kib 10928 -> 10816, frees 1049 -> 1050, malloc_bytes 325291 -> 325702, mallocs 1207 -> 1208
- ledger append: cli_cpu_us 11745 -> 11453, frees 1049 -> 1050, malloc_bytes 326059 -> 326598, mallocs 1209 -> 1211
- cli version: cli_cpu_us 8603 -> 7721, cli_maxrss_kib 4928 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 11995 -> 11294, frees 1369 -> 1361, malloc_bytes 371774 -> 371609, mallocs 1567 -> 1559
- acp suspend: cli_cpu_us 10308 -> 11377, frees 9770 -> 9755, malloc_bytes 3530759 -> 3531222, mallocs 12866 -> 12843, opens 142 -> 139, read_bytes.store 38002 -> 38019, read_calls.ipc 187 -> 183, write_bytes.jobs 3589 -> 3588, write_bytes.store 95959 -> 95965
- acp move: cli_cpu_us 11319 -> 11056, cli_maxrss_kib 12320 -> 12384, frees 6915 -> 6756, malloc_bytes 2024896 -> 2021473, mallocs 8412 -> 8206, opens 73 -> 33, read_bytes.jobs 4107 -> 4103, read_bytes.store 1186 -> 1184, read_calls.ipc 124 -> 76
- acp resume: cli_cpu_us 18717 -> 14838, cli_maxrss_kib 15472 -> 15680, frees 9307 -> 9270, malloc_bytes 4398370 -> 4300080, mallocs 11543 -> 11153, read_bytes.ipc 4928 -> 4932, read_bytes.state 122 -> 270, read_bytes.store 12757 -> 12763, read_calls.ipc 168 -> 166, read_calls.state 4 -> 8, write_bytes.jobs 3555 -> 3553, write_bytes.store 4530 -> 4531
- acp pull: cli_cpu_us 17170 -> 18266, cli_maxrss_kib 15792 -> 14464, frees 23232 -> 23012, malloc_bytes 8850668 -> 8829214, mallocs 28734 -> 28437, opens 416 -> 371, read_bytes.ipc 11155 -> 11156, read_bytes.jobs 8535 -> 8548, read_bytes.store 25398 -> 25442, read_calls.ipc 492 -> 436, write_bytes.jobs 8390 -> 8402, write_bytes.store 60227 -> 60228
- claude suspend: cli_cpu_us 11979 -> 11059, cli_maxrss_kib 11632 -> 11680, frees 13071 -> 13056, malloc_bytes 3605714 -> 3595874, mallocs 16394 -> 16372, opens 145 -> 142, read_bytes.store 9853 -> 9843, read_calls.ipc 185 -> 181, write_bytes.jobs 4757 -> 4766, write_bytes.store 54991 -> 54993
- claude move: cli_cpu_us 12301 -> 14075, cli_maxrss_kib 12272 -> 12320, frees 6805 -> 6632, malloc_bytes 2012587 -> 2009610, mallocs 8267 -> 8051, opens 73 -> 33, read_bytes.jobs 4098 -> 4078, read_calls.ipc 124 -> 76
- claude resume: cli_cpu_us 18970 -> 17601, cli_maxrss_kib 15328 -> 15376, frees 9457 -> 9442, malloc_bytes 4409385 -> 4411018, mallocs 11448 -> 11431, opens 249 -> 247, read_bytes.ipc 4407 -> 4412, read_bytes.jobs 8589 -> 8583, read_bytes.store 13308 -> 13300, read_calls.ipc 173 -> 166, write_bytes.jobs 4710 -> 4705, write_bytes.store 4530 -> 4531
- claude pull: cli_cpu_us 18727 -> 16498, cli_maxrss_kib 15440 -> 14000, frees 27364 -> 27151, malloc_bytes 9151915 -> 9148607, mallocs 33868 -> 33588, opens 461 -> 415, read_bytes.jobs 15741 -> 15742, read_bytes.store 26488 -> 26512, read_calls.ipc 512 -> 450, write_bytes.jobs 10902 -> 10912, write_bytes.store 85091 -> 70233, write_calls.store 51 -> 45
- codex suspend: cli_cpu_us 11124 -> 12438, cli_maxrss_kib 11696 -> 11728, frees 10295 -> 10270, malloc_bytes 3688176 -> 3661204, mallocs 12936 -> 12563, opens 154 -> 149, read_bytes.state 235 -> 87, read_bytes.store 10170 -> 10200, read_calls.ipc 190 -> 186, read_calls.state 6 -> 2, write_bytes.store 63601 -> 88327, write_calls.store 37 -> 49
- codex move: cli_cpu_us 10836 -> 11238, cli_maxrss_kib 12272 -> 12352, frees 6816 -> 6639, malloc_bytes 2012840 -> 2009561, mallocs 8270 -> 8055, opens 73 -> 33, read_bytes.jobs 4094 -> 4102, read_calls.ipc 124 -> 76
- codex resume: cli_cpu_us 18841 -> 15420, cli_maxrss_kib 15328 -> 15392, frees 9615 -> 9597, malloc_bytes 4515649 -> 4516297, mallocs 11585 -> 11564, opens 295 -> 291, read_bytes.ipc 4557 -> 4561, read_bytes.jobs 12198 -> 12197, read_bytes.state 270 -> 122, read_bytes.store 13783 -> 13796, read_calls.ipc 182 -> 177, read_calls.state 8 -> 4, write_bytes.jobs 5632 -> 5638, write_bytes.store 4525 -> 4530
- codex pull: cli_cpu_us 18624 -> 19273, cli_maxrss_kib 14144 -> 15360, frees 24142 -> 23898, malloc_bytes 9300956 -> 9295039, mallocs 29412 -> 29103, opens 512 -> 466, read_bytes.jobs 20125 -> 20100, read_bytes.store 27350 -> 27390, read_calls.ipc 515 -> 450, write_bytes.jobs 12691 -> 12656, write_bytes.store 93694 -> 60757, write_calls.store 55 -> 39
- cold start: cli_cpu_us 134080 -> 154259, cli_maxrss_kib 10896 -> 10752, cli_processes 13 -> 17, frees 11208 -> 14362, malloc_bytes 4281681 -> 5456960, mallocs 13888 -> 17568, write_bytes.ipc 2145 -> 2840, write_calls.ipc 25 -> 33
- restart: cli_cpu_us 167447 -> 183099, cli_maxrss_kib 11168 -> 11264, cli_processes 18 -> 19, frees 21094 -> 22511, malloc_bytes 6966027 -> 7335424, mallocs 26642 -> 28208, opens 284 -> 297, read_bytes.store 469716 -> 465596, read_calls.store 116 -> 115, write_bytes.ipc 6935 -> 8096, write_calls.ipc 97 -> 114

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20736 | 21376 | 20704 | 21440 | 0.999 | 0.992 to 1.002 |
| daemon b | 27136 | 29072 | 27616 | 29984 | 1.009 | 0.997 to 1.023 |
