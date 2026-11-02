### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 97.9 | 100.9 | 1.004 | 0.984 to 1.035 | not shown, median above 1 |
| acp launch | 20 | 37.3 | 36.2 | 0.986 | 0.946 to 1.033 | not shown |
| acp approve | 20 | 10.2 | 10.0 | 0.992 | 0.973 to 1.058 | not shown |
| claude new | 20 | 66.5 | 65.6 | 1.002 | 0.969 to 1.018 | not shown, median above 1 |
| claude launch | 20 | 95.6 | 96.1 | 0.998 | 0.970 to 1.033 | not shown |
| claude approve | 20 | 10.2 | 10.3 | 1.006 | 0.957 to 1.025 | not shown, median above 1 |
| codex new | 20 | 70.6 | 66.0 | 0.962 | 0.907 to 0.996 | faster |
| codex launch | 20 | 102.6 | 103.0 | 1.016 | 0.992 to 1.046 | not shown, median above 1 |
| codex approve | 20 | 9.9 | 10.1 | 0.996 | 0.961 to 1.019 | not shown |
| ledger append | 20 | 10.0 | 10.0 | 0.999 | 0.952 to 1.028 | not shown |
| cli version | 20 | 4.0 | 3.9 | 0.982 | 0.941 to 1.023 | not shown |
| cli ls | 20 | 6.3 | 6.1 | 0.991 | 0.933 to 1.019 | not shown |
| acp suspend | 20 | 353.5 | 343.0 | 0.996 | 0.960 to 1.018 | not shown |
| acp move | 20 | 109.9 | 110.8 | 1.016 | 1.001 to 1.028 | slower |
| acp resume | 20 | 301.8 | 304.6 | 1.007 | 0.988 to 1.073 | not shown, median above 1 |
| acp pull | 20 | 734.1 | 734.3 | 1.021 | 0.992 to 1.038 | not shown, median above 1 |
| claude suspend | 20 | 350.9 | 351.7 | 1.010 | 0.975 to 1.024 | not shown, median above 1 |
| claude move | 20 | 112.8 | 111.6 | 0.999 | 0.991 to 1.007 | not shown |
| claude resume | 20 | 408.0 | 396.2 | 1.011 | 0.982 to 1.024 | not shown, median above 1 |
| claude pull | 20 | 851.9 | 840.6 | 0.984 | 0.952 to 1.007 | not shown |
| codex suspend | 20 | 620.3 | 600.9 | 0.994 | 0.980 to 1.042 | not shown |
| codex move | 20 | 112.6 | 113.3 | 0.996 | 0.988 to 1.027 | not shown |
| codex resume | 20 | 617.5 | 609.7 | 0.998 | 0.960 to 1.012 | not shown |
| codex pull | 20 | 1333.8 | 1328.6 | 1.003 | 0.977 to 1.026 | not shown, median above 1 |
| cold start | 20 | 60.9 | 62.9 | 1.040 | 0.993 to 1.073 | not shown, median above 1 |
| restart | 20 | 98.5 | 100.8 | 0.984 | 0.935 to 1.026 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 37905 -> 37906 | 14512 -> 14784 | 24.0 -> 23.1 |
| acp launch | 2 -> 2 | 5 -> 5 | 0 -> 0 | 135 -> 135 | 295 -> 295 | 4094 -> 4084 | 15056 -> 15104 | 11.7 -> 12.0 |
| acp approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 818 -> 817 | 10480 -> 10480 | 8.1 -> 8.2 |
| claude new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1093 -> 1092 | 10672 -> 10640 | 9.0 -> 8.1 |
| claude launch | 2 -> 2 | 7 -> 7 | 0 -> 0 | 116 -> 116 | 790 -> 790 | 3172 -> 3169 | 14400 -> 14416 | 12.1 -> 11.3 |
| claude approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 816 -> 815 | 10480 -> 10432 | 8.0 -> 10.6 |
| codex new | 1 -> 1 | 3 -> 3 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1092 -> 1092 | 10688 -> 10608 | 8.0 -> 8.2 |
| codex launch | 2 -> 2 | 8 -> 8 | 0 -> 0 | 116 -> 116 | 297 -> 297 | 3508 -> 3517 | 14288 -> 14288 | 12.9 -> 11.2 |
| codex approve | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 148 -> 0 | 820 -> 816 | 10480 -> 10448 | 8.3 -> 8.6 |
| ledger append | 0 -> 0 | 1 -> 1 | 0 -> 0 | 98 -> 98 | 0 -> 148 | 817 -> 820 | 10464 -> 10464 | 8.0 -> 8.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 256 -> 256 | 5088 -> 5104 | 7.2 -> 6.6 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1067 -> 1066 | 10736 -> 10672 | 8.3 -> 8.3 |
| acp suspend | 35 -> 35 | 20 -> 20 | 14 -> 14 | 522 -> 522 | 24622 -> 24589 | 12274 -> 12273 | 11312 -> 11264 | 9.1 -> 8.8 |
| acp move | 8 -> 8 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2467 -> 2449 | 4766 -> 4766 | 11920 -> 11872 | 9.8 -> 10.0 |
| acp resume | 14 -> 14 | 13 -> 13 | 55 -> 55 | 167 -> 167 | 16934 -> 16904 | 8161 -> 7755 | 15296 -> 15312 | 16.1 -> 12.7 |
| acp pull | 56 -> 56 | 41 -> 41 | 71 -> 71 | 1376 -> 1376 | 34860 -> 34924 | 21155 -> 21550 | 15408 -> 15440 | 14.5 -> 13.8 |
| claude suspend | 34 -> 34 | 20 -> 20 | 15 -> 15 | 522 -> 522 | 13612 -> 13618 | 14114 -> 14116 | 11296 -> 11216 | 9.4 -> 9.1 |
| claude move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2578 -> 2572 | 4973 -> 4970 | 11904 -> 11888 | 9.3 -> 9.4 |
| claude resume | 16 -> 16 | 13 -> 13 | 59 -> 59 | 148 -> 148 | 20006 -> 19994 | 8027 -> 8025 | 15312 -> 15232 | 13.8 -> 16.4 |
| claude pull | 61 -> 61 | 41 -> 41 | 77 -> 77 | 1338 -> 1338 | 40043 -> 40165 | 25205 -> 25193 | 15312 -> 15392 | 14.6 -> 14.2 |
| codex suspend | 36 -> 36 | 19 -> 19 | 16 -> 16 | 522 -> 522 | 14517 -> 14509 | 11943 -> 11926 | 11312 -> 11216 | 13.3 -> 9.7 |
| codex move | 11 -> 11 | 7 -> 7 | 0 -> 0 | 520 -> 520 | 2656 -> 2644 | 4987 -> 4987 | 11936 -> 11888 | 10.1 -> 13.2 |
| codex resume | 17 -> 17 | 13 -> 13 | 63 -> 63 | 148 -> 148 | 23084 -> 23231 | 8178 -> 8182 | 15200 -> 15280 | 14.1 -> 14.4 |
| codex pull | 64 -> 64 | 40 -> 40 | 82 -> 82 | 1338 -> 1338 | 43998 -> 44019 | 22335 -> 22361 | 15328 -> 15440 | 14.5 -> 14.4 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5350 -> 4370 | 10384 -> 10352 | 57.0 -> 37.5 |
| restart | 0 -> 0 | 6 -> 6 | 0 -> 0 | 463 -> 463 | 270043 -> 282403 | 8991 -> 8998 | 10912 -> 10848 | 45.2 -> 45.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 24026 -> 23065, cli_maxrss_kib 14512 -> 14784, malloc_bytes 13339716 -> 13340055, mallocs 37905 -> 37906
- acp launch: cli_cpu_us 11676 -> 11980, cli_maxrss_kib 15056 -> 15104, frees 3350 -> 3339, malloc_bytes 685896 -> 685152, mallocs 4094 -> 4084, read_calls.ipc 6 -> 7
- acp approve: cli_cpu_us 8073 -> 8224, malloc_bytes 262416 -> 262083, mallocs 818 -> 817
- claude new: cli_cpu_us 8965 -> 8146, cli_maxrss_kib 10672 -> 10640, frees 880 -> 879, malloc_bytes 301976 -> 301915, mallocs 1093 -> 1092
- claude launch: cli_cpu_us 12058 -> 11274, cli_maxrss_kib 14400 -> 14416, frees 2804 -> 2802, malloc_bytes 528165 -> 527764, mallocs 3172 -> 3169
- claude approve: cli_cpu_us 8013 -> 10629, cli_maxrss_kib 10480 -> 10432, frees 684 -> 683, malloc_bytes 261824 -> 261763, mallocs 816 -> 815
- codex new: cli_cpu_us 8044 -> 8230, cli_maxrss_kib 10688 -> 10608, malloc_bytes 301856 -> 301859
- codex launch: cli_cpu_us 12867 -> 11250, malloc_bytes 618051 -> 618630, mallocs 3508 -> 3517
- codex approve: cli_cpu_us 8263 -> 8597, cli_maxrss_kib 10480 -> 10448, frees 688 -> 684, malloc_bytes 262484 -> 261827, mallocs 820 -> 816, opens 3 -> 1, read_bytes.state 148 -> 0, read_calls.state 4 -> 0
- ledger append: cli_cpu_us 8040 -> 7970, frees 684 -> 687, malloc_bytes 262080 -> 262679, mallocs 817 -> 820, opens 1 -> 3, read_bytes.state 0 -> 148, read_calls.state 0 -> 4
- cli version: cli_cpu_us 7189 -> 6558, cli_maxrss_kib 5088 -> 5104, malloc_bytes 106771 -> 106774
- cli ls: cli_cpu_us 8257 -> 8329, cli_maxrss_kib 10736 -> 10672, frees 905 -> 904, malloc_bytes 297812 -> 297751, mallocs 1067 -> 1066
- acp suspend: cli_cpu_us 9145 -> 8819, cli_maxrss_kib 11312 -> 11264, frees 9234 -> 9235, malloc_bytes 3433860 -> 3433611, mallocs 12274 -> 12273, read_bytes.store 22354 -> 22321, write_bytes.jobs 3589 -> 3582, write_bytes.store 79461 -> 79433
- acp move: cli_cpu_us 9792 -> 9991, cli_maxrss_kib 11920 -> 11872, malloc_bytes 1623160 -> 1623159, read_bytes.store 2348 -> 2330
- acp resume: cli_cpu_us 16070 -> 12712, cli_maxrss_kib 15296 -> 15312, frees 6492 -> 6451, malloc_bytes 3573817 -> 3471370, mallocs 8161 -> 7755, read_bytes.jobs 3364 -> 3362, read_bytes.store 13448 -> 13420, write_bytes.jobs 3547 -> 3550, write_bytes.store 2478 -> 2480
- acp pull: cli_cpu_us 14526 -> 13826, cli_maxrss_kib 15408 -> 15440, frees 17214 -> 17247, malloc_bytes 7331044 -> 7432423, mallocs 21155 -> 21550, opens 348 -> 350, read_bytes.jobs 6228 -> 6230, read_bytes.state 174 -> 322, read_bytes.store 28458 -> 28372, read_calls.state 4 -> 8, write_bytes.jobs 8387 -> 8397, write_bytes.store 49090 -> 49074
- claude suspend: cli_cpu_us 9410 -> 9104, cli_maxrss_kib 11296 -> 11216, frees 11270 -> 11267, malloc_bytes 3422738 -> 3422783, mallocs 14114 -> 14116, read_bytes.store 10531 -> 10537, write_bytes.jobs 4187 -> 4199, write_bytes.store 46703 -> 46693
- claude move: cli_cpu_us 9293 -> 9361, cli_maxrss_kib 11904 -> 11888, frees 3972 -> 3970, malloc_bytes 1657308 -> 1657573, mallocs 4973 -> 4970, read_bytes.store 2496 -> 2490
- claude resume: cli_cpu_us 13802 -> 16368, cli_maxrss_kib 15312 -> 15232, frees 6610 -> 6609, malloc_bytes 3579915 -> 3579596, mallocs 8027 -> 8025, read_bytes.jobs 6160 -> 6154, read_bytes.store 13724 -> 13718, write_bytes.jobs 4144 -> 4132
- claude pull: cli_cpu_us 14629 -> 14187, cli_maxrss_kib 15312 -> 15392, frees 20256 -> 20251, malloc_bytes 7722729 -> 7721816, mallocs 25205 -> 25193, opens 389 -> 391, read_bytes.jobs 10632 -> 10623, read_bytes.state 174 -> 322, read_bytes.store 29237 -> 29220, read_calls.state 4 -> 8, write_bytes.jobs 9790 -> 9777
- codex suspend: cli_cpu_us 13346 -> 9717, cli_maxrss_kib 11312 -> 11216, frees 9704 -> 9691, malloc_bytes 3504934 -> 3503855, mallocs 11943 -> 11926, read_bytes.store 10849 -> 10841, read_calls.ipc 179 -> 180, write_bytes.jobs 4839 -> 4851, write_bytes.store 47077 -> 55313, write_calls.store 29 -> 33
- codex move: cli_cpu_us 10141 -> 13165, cli_maxrss_kib 11936 -> 11888, malloc_bytes 1657652 -> 1657655, read_bytes.store 2550 -> 2538
- codex resume: cli_cpu_us 14145 -> 14440, cli_maxrss_kib 15200 -> 15280, frees 6775 -> 6778, malloc_bytes 3687483 -> 3688149, mallocs 8178 -> 8182, opens 278 -> 280, read_bytes.jobs 8838 -> 8839, read_bytes.state 122 -> 270, read_bytes.store 14124 -> 14122, read_calls.ipc 76 -> 77, read_calls.state 4 -> 8, write_bytes.jobs 4849 -> 4832, write_bytes.store 2478 -> 2480
- codex pull: cli_cpu_us 14519 -> 14414, cli_maxrss_kib 15328 -> 15440, frees 18219 -> 18230, malloc_bytes 7859298 -> 7862494, mallocs 22335 -> 22361, read_bytes.jobs 13842 -> 13849, read_bytes.store 29982 -> 29996, write_bytes.jobs 11095 -> 11104, write_bytes.store 49580 -> 66085, write_calls.store 33 -> 41
- cold start: cli_cpu_us 57037 -> 37511, cli_maxrss_kib 10384 -> 10352, cli_processes 6 -> 4, frees 3760 -> 2992, malloc_bytes 1893722 -> 1432369, mallocs 5350 -> 4370, write_bytes.ipc 849 -> 487, write_calls.ipc 11 -> 7
- restart: cli_cpu_us 45173 -> 45660, cli_maxrss_kib 10912 -> 10848, frees 6169 -> 6177, malloc_bytes 2386822 -> 2387340, mallocs 8991 -> 8998, read_bytes.store 255476 -> 267836, read_calls.store 64 -> 67

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 26480 | 28768 | 26064 | 28448 | 0.992 | 0.986 to 0.995 |
| b | 26 | 26184 | 27264 | 26888 | 27632 | 1.020 | 1.017 to 1.028 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 26 | 7056 | 8928 | 6872 | 8784 | 0.978 | 0.976 to 0.982 |
| b | 26 | 8080 | 8416 | 8256 | 8576 | 1.024 | 1.018 to 1.026 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 21168 | 21504 | 21128 | 21456 | 0.997 | 0.995 to 1.000 |
| daemon b | 28152 | 29648 | 28296 | 29584 | 1.001 | 0.996 to 1.003 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5224 | 5584 | 5176 | 5536 | 0.990 | 0.977 to 1.002 |
| daemon b | 9440 | 10336 | 9784 | 10304 | 1.032 | 1.023 to 1.036 |
