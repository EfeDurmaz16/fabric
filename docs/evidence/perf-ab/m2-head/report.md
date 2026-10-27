### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 25 | 155.9 | 154.2 | 1.003 | 0.984 to 1.009 | not shown, median above 1 |
| acp launch | 25 | 71.1 | 66.9 | 0.939 | 0.903 to 0.975 | faster |
| acp approve | 25 | 16.1 | 15.9 | 1.001 | 0.975 to 1.046 | not shown, median above 1 |
| claude new | 25 | 135.2 | 133.2 | 0.993 | 0.986 to 1.018 | not shown |
| claude launch | 25 | 90.1 | 109.0 | 1.212 | 1.202 to 1.229 | slower |
| claude approve | 25 | 16.0 | 16.1 | 1.008 | 0.970 to 1.055 | not shown, median above 1 |
| codex new | 25 | 133.0 | 133.0 | 0.986 | 0.974 to 1.007 | not shown |
| codex launch | 25 | 102.8 | 121.2 | 1.179 | 1.163 to 1.207 | slower |
| codex approve | 25 | 16.1 | 16.2 | 1.014 | 0.956 to 1.042 | not shown, median above 1 |
| ledger append | 25 | 16.0 | 16.1 | 0.997 | 0.971 to 1.017 | not shown |
| cli version | 25 | 3.0 | 3.2 | 1.008 | 0.927 to 1.076 | not shown, median above 1 |
| cli ls | 25 | 5.5 | 5.7 | 1.027 | 0.998 to 1.042 | not shown, median above 1 |
| acp suspend | 25 | 389.0 | 391.9 | 0.988 | 0.967 to 1.014 | not shown |
| acp move | 25 | 163.1 | 162.8 | 0.988 | 0.969 to 1.009 | not shown |
| acp resume | 25 | 665.1 | 693.9 | 0.997 | 0.980 to 1.042 | not shown |
| acp pull | 25 | 1154.9 | 1184.8 | 1.015 | 0.981 to 1.023 | not shown, median above 1 |
| claude suspend | 25 | 455.8 | 461.9 | 1.017 | 0.997 to 1.036 | not shown, median above 1 |
| claude move | 25 | 158.0 | 160.0 | 1.010 | 0.984 to 1.022 | not shown, median above 1 |
| claude resume | 25 | 848.8 | 883.8 | 1.040 | 1.020 to 1.065 | slower |
| claude pull | 25 | 1441.8 | 1465.8 | 1.021 | 0.976 to 1.060 | not shown, median above 1 |
| codex suspend | 25 | 683.7 | 677.0 | 0.990 | 0.976 to 1.026 | not shown |
| codex move | 25 | 159.6 | 161.2 | 1.004 | 1.001 to 1.021 | slower |
| codex resume | 25 | 1070.1 | 1086.0 | 1.025 | 1.007 to 1.042 | slower |
| codex pull | 25 | 1919.7 | 1958.9 | 1.012 | 0.999 to 1.054 | not shown, median above 1 |
| cold start | 25 | 58.1 | 58.6 | 1.008 | 0.986 to 1.028 | not shown, median above 1 |
| restart | 25 | 484.6 | 484.8 | 0.994 | 0.969 to 1.004 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38952 -> 38520 | 14400 -> 14816 | 18.5 -> 18.8 |
| acp launch | 4 -> 4 | 15 -> 12 | 0 -> 0 | 135 -> 135 | 1185 -> 683 | 4130 -> 4540 | 14400 -> 15472 | 13.1 -> 14.7 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1197 -> 1210 | 10608 -> 10992 | 9.7 -> 9.8 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1671 -> 1688 | 10848 -> 11088 | 9.1 -> 9.1 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1103 -> 1100 | 3536 -> 3628 | 14208 -> 14768 | 12.3 -> 12.4 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1194 -> 1207 | 10576 -> 10896 | 9.4 -> 8.4 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1672 -> 1691 | 10720 -> 11056 | 9.1 -> 9.2 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3846 -> 3966 | 14160 -> 14752 | 12.9 -> 12.6 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1196 -> 1208 | 10544 -> 10944 | 9.8 -> 9.4 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1194 -> 1211 | 10560 -> 10976 | 8.6 -> 9.8 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4928 | 5.8 -> 5.7 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1535 -> 1589 | 10800 -> 11088 | 9.5 -> 9.1 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40084 -> 40308 | 15014 -> 12866 | 11248 -> 11728 | 8.6 -> 11.7 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5205 -> 5282 | 8334 -> 8419 | 11872 -> 12352 | 9.6 -> 10.1 |
| acp resume | 32 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16075 -> 16331 | 11177 -> 11564 | 14560 -> 15568 | 12.6 -> 13.5 |
| acp pull | 111 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 33228 -> 33875 | 30381 -> 28686 | 14768 -> 15712 | 13.4 -> 14.4 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13302 -> 13512 | 19221 -> 16412 | 11280 -> 11744 | 8.9 -> 11.1 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5276 -> 5313 | 8233 -> 8262 | 11936 -> 12336 | 10.0 -> 10.2 |
| claude resume | 34 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21686 -> 21897 | 11114 -> 11449 | 14768 -> 15328 | 14.1 -> 14.1 |
| claude pull | 114 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 41659 -> 42289 | 36333 -> 33873 | 14912 -> 15456 | 14.1 -> 14.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14430 -> 14677 | 16359 -> 12604 | 11264 -> 11680 | 11.1 -> 10.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5343 -> 5345 | 8286 -> 8265 | 11936 -> 12288 | 10.4 -> 10.4 |
| codex resume | 35 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 25765 -> 26018 | 11227 -> 11590 | 14784 -> 15344 | 13.3 -> 14.2 |
| codex pull | 117 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 46884 -> 47621 | 32625 -> 29374 | 15040 -> 15456 | 18.6 -> 15.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5909 -> 5954 | 10496 -> 10864 | 36.4 -> 38.3 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 493385 -> 497505 | 17649 -> 17374 | 11056 -> 11216 | 86.9 -> 88.5 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 18500 -> 18799, cli_maxrss_kib 14400 -> 14816, frees 28127 -> 27788, malloc_bytes 13539921 -> 13432028, mallocs 38952 -> 38520
- acp launch: cli_cpu_us 13122 -> 14655, cli_maxrss_kib 14400 -> 15472, cmd:fabric-fake-agent 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 3360 -> 3749, fullfsync 15 -> 12, malloc_bytes 718133 -> 776124, mallocs 4130 -> 4540, opens 40 -> 37, read_bytes.jobs 1011 -> 509, read_calls.jobs 12 -> 10, renames 6 -> 5, spawn:fabric-fake-agent 1 -> 0, spawn:sh 0 -> 1, syncs.jobs 14 -> 11, write_bytes.ipc 969 -> 1025, write_bytes.jobs 1033 -> 577, write_calls.ipc 11 -> 14, write_calls.jobs 6 -> 5
- acp approve: cli_cpu_us 9706 -> 9815, cli_maxrss_kib 10608 -> 10992, frees 1038 -> 1049, malloc_bytes 323958 -> 326190, mallocs 1197 -> 1210
- claude new: cli_cpu_us 9126 -> 9147, cli_maxrss_kib 10848 -> 11088, frees 1406 -> 1423, malloc_bytes 391065 -> 393136, mallocs 1671 -> 1688
- claude launch: cli_cpu_us 12339 -> 12387, cli_maxrss_kib 14208 -> 14768, cmd:claude 1 -> 0, cmd:sh holding claude 0 -> 1, frees 3132 -> 3220, malloc_bytes 605253 -> 614834, mallocs 3536 -> 3628, read_bytes.jobs 929 -> 926, spawn:claude 1 -> 0, spawn:sh 0 -> 1, write_bytes.jobs 932 -> 929
- claude approve: cli_cpu_us 9374 -> 8351, cli_maxrss_kib 10576 -> 10896, frees 1036 -> 1049, malloc_bytes 323575 -> 325294, mallocs 1194 -> 1207
- codex new: cli_cpu_us 9078 -> 9249, cli_maxrss_kib 10720 -> 11056, frees 1407 -> 1428, malloc_bytes 391218 -> 392936, mallocs 1672 -> 1691
- codex launch: cli_cpu_us 12887 -> 12627, cli_maxrss_kib 14160 -> 14752, cmd:codex 1 -> 0, cmd:sh holding codex 0 -> 1, frees 3422 -> 3539, malloc_bytes 696477 -> 708416, mallocs 3846 -> 3966, spawn:codex 1 -> 0, spawn:sh 0 -> 1
- codex approve: cli_cpu_us 9819 -> 9446, cli_maxrss_kib 10544 -> 10944, frees 1038 -> 1049, malloc_bytes 323703 -> 325549, mallocs 1196 -> 1208
- ledger append: cli_cpu_us 8611 -> 9797, cli_maxrss_kib 10560 -> 10976, frees 1034 -> 1050, malloc_bytes 324043 -> 326786, mallocs 1194 -> 1211
- cli version: cli_cpu_us 5809 -> 5675, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9477 -> 9076, cli_maxrss_kib 10800 -> 11088, frees 1337 -> 1390, malloc_bytes 368842 -> 373633, mallocs 1535 -> 1589
- acp suspend: cli_cpu_us 8605 -> 11698, cli_maxrss_kib 11248 -> 11728, frees 10357 -> 9778, malloc_bytes 3567211 -> 3513444, mallocs 15014 -> 12866, read_bytes.ipc 5268 -> 5325, read_bytes.store 37695 -> 37919, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3581 -> 3590, write_bytes.store 95724 -> 95919
- acp move: cli_cpu_us 9647 -> 10065, cli_maxrss_kib 11872 -> 12352, frees 6839 -> 6919, malloc_bytes 2016613 -> 2024516, mallocs 8334 -> 8419, read_bytes.jobs 4095 -> 4115, read_bytes.store 1110 -> 1167, write_bytes.ipc 2698 -> 2510
- acp resume: cli_cpu_us 12560 -> 13510, cli_maxrss_kib 14560 -> 15568, cmd:fabric-fake-agent 1 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:sh holding fabric-fake-agent 0 -> 1, frees 8974 -> 9326, malloc_bytes 4355008 -> 4400586, mallocs 11177 -> 11564, opens 212 -> 215, read_bytes.ipc 4937 -> 4936, read_bytes.jobs 3561 -> 3559, read_bytes.store 12392 -> 12650, read_calls.ipc 171 -> 174, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 32 -> 33, write_bytes.ipc 2046 -> 2076, write_bytes.jobs 3553 -> 3555, write_bytes.store 4533 -> 4531, write_calls.ipc 28 -> 29
- acp pull: cli_cpu_us 13446 -> 14370, cli_maxrss_kib 14768 -> 15712, cmd:fabric-fake-agent 2 -> 1, cmd:git symbolic-ref 3 -> 4, cmd:sh holding fabric-fake-agent 0 -> 1, frees 23386 -> 23208, malloc_bytes 8814916 -> 8829746, mallocs 30381 -> 28686, opens 417 -> 416, read_bytes.ipc 11102 -> 11156, read_bytes.jobs 8541 -> 8527, read_bytes.store 24513 -> 25174, read_calls.ipc 498 -> 499, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 111 -> 112, write_bytes.ipc 4625 -> 4616, write_bytes.jobs 8393 -> 8381, write_bytes.store 59966 -> 60197, write_calls.ipc 84 -> 85
- claude suspend: cli_cpu_us 8912 -> 11076, cli_maxrss_kib 11280 -> 11744, frees 13988 -> 13083, malloc_bytes 3668157 -> 3597146, mallocs 19221 -> 16412, read_bytes.ipc 5236 -> 5293, read_bytes.store 9539 -> 9749, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4761 -> 4755, write_bytes.store 54758 -> 54947
- claude move: cli_cpu_us 9994 -> 10232, cli_maxrss_kib 11936 -> 12336, frees 6763 -> 6798, malloc_bytes 2008373 -> 2012169, mallocs 8233 -> 8262, read_bytes.store 1210 -> 1247
- claude resume: cli_cpu_us 14140 -> 14070, cli_maxrss_kib 14768 -> 15328, cmd:claude 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding claude 0 -> 1, frees 9150 -> 9456, malloc_bytes 4373772 -> 4409462, mallocs 11114 -> 11449, opens 248 -> 249, read_bytes.ipc 4416 -> 4415, read_bytes.jobs 8579 -> 8585, read_bytes.store 12985 -> 13190, read_calls.ipc 176 -> 178, spawn:claude 3 -> 2, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 34 -> 35, write_bytes.ipc 1852 -> 1779, write_bytes.jobs 4693 -> 4713, write_bytes.store 4534 -> 4532
- claude pull: cli_cpu_us 14057 -> 14047, cli_maxrss_kib 14912 -> 15456, cmd:claude 8 -> 7, cmd:git symbolic-ref 3 -> 4, cmd:sh holding claude 0 -> 1, frees 27949 -> 27367, malloc_bytes 9183953 -> 9151447, mallocs 36333 -> 33873, opens 456 -> 459, read_bytes.ipc 10601 -> 10658, read_bytes.jobs 15718 -> 15719, read_bytes.state 174 -> 322, read_bytes.store 25767 -> 26248, read_calls.ipc 506 -> 509, read_calls.state 4 -> 8, spawn:claude 8 -> 7, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 114 -> 115, write_bytes.ipc 4279 -> 4123, write_bytes.jobs 10882 -> 10887, write_bytes.store 78249 -> 70204, write_calls.store 49 -> 45
- codex suspend: cli_cpu_us 11061 -> 10288, cli_maxrss_kib 11264 -> 11680, frees 11721 -> 10289, malloc_bytes 3853204 -> 3679672, mallocs 16359 -> 12604, read_bytes.ipc 5251 -> 5307, read_bytes.store 9849 -> 10096, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5658 -> 5629, write_bytes.store 79848 -> 88283, write_calls.store 45 -> 49
- codex move: cli_cpu_us 10432 -> 10376, cli_maxrss_kib 11936 -> 12288, frees 6831 -> 6812, malloc_bytes 2010345 -> 2012210, mallocs 8286 -> 8265, read_bytes.jobs 4106 -> 4066, read_bytes.store 1237 -> 1279
- codex resume: cli_cpu_us 13338 -> 14176, cli_maxrss_kib 14784 -> 15344, cmd:codex 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding codex 0 -> 1, frees 9295 -> 9619, malloc_bytes 4477472 -> 4515800, mallocs 11227 -> 11590, opens 292 -> 293, read_bytes.jobs 12186 -> 12197, read_bytes.store 13457 -> 13699, read_calls.ipc 181 -> 183, spawn:codex 3 -> 2, spawn:git 24 -> 25, spawn:sh 0 -> 1, spawns 35 -> 36, write_bytes.ipc 1786 -> 1713, write_bytes.jobs 5625 -> 5626
- codex pull: cli_cpu_us 18563 -> 15023, cli_maxrss_kib 15040 -> 15456, cmd:codex 9 -> 8, cmd:git symbolic-ref 3 -> 4, cmd:sh holding codex 0 -> 1, frees 25198 -> 24097, malloc_bytes 9330392 -> 9298620, mallocs 32625 -> 29374, opens 509 -> 512, read_bytes.ipc 10764 -> 10822, read_bytes.jobs 20108 -> 20118, read_bytes.state 174 -> 322, read_bytes.store 26602 -> 27181, read_calls.ipc 513 -> 517, read_calls.state 4 -> 8, spawn:codex 9 -> 8, spawn:git 58 -> 59, spawn:sh 0 -> 1, spawns 117 -> 118, write_bytes.ipc 4213 -> 4057, write_bytes.jobs 12674 -> 12693, write_bytes.store 93472 -> 85435, write_calls.store 55 -> 51
- cold start: cli_cpu_us 36380 -> 38297, cli_maxrss_kib 10496 -> 10864, frees 4439 -> 4453, malloc_bytes 1659586 -> 1672319, mallocs 5909 -> 5954, write_bytes.ipc 487 -> 516
- restart: cli_cpu_us 86909 -> 88500, cli_maxrss_kib 11056 -> 11216, frees 12977 -> 13081, malloc_bytes 4445084 -> 4383456, mallocs 17649 -> 17374, opens 258 -> 245, read_bytes.store 469716 -> 473836, read_calls.store 116 -> 117, write_bytes.ipc 3701 -> 3860, write_calls.ipc 53 -> 55

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20128 | 20832 | 20624 | 21360 | 1.024 | 1.022 to 1.027 |
| daemon b | 30080 | 31120 | 30976 | 32384 | 1.038 | 1.025 to 1.043 |
