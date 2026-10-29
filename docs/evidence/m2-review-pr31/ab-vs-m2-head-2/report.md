### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 87.6 | 89.2 | 1.018 | 0.998 to 1.040 | not shown, median above 1 |
| acp launch | 20 | 33.5 | 33.4 | 1.006 | 0.982 to 1.017 | not shown, median above 1 |
| acp approve | 20 | 9.0 | 9.1 | 1.009 | 0.997 to 1.035 | not shown, median above 1 |
| claude new | 20 | 60.4 | 60.9 | 0.998 | 0.970 to 1.006 | not shown |
| claude launch | 20 | 92.6 | 93.6 | 0.993 | 0.972 to 1.018 | not shown |
| claude approve | 20 | 10.2 | 10.3 | 1.018 | 0.982 to 1.050 | not shown, median above 1 |
| codex new | 20 | 63.2 | 62.8 | 0.987 | 0.957 to 1.025 | not shown |
| codex launch | 20 | 99.1 | 99.1 | 1.011 | 0.967 to 1.029 | not shown, median above 1 |
| codex approve | 20 | 10.0 | 9.9 | 0.983 | 0.953 to 1.015 | not shown |
| ledger append | 20 | 10.0 | 10.0 | 1.000 | 0.961 to 1.026 | not shown, median above 1 |
| cli version | 20 | 3.8 | 3.6 | 0.949 | 0.906 to 1.055 | not shown |
| cli ls | 20 | 5.8 | 5.7 | 0.998 | 0.983 to 1.028 | not shown |
| acp suspend | 20 | 313.9 | 315.1 | 1.019 | 0.995 to 1.028 | not shown, median above 1 |
| acp move | 20 | 105.4 | 105.7 | 1.006 | 0.985 to 1.019 | not shown, median above 1 |
| acp resume | 20 | 284.8 | 287.6 | 0.999 | 0.989 to 1.015 | not shown |
| acp pull | 20 | 675.0 | 677.3 | 0.998 | 0.981 to 1.023 | not shown |
| claude suspend | 20 | 313.6 | 313.0 | 1.000 | 0.988 to 1.015 | not shown, median above 1 |
| claude move | 20 | 108.7 | 110.0 | 1.004 | 0.990 to 1.013 | not shown, median above 1 |
| claude resume | 20 | 389.8 | 382.6 | 0.990 | 0.971 to 1.010 | not shown |
| claude pull | 20 | 805.4 | 789.9 | 0.982 | 0.968 to 0.995 | faster |
| codex suspend | 20 | 608.8 | 602.3 | 0.995 | 0.954 to 1.037 | not shown |
| codex move | 20 | 110.5 | 111.2 | 1.000 | 0.984 to 1.020 | not shown, median above 1 |
| codex resume | 20 | 615.4 | 629.0 | 0.995 | 0.985 to 1.019 | not shown |
| codex pull | 20 | 1314.2 | 1333.1 | 0.991 | 0.962 to 1.022 | not shown |
| cold start | 20 | 59.2 | 58.6 | 1.001 | 0.980 to 1.021 | not shown, median above 1 |
| restart | 20 | 80.4 | 78.1 | 1.001 | 0.976 to 1.019 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 37907 -> 37906 | 14544 -> 14144 | 23.9 -> 23.7 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4089 -> 4485 | 15056 -> 15008 | 15.5 -> 14.0 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 817 -> 817 | 10512 -> 10448 | 9.0 -> 9.3 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1092 -> 1091 | 10672 -> 10592 | 8.9 -> 9.4 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 784 -> 783 | 3161 -> 3167 | 14416 -> 14336 | 13.2 -> 12.0 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 816 -> 816 | 10480 -> 10448 | 8.6 -> 9.0 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1092 -> 1092 | 10672 -> 10576 | 8.5 -> 8.9 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3515 -> 3514 | 14304 -> 14208 | 12.7 -> 12.3 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 816 -> 815 | 10480 -> 10480 | 8.4 -> 10.3 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 817 -> 817 | 10464 -> 10432 | 8.4 -> 8.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 256 -> 256 | 5088 -> 5040 | 7.3 -> 7.6 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1161 -> 1163 | 10736 -> 10560 | 8.8 -> 8.9 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 41003 -> 41001 | 12316 -> 12338 | 11312 -> 11248 | 10.5 -> 9.0 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2461 -> 2459 | 4766 -> 4767 | 11904 -> 11792 | 9.6 -> 10.5 |
| acp resume | 14 -> 14 | 13 -> 13 | 61 -> 61 | 167 -> 167 | 21852 -> 21715 | 7944 -> 8160 | 15328 -> 15152 | 15.2 -> 14.1 |
| acp pull | 56 -> 56 | 41 -> 41 | 77 -> 77 | 1376 -> 1376 | 39611 -> 39585 | 21718 -> 21648 | 15408 -> 15264 | 14.1 -> 14.2 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 14164 -> 14173 | 15859 -> 15863 | 11280 -> 11200 | 9.9 -> 9.6 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2560 -> 2574 | 4972 -> 4969 | 11888 -> 11760 | 10.2 -> 10.0 |
| claude resume | 16 -> 16 | 13 -> 13 | 65 -> 65 | 148 -> 148 | 27031 -> 27033 | 8208 -> 8203 | 15200 -> 15136 | 14.0 -> 15.8 |
| claude pull | 61 -> 61 | 41 -> 41 | 83 -> 83 | 1338 -> 1338 | 47590 -> 47636 | 27079 -> 27094 | 15312 -> 15296 | 14.9 -> 15.4 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 15304 -> 15305 | 12068 -> 12049 | 11280 -> 11216 | 9.4 -> 9.0 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2642 -> 2636 | 4988 -> 4987 | 11904 -> 11760 | 11.6 -> 10.8 |
| codex resume | 17 -> 17 | 13 -> 13 | 69 -> 69 | 148 -> 148 | 31031 -> 31020 | 8325 -> 8331 | 15216 -> 15152 | 15.1 -> 13.7 |
| codex pull | 64 -> 64 | 40 -> 40 | 88 -> 88 | 1338 -> 1338 | 52877 -> 52711 | 22586 -> 22586 | 15280 -> 15216 | 14.7 -> 15.2 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 4370 -> 4371 | 10384 -> 10288 | 40.2 -> 33.3 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 473759 -> 506719 | 9468 -> 9460 | 10928 -> 10752 | 37.7 -> 35.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 23892 -> 23662, cli_maxrss_kib 14544 -> 14144, frees 27245 -> 27244, malloc_bytes 13340116 -> 13339783, mallocs 37907 -> 37906
- acp launch: cli_cpu_us 15520 -> 14033, cli_maxrss_kib 15056 -> 15008, frees 3339 -> 3369, malloc_bytes 685533 -> 787425, mallocs 4089 -> 4485
- acp approve: cli_cpu_us 8999 -> 9262, cli_maxrss_kib 10512 -> 10448, frees 684 -> 683, malloc_bytes 262080 -> 262355
- claude new: cli_cpu_us 8949 -> 9351, cli_maxrss_kib 10672 -> 10592, malloc_bytes 301852 -> 301851, mallocs 1092 -> 1091
- claude launch: cli_cpu_us 13201 -> 11961, cli_maxrss_kib 14416 -> 14336, malloc_bytes 527187 -> 527629, mallocs 3161 -> 3167, read_bytes.jobs 610 -> 609, write_bytes.jobs 928 -> 927
- claude approve: cli_cpu_us 8562 -> 9028, cli_maxrss_kib 10480 -> 10448, malloc_bytes 261888 -> 261827
- codex new: cli_cpu_us 8520 -> 8947, cli_maxrss_kib 10672 -> 10576, malloc_bytes 301856 -> 301859
- codex launch: cli_cpu_us 12711 -> 12283, cli_maxrss_kib 14304 -> 14208, frees 3129 -> 3128, malloc_bytes 618559 -> 618498, mallocs 3515 -> 3514
- codex approve: cli_cpu_us 8407 -> 10340, frees 684 -> 683, malloc_bytes 261824 -> 261763, mallocs 816 -> 815
- ledger append: cli_cpu_us 8397 -> 8524, cli_maxrss_kib 10464 -> 10432, malloc_bytes 262080 -> 262083
- cli version: cli_cpu_us 7285 -> 7600, cli_maxrss_kib 5088 -> 5040, malloc_bytes 106771 -> 106774
- cli ls: cli_cpu_us 8838 -> 8943, cli_maxrss_kib 10736 -> 10560, frees 987 -> 989, malloc_bytes 307332 -> 307463, mallocs 1161 -> 1163
- acp suspend: cli_cpu_us 10537 -> 8979, cli_maxrss_kib 11312 -> 11248, frees 9276 -> 9285, malloc_bytes 3435407 -> 3453744, mallocs 12316 -> 12338, read_bytes.store 38735 -> 38733, write_bytes.jobs 3584 -> 3588
- acp move: cli_cpu_us 9590 -> 10499, cli_maxrss_kib 11904 -> 11792, frees 3796 -> 3795, malloc_bytes 1622884 -> 1623496, mallocs 4766 -> 4767, read_bytes.store 2342 -> 2340
- acp resume: cli_cpu_us 15226 -> 14072, cli_maxrss_kib 15328 -> 15152, frees 6515 -> 6524, malloc_bytes 3487785 -> 3574126, mallocs 7944 -> 8160, read_bytes.jobs 3363 -> 3366, read_bytes.state 270 -> 122, read_bytes.store 18219 -> 18227, read_calls.state 8 -> 4, write_bytes.jobs 3548 -> 3560, write_bytes.store 2477 -> 2478
- acp pull: cli_cpu_us 14101 -> 14187, cli_maxrss_kib 15408 -> 15264, frees 17361 -> 17316, malloc_bytes 7447346 -> 7437874, mallocs 21718 -> 21648, opens 368 -> 366, read_bytes.jobs 6231 -> 6233, read_bytes.store 33206 -> 33178, write_bytes.jobs 8386 -> 8403, write_bytes.store 57320 -> 57310
- claude suspend: cli_cpu_us 9881 -> 9625, cli_maxrss_kib 11280 -> 11200, frees 12602 -> 12600, malloc_bytes 3519301 -> 3519540, mallocs 15859 -> 15863, read_bytes.store 10522 -> 10531, write_bytes.jobs 4771 -> 4755, write_bytes.store 54941 -> 63197, write_calls.store 33 -> 37
- claude move: cli_cpu_us 10201 -> 9955, cli_maxrss_kib 11888 -> 11760, frees 3971 -> 3969, malloc_bytes 1657598 -> 1657509, mallocs 4972 -> 4969, read_bytes.store 2478 -> 2492
- claude resume: cli_cpu_us 13960 -> 15833, cli_maxrss_kib 15200 -> 15136, frees 6760 -> 6748, malloc_bytes 3594161 -> 3593272, mallocs 8208 -> 8203, opens 254 -> 252, read_bytes.jobs 8394 -> 8388, read_bytes.store 18515 -> 18523, write_bytes.store 2478 -> 2479
- claude pull: cli_cpu_us 14876 -> 15365, cli_maxrss_kib 15312 -> 15296, frees 21694 -> 21695, malloc_bytes 7788368 -> 7789432, mallocs 27079 -> 27094, read_bytes.ipc 7227 -> 7226, read_bytes.store 33994 -> 34040, write_bytes.jobs 10888 -> 10912, write_bytes.store 57465 -> 65709, write_calls.store 37 -> 41
- codex suspend: cli_cpu_us 9444 -> 9046, cli_maxrss_kib 11280 -> 11216, frees 9818 -> 9806, malloc_bytes 3602335 -> 3584379, mallocs 12068 -> 12049, read_bytes.store 10844 -> 10845, write_bytes.jobs 5649 -> 5641, write_bytes.store 63547 -> 63543
- codex move: cli_cpu_us 11643 -> 10769, cli_maxrss_kib 11904 -> 11760, frees 3985 -> 3986, malloc_bytes 1657988 -> 1657655, mallocs 4988 -> 4987, read_bytes.store 2536 -> 2530
- codex resume: cli_cpu_us 15140 -> 13650, cli_maxrss_kib 15216 -> 15152, malloc_bytes 3699738 -> 3700123, mallocs 8325 -> 8331, read_bytes.jobs 12006 -> 12000, read_bytes.store 18903 -> 18898, write_bytes.jobs 5644 -> 5635, write_bytes.store 2477 -> 2478
- codex pull: cli_cpu_us 14712 -> 15209, cli_maxrss_kib 15280 -> 15216, frees 18435 -> 18434, malloc_bytes 7937911 -> 7939231, opens 460 -> 458, read_bytes.jobs 17809 -> 17805, read_bytes.state 322 -> 174, read_bytes.store 34746 -> 34732, read_calls.state 8 -> 4, write_bytes.jobs 12684 -> 12679, write_bytes.store 82547 -> 90783, write_calls.store 49 -> 53
- cold start: cli_cpu_us 40172 -> 33302, cli_maxrss_kib 10384 -> 10288, malloc_bytes 1432354 -> 1432433, mallocs 4370 -> 4371
- restart: cli_cpu_us 37733 -> 35012, cli_maxrss_kib 10928 -> 10752, frees 6361 -> 6352, malloc_bytes 2262536 -> 2262044, mallocs 9468 -> 9460, read_bytes.store 453236 -> 486196, read_calls.store 112 -> 120

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 25200 | 28608 | 25944 | 28624 | 0.992 | 0.988 to 0.995 |
| b | 78 | 25928 | 28144 | 26424 | 27712 | 1.005 | 0.997 to 1.015 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 78 | 6968 | 9216 | 6888 | 9232 | 0.993 | 0.987 to 1.006 |
| b | 78 | 8736 | 9024 | 8336 | 8528 | 0.959 | 0.943 to 0.966 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 21096 | 21376 | 20976 | 21360 | 0.997 | 0.995 to 1.000 |
| daemon b | 28624 | 29520 | 28696 | 29584 | 1.005 | 0.999 to 1.008 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5136 | 5440 | 5040 | 5440 | 0.992 | 0.983 to 1.002 |
| daemon b | 9536 | 10496 | 9632 | 10512 | 1.002 | 0.997 to 1.008 |
