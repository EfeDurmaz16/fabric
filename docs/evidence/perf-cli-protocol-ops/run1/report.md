### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 468.8 | 343.4 | 0.786 | 0.704 to 0.834 | faster |
| acp launch | 20 | 185.0 | 137.9 | 0.720 | 0.616 to 0.821 | faster |
| acp approve | 20 | 40.7 | 33.6 | 0.798 | 0.728 to 0.941 | faster |
| claude new | 20 | 352.1 | 346.7 | 0.888 | 0.715 to 1.141 | not shown |
| claude launch | 20 | 164.4 | 167.4 | 1.028 | 0.867 to 1.135 | not shown, median above 1 |
| claude approve | 20 | 34.8 | 34.5 | 1.054 | 0.839 to 1.269 | not shown, median above 1 |
| codex new | 20 | 384.7 | 350.5 | 0.850 | 0.677 to 1.056 | not shown |
| codex launch | 20 | 205.1 | 214.2 | 1.028 | 0.895 to 1.173 | not shown, median above 1 |
| codex approve | 20 | 30.8 | 35.0 | 1.014 | 0.927 to 1.220 | not shown, median above 1 |
| ledger append | 20 | 33.9 | 38.1 | 0.975 | 0.814 to 1.180 | not shown |
| cli version | 20 | 4.5 | 4.6 | 1.041 | 0.950 to 1.115 | not shown, median above 1 |
| cli ls | 20 | 8.2 | 7.9 | 0.954 | 0.857 to 1.037 | not shown |
| acp suspend | 20 | 946.3 | 860.3 | 0.946 | 0.864 to 1.010 | not shown |
| acp move | 20 | 274.6 | 254.0 | 0.938 | 0.825 to 1.032 | not shown |
| acp resume | 20 | 1695.6 | 1626.8 | 1.016 | 0.889 to 1.068 | not shown, median above 1 |
| acp pull | 20 | 3020.5 | 2807.0 | 0.902 | 0.839 to 1.080 | not shown |
| claude suspend | 20 | 1023.8 | 964.9 | 0.960 | 0.848 to 0.998 | faster |
| claude move | 20 | 296.5 | 258.4 | 0.891 | 0.852 to 0.965 | faster |
| claude resume | 20 | 1828.5 | 1698.2 | 0.887 | 0.804 to 0.979 | faster |
| claude pull | 20 | 2956.4 | 2765.7 | 0.944 | 0.870 to 1.038 | not shown |
| codex suspend | 20 | 1577.1 | 1356.4 | 0.942 | 0.845 to 1.088 | not shown |
| codex move | 20 | 262.7 | 308.6 | 1.108 | 0.947 to 1.273 | not shown, median above 1 |
| codex resume | 20 | 2544.7 | 2315.2 | 0.913 | 0.855 to 1.011 | not shown |
| codex pull | 20 | 3813.8 | 3996.3 | 0.982 | 0.916 to 1.065 | not shown |
| cold start | 20 | 228.6 | 275.6 | 1.040 | 0.646 to 1.262 | not shown, median above 1 |
| restart | 20 | 969.1 | 308.9 | 0.273 | 0.249 to 0.332 | faster |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 1 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38956 -> 37841 | 14848 -> 14592 | 27.7 -> 23.1 |
| acp launch | 4 -> 2 | 15 -> 11 | 0 -> 0 | 135 -> 135 | 1186 -> 295 | 4119 -> 3980 | 14384 -> 15136 | 16.1 -> 15.7 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1193 -> 754 | 10576 -> 10352 | 11.0 -> 10.4 |
| claude new | 3 -> 1 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1672 -> 1029 | 10784 -> 10608 | 12.1 -> 13.8 |
| claude launch | 4 -> 2 | 15 -> 14 | 0 -> 0 | 116 -> 116 | 1247 -> 789 | 3524 -> 3076 | 14224 -> 14432 | 16.1 -> 14.8 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1191 -> 753 | 10528 -> 10336 | 13.7 -> 9.5 |
| codex new | 3 -> 1 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1670 -> 1027 | 10768 -> 10576 | 12.3 -> 9.6 |
| codex launch | 4 -> 2 | 17 -> 16 | 0 -> 0 | 116 -> 116 | 685 -> 297 | 3844 -> 3413 | 14128 -> 14336 | 15.1 -> 15.2 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1191 -> 754 | 10544 -> 10352 | 11.4 -> 10.1 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 755 | 10560 -> 10368 | 12.2 -> 9.9 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4960 | 6.8 -> 6.8 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1537 -> 1097 | 10832 -> 10528 | 9.5 -> 7.3 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40063 -> 40260 | 15001 -> 12427 | 11264 -> 11216 | 11.6 -> 13.4 |
| acp move | 42 -> 36 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5232 -> 1298 | 8344 -> 7155 | 11856 -> 11920 | 17.5 -> 11.2 |
| acp resume | 32 -> 29 | 24 -> 23 | 51 -> 51 | 167 -> 167 | 16208 -> 16218 | 11165 -> 10883 | 14576 -> 15296 | 20.0 -> 16.9 |
| acp pull | 111 -> 102 | 81 -> 80 | 59 -> 59 | 1376 -> 1376 | 33370 -> 31767 | 30018 -> 27364 | 13424 -> 14096 | 26.4 -> 17.7 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13286 -> 13584 | 19227 -> 16379 | 11200 -> 11232 | 18.9 -> 10.8 |
| claude move | 42 -> 39 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5259 -> 1338 | 8226 -> 7304 | 11888 -> 11872 | 13.5 -> 10.8 |
| claude resume | 34 -> 31 | 24 -> 23 | 55 -> 55 | 148 -> 148 | 21829 -> 21910 | 11098 -> 10868 | 14720 -> 14640 | 21.3 -> 17.1 |
| claude pull | 114 -> 108 | 83 -> 82 | 63 -> 63 | 1338 -> 1338 | 41807 -> 40154 | 36336 -> 32910 | 13744 -> 13904 | 18.8 -> 24.4 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14641 -> 14814 | 16264 -> 12631 | 10128 -> 10672 | 13.7 -> 14.3 |
| codex move | 42 -> 39 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5331 -> 1399 | 8255 -> 7306 | 11888 -> 11904 | 15.8 -> 12.1 |
| codex resume | 35 -> 32 | 24 -> 23 | 59 -> 59 | 148 -> 148 | 25994 -> 26080 | 11238 -> 11016 | 13488 -> 13712 | 20.7 -> 18.3 |
| codex pull | 117 -> 111 | 82 -> 81 | 67 -> 67 | 1338 -> 1338 | 47361 -> 45715 | 32692 -> 29353 | 13648 -> 13808 | 24.2 -> 23.3 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 9368 -> 7577 | 10416 -> 10320 | 85.1 -> 137.0 |
| restart | 0 -> 0 | 38 -> 12 | 0 -> 0 | 463 -> 463 | 476905 -> 506719 | 19346 -> 12915 | 11008 -> 10720 | 145.5 -> 74.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 27661 -> 23090, cli_maxrss_kib 14848 -> 14592, cmd:defaults 2 -> 0, frees 28127 -> 27176, malloc_bytes 13540216 -> 13325289, mallocs 38956 -> 37841, opens 11 -> 9, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1
- acp launch: cli_cpu_us 16110 -> 15659, cli_maxrss_kib 14384 -> 15136, cmd:fabric-fake-agent 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 3352 -> 3233, fullfsync 15 -> 11, malloc_bytes 717731 -> 684935, mallocs 4119 -> 3980, opens 40 -> 27, read_bytes.ipc 500 -> 494, read_bytes.jobs 1012 -> 121, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 2, renames 6 -> 5, spawn:fabric-fake-agent 1 -> 0, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 10, write_bytes.ipc 969 -> 1025, write_bytes.jobs 1034 -> 577, write_calls.ipc 11 -> 14, write_calls.jobs 6 -> 5
- acp approve: cli_cpu_us 10999 -> 10373, cli_maxrss_kib 10576 -> 10352, frees 1033 -> 619, malloc_bytes 324151 -> 247270, mallocs 1193 -> 754, opens 3 -> 2
- claude new: cli_cpu_us 12149 -> 13780, cli_maxrss_kib 10784 -> 10608, cmd:defaults 2 -> 0, frees 1407 -> 812, malloc_bytes 391277 -> 287217, mallocs 1672 -> 1029, opens 11 -> 9, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1
- claude launch: cli_cpu_us 16127 -> 14761, cli_maxrss_kib 14224 -> 14432, cmd:claude 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding claude 0 -> 1, frees 3122 -> 2709, fullfsync 15 -> 14, malloc_bytes 604888 -> 523638, mallocs 3524 -> 3076, opens 45 -> 35, read_bytes.ipc 169 -> 163, read_bytes.jobs 925 -> 615, read_bytes.state 322 -> 174, read_calls.ipc 11 -> 6, read_calls.jobs 12 -> 6, read_calls.state 8 -> 4, spawn:claude 1 -> 0, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 4 -> 2, syncs.jobs 14 -> 13, write_bytes.jobs 928 -> 933
- claude approve: cli_cpu_us 13722 -> 9520, cli_maxrss_kib 10528 -> 10336, frees 1033 -> 619, malloc_bytes 323383 -> 247014, mallocs 1191 -> 753, opens 3 -> 2
- codex new: cli_cpu_us 12298 -> 9572, cli_maxrss_kib 10768 -> 10576, cmd:defaults 2 -> 0, frees 1406 -> 812, malloc_bytes 390673 -> 287093, mallocs 1670 -> 1027, opens 11 -> 9, read_calls.ipc 6 -> 2, spawn:defaults 2 -> 0, spawns 3 -> 1
- codex launch: cli_cpu_us 15137 -> 15250, cli_maxrss_kib 14128 -> 14336, cmd:codex 1 -> 0, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding codex 0 -> 1, frees 3421 -> 3025, fullfsync 17 -> 16, malloc_bytes 696349 -> 617797, mallocs 3844 -> 3413, opens 46 -> 36, read_bytes.ipc 327 -> 321, read_bytes.jobs 511 -> 123, read_calls.ipc 12 -> 7, read_calls.jobs 12 -> 4, spawn:codex 1 -> 0, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 4 -> 2, syncs.jobs 16 -> 15
- codex approve: cli_cpu_us 11411 -> 10073, cli_maxrss_kib 10544 -> 10352, frees 1033 -> 620, malloc_bytes 323383 -> 247078, mallocs 1191 -> 754, opens 3 -> 2
- ledger append: cli_cpu_us 12225 -> 9896, cli_maxrss_kib 10560 -> 10368, frees 1033 -> 620, malloc_bytes 323639 -> 247334, mallocs 1192 -> 755, opens 3 -> 2
- cli version: cli_cpu_us 6790 -> 6850, cli_maxrss_kib 4928 -> 4960, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 9535 -> 7310, cli_maxrss_kib 10832 -> 10528, frees 1338 -> 923, malloc_bytes 369418 -> 292473, mallocs 1537 -> 1097, opens 13 -> 0
- acp suspend: cli_cpu_us 11550 -> 13380, cli_maxrss_kib 11264 -> 11216, frees 10347 -> 9338, malloc_bytes 3566160 -> 3453623, mallocs 15001 -> 12427, opens 142 -> 135, read_bytes.ipc 5269 -> 5325, read_bytes.jobs 2302 -> 2181, read_bytes.store 37674 -> 37992, read_calls.ipc 190 -> 193, read_calls.jobs 24 -> 22, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3587 -> 3590, write_bytes.store 95706 -> 95951
- acp move: cli_cpu_us 17491 -> 11217, cli_maxrss_kib 11856 -> 11920, cmd:claude 3 -> 0, cmd:codex 3 -> 0, frees 6851 -> 5796, malloc_bytes 2017544 -> 1778630, mallocs 8344 -> 7155, opens 73 -> 51, read_bytes.ipc 468 -> 388, read_bytes.jobs 4115 -> 119, read_bytes.store 1117 -> 1179, read_calls.ipc 124 -> 108, read_calls.jobs 10 -> 2, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawns 42 -> 36, write_bytes.ipc 2698 -> 2464
- acp resume: cli_cpu_us 20049 -> 16888, cli_maxrss_kib 14576 -> 15296, cmd:defaults 2 -> 0, cmd:fabric-fake-agent 1 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 8962 -> 8713, fullfsync 24 -> 23, malloc_bytes 4354099 -> 4289890, mallocs 11165 -> 10883, opens 212 -> 200, read_bytes.ipc 4938 -> 4931, read_bytes.jobs 3557 -> 3367, read_bytes.state 270 -> 122, read_bytes.store 12381 -> 12729, read_calls.ipc 172 -> 158, read_calls.jobs 28 -> 24, read_calls.state 8 -> 4, spawn:defaults 2 -> 0, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 25, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 32 -> 29, syncs.jobs 31 -> 30, write_bytes.ipc 2046 -> 2076, write_bytes.jobs 3550 -> 3558, write_bytes.store 4532 -> 4531, write_calls.ipc 28 -> 29
- acp pull: cli_cpu_us 26393 -> 17744, cli_maxrss_kib 13424 -> 14096, cmd:claude 3 -> 0, cmd:codex 3 -> 0, cmd:defaults 6 -> 4, cmd:fabric-fake-agent 2 -> 1, cmd:git symbolic-ref 3 -> 4, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 23367 -> 22019, fullfsync 81 -> 80, malloc_bytes 8714391 -> 8609365, mallocs 30018 -> 27364, opens 417 -> 389, read_bytes.ipc 11101 -> 11072, read_bytes.jobs 8544 -> 6229, read_bytes.state 322 -> 174, read_bytes.store 24504 -> 25364, read_calls.ipc 490 -> 475, read_calls.jobs 59 -> 49, read_calls.state 8 -> 4, spawn:claude 3 -> 0, spawn:codex 3 -> 0, spawn:defaults 6 -> 4, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 59, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 111 -> 102, syncs.jobs 64 -> 63, write_bytes.ipc 4625 -> 4570, write_bytes.jobs 8398 -> 8391, write_bytes.store 59981 -> 60228, write_calls.ipc 84 -> 85
- claude suspend: cli_cpu_us 18905 -> 10830, cli_maxrss_kib 11200 -> 11232, frees 13992 -> 12670, malloc_bytes 3669167 -> 3625077, mallocs 19227 -> 16379, opens 145 -> 140, read_bytes.ipc 5237 -> 5293, read_bytes.jobs 3676 -> 3555, read_bytes.state 87 -> 235, read_bytes.store 9523 -> 9794, read_calls.ipc 189 -> 191, read_calls.jobs 27 -> 25, read_calls.state 2 -> 6, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4754 -> 4757, write_bytes.store 54762 -> 54963
- claude move: cli_cpu_us 13454 -> 10774, cli_maxrss_kib 11888 -> 11872, cmd:codex 3 -> 0, frees 6759 -> 5937, malloc_bytes 2006612 -> 1811414, mallocs 8226 -> 7304, opens 73 -> 54, read_bytes.ipc 468 -> 432, read_bytes.jobs 4054 -> 82, read_bytes.store 1205 -> 1256, read_calls.ipc 124 -> 116, read_calls.jobs 10 -> 2, spawn:codex 3 -> 0, spawns 42 -> 39, write_bytes.ipc 2362 -> 2343
- claude resume: cli_cpu_us 21310 -> 17140, cli_maxrss_kib 14720 -> 14640, cmd:claude 3 -> 2, cmd:defaults 2 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding claude 0 -> 1, frees 9141 -> 8934, fullfsync 24 -> 23, malloc_bytes 4372177 -> 4309633, mallocs 11098 -> 10868, opens 248 -> 236, read_bytes.ipc 4416 -> 4411, read_bytes.jobs 8590 -> 8392, read_bytes.store 12969 -> 13248, read_calls.ipc 178 -> 163, read_calls.jobs 30 -> 26, spawn:claude 3 -> 2, spawn:defaults 2 -> 0, spawn:git 24 -> 25, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 34 -> 31, syncs.jobs 35 -> 34, write_bytes.ipc 1852 -> 1779, write_bytes.jobs 4706 -> 4698, write_bytes.store 4537 -> 4533
- claude pull: cli_cpu_us 18783 -> 24371, cli_maxrss_kib 13744 -> 13904, cmd:claude 8 -> 7, cmd:codex 3 -> 0, cmd:defaults 6 -> 4, cmd:git symbolic-ref 3 -> 4, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding claude 0 -> 1, frees 27942 -> 26511, fullfsync 83 -> 82, malloc_bytes 9185468 -> 8963648, mallocs 36336 -> 32910, opens 460 -> 435, read_bytes.ipc 10601 -> 10616, read_bytes.jobs 15733 -> 13418, read_bytes.store 25752 -> 26414, read_calls.ipc 508 -> 497, read_calls.jobs 63 -> 53, spawn:claude 8 -> 7, spawn:codex 3 -> 0, spawn:defaults 6 -> 4, spawn:git 58 -> 59, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 114 -> 108, syncs.jobs 70 -> 69, write_bytes.ipc 4279 -> 4104, write_bytes.jobs 10903 -> 10882, write_bytes.store 68401 -> 68608
- codex suspend: cli_cpu_us 13742 -> 14271, cli_maxrss_kib 10128 -> 10672, frees 11750 -> 9911, malloc_bytes 3739709 -> 3710998, mallocs 16264 -> 12631, opens 154 -> 147, read_bytes.ipc 5251 -> 5307, read_bytes.jobs 4494 -> 4373, read_bytes.store 9912 -> 10206, read_calls.ipc 193 -> 196, read_calls.jobs 28 -> 26, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5644 -> 5642, write_bytes.store 55162 -> 80089, write_calls.store 33 -> 45
- codex move: cli_cpu_us 15849 -> 12099, cli_maxrss_kib 11888 -> 11904, cmd:claude 3 -> 0, frees 6797 -> 5945, malloc_bytes 2009808 -> 1810936, mallocs 8255 -> 7306, opens 73 -> 54, read_bytes.ipc 468 -> 424, read_bytes.jobs 4086 -> 106, read_bytes.store 1245 -> 1293, read_calls.ipc 124 -> 116, read_calls.jobs 10 -> 2, spawn:claude 3 -> 0, spawns 42 -> 39, write_bytes.ipc 2362 -> 2335
- codex resume: cli_cpu_us 20675 -> 18260, cli_maxrss_kib 13488 -> 13712, cmd:codex 3 -> 2, cmd:defaults 2 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding codex 0 -> 1, frees 9299 -> 9097, fullfsync 24 -> 23, malloc_bytes 4479038 -> 4416998, mallocs 11238 -> 11016, opens 294 -> 282, read_bytes.ipc 4566 -> 4560, read_bytes.jobs 12199 -> 12000, read_bytes.store 13525 -> 13810, read_calls.ipc 180 -> 173, read_calls.jobs 29 -> 25, spawn:codex 3 -> 2, spawn:defaults 2 -> 0, spawn:git 24 -> 25, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 35 -> 32, syncs.jobs 39 -> 38, write_bytes.ipc 1786 -> 1713, write_bytes.store 4531 -> 4529
- codex pull: cli_cpu_us 24189 -> 23300, cli_maxrss_kib 13648 -> 13808, cmd:claude 3 -> 0, cmd:codex 9 -> 8, cmd:defaults 6 -> 4, cmd:git symbolic-ref 3 -> 4, cmd:ps -o lstart= 2 -> 1, cmd:ps -o pgid= 1 -> 0, cmd:sh holding codex 0 -> 1, frees 25229 -> 23323, fullfsync 82 -> 81, malloc_bytes 9338239 -> 9344963, mallocs 32692 -> 29353, opens 515 -> 490, read_bytes.ipc 10765 -> 10772, read_bytes.jobs 20113 -> 17800, read_bytes.store 26778 -> 27445, read_calls.ipc 512 -> 495, read_calls.jobs 63 -> 53, spawn:claude 3 -> 0, spawn:codex 9 -> 8, spawn:defaults 6 -> 4, spawn:git 58 -> 59, spawn:ps 3 -> 1, spawn:sh 0 -> 1, spawns 117 -> 111, syncs.jobs 76 -> 75, write_bytes.ipc 4213 -> 4030, write_bytes.jobs 12681 -> 12693, write_bytes.store 93514 -> 93729
- cold start: cli_cpu_us 85098 -> 136973, cli_maxrss_kib 10416 -> 10320, cli_processes 8 -> 12, frees 7378 -> 5328, malloc_bytes 2814234 -> 3053703, mallocs 9368 -> 7577, write_bytes.ipc 1211 -> 1935, write_calls.ipc 15 -> 23
- restart: cli_cpu_us 145504 -> 74727, cli_maxrss_kib 11008 -> 10720, cli_processes 13 -> 8, frees 15107 -> 9218, fullfsync 38 -> 12, malloc_bytes 5173361 -> 3334098, mallocs 19346 -> 12915, opens 271 -> 90, read_bytes.jobs 6522 -> 3376, read_bytes.store 453236 -> 486196, read_calls.jobs 104 -> 52, read_calls.store 112 -> 120, renames 17 -> 4, syncs.jobs 32 -> 6, write_bytes.ipc 4454 -> 4901, write_bytes.jobs 2290 -> 487, write_calls.ipc 64 -> 71, write_calls.jobs 16 -> 3

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20160 | 20752 | 20920 | 21472 | 1.038 | 1.033 to 1.042 |
| daemon b | 25632 | 27872 | 26608 | 29136 | 1.042 | 1.028 to 1.060 |
