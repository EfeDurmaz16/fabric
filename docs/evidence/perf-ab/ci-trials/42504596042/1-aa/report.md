### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 228.7 | 228.9 | 1.014 | 0.864 to 1.289 | not shown, median above 1 |
| acp launch | 15 | 34.2 | 34.2 | 1.014 | 0.897 to 1.035 | not shown, median above 1 |
| acp approve | 15 | 7.4 | 7.4 | 1.033 | 0.979 to 1.070 | not shown, median above 1 |
| claude new | 15 | 222.7 | 231.0 | 0.929 | 0.822 to 1.012 | not shown |
| claude launch | 15 | 211.9 | 199.9 | 1.019 | 0.628 to 1.334 | not shown, median above 1 |
| claude approve | 15 | 9.3 | 9.5 | 1.025 | 0.832 to 1.140 | not shown, median above 1 |
| codex new | 15 | 268.0 | 246.0 | 0.994 | 0.809 to 1.047 | not shown |
| codex launch | 15 | 224.7 | 211.8 | 0.914 | 0.746 to 1.188 | not shown |
| codex approve | 15 | 7.6 | 7.6 | 1.008 | 0.967 to 1.125 | not shown, median above 1 |
| ledger append | 15 | 7.8 | 7.6 | 0.975 | 0.913 to 1.023 | not shown |
| cli version | 15 | 3.3 | 3.3 | 0.943 | 0.881 to 0.955 | faster |
| cli ls | 15 | 4.3 | 4.4 | 1.039 | 0.997 to 1.066 | not shown, median above 1 |
| acp suspend | 15 | 499.4 | 481.4 | 0.929 | 0.892 to 1.025 | not shown |
| acp move | 15 | 299.7 | 319.5 | 1.062 | 0.929 to 1.173 | not shown, median above 1 |
| acp resume | 15 | 874.7 | 925.3 | 0.920 | 0.900 to 1.035 | not shown |
| acp pull | 15 | 1849.0 | 1824.9 | 0.957 | 0.900 to 1.048 | not shown |
| claude suspend | 15 | 679.1 | 635.0 | 0.969 | 0.857 to 1.049 | not shown |
| claude move | 15 | 329.6 | 319.1 | 1.026 | 0.886 to 1.128 | not shown, median above 1 |
| claude resume | 15 | 1471.5 | 1420.4 | 1.057 | 0.995 to 1.107 | not shown, median above 1 |
| claude pull | 15 | 2337.0 | 2405.5 | 0.981 | 0.944 to 1.060 | not shown |
| codex suspend | 15 | 743.4 | 716.9 | 0.879 | 0.851 to 1.041 | not shown |
| codex move | 15 | 318.9 | 326.9 | 1.022 | 0.897 to 1.138 | not shown, median above 1 |
| codex resume | 15 | 1320.1 | 1418.1 | 0.952 | 0.890 to 1.108 | not shown |
| codex pull | 15 | 2468.2 | 2497.0 | 1.046 | 1.007 to 1.089 | slower |
| cold start | 15 | 88.2 | 73.5 | 0.802 | 0.650 to 1.065 | not shown |
| restart | 15 | 177.3 | 175.5 | 1.039 | 0.957 to 1.119 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38479 -> 38478 | 7120 -> 7024 | 4065 -> 3985 | 9.8 -> 11.0 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1186 -> 1189 | 3657 -> 3656 | 6976 -> 7024 | 3361 -> 3409 | 10.6 -> 9.7 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 733 -> 733 | 5248 -> 5184 | 3137 -> 3073 | 8.5 -> 7.0 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1205 | 5392 -> 5344 | 3217 -> 3137 | 8.8 -> 8.1 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1102 | 3049 -> 3053 | 7056 -> 7040 | 3441 -> 3425 | 10.5 -> 10.3 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5152 -> 5184 | 3025 -> 3073 | 9.0 -> 8.1 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5360 -> 5296 | 3169 -> 3121 | 9.5 -> 8.6 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3371 -> 3365 | 7104 -> 7104 | 3377 -> 3393 | 9.8 -> 9.2 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5120 | 3073 -> 3009 | 7.2 -> 7.5 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 733 -> 732 | 5200 -> 5184 | 3073 -> 3041 | 9.2 -> 7.3 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2625 -> 2641 | 6.5 -> 7.4 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5264 -> 5248 | 3121 -> 3105 | 8.0 -> 7.0 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48266 -> 48233 | 14495 -> 14492 | 5616 -> 5616 | 3297 -> 3313 | 8.0 -> 8.2 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5216 -> 5215 | 7939 -> 7930 | 5808 -> 5792 | 3361 -> 3361 | 9.3 -> 8.4 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19832 -> 19796 | 11225 -> 11219 | 7072 -> 7104 | 3425 -> 3441 | 11.1 -> 10.1 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37013 -> 36875 | 30352 -> 30762 | 7168 -> 7152 | 3473 -> 3457 | 11.3 -> 9.3 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13822 -> 13843 | 20430 -> 20472 | 5536 -> 5600 | 3249 -> 3313 | 7.1 -> 7.6 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5438 -> 5283 | 7851 -> 7855 | 5808 -> 5776 | 3361 -> 3329 | 8.7 -> 7.8 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27846 -> 27872 | 11664 -> 11661 | 7392 -> 7392 | 3473 -> 3473 | 10.6 -> 14.0 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48451 -> 48546 | 38521 -> 38496 | 7456 -> 7472 | 3505 -> 3521 | 12.3 -> 11.5 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15188 -> 15199 | 15920 -> 15946 | 5584 -> 5536 | 3265 -> 3249 | 8.0 -> 9.5 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5317 -> 5331 | 7863 -> 7872 | 5760 -> 5776 | 3313 -> 3329 | 9.7 -> 8.7 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32899 -> 33065 | 11757 -> 11753 | 7360 -> 7392 | 3457 -> 3505 | 13.7 -> 12.7 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54906 -> 54922 | 33137 -> 33137 | 7488 -> 7440 | 3569 -> 3521 | 14.0 -> 11.2 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 3264 -> 3261 | 5184 -> 5200 | 3057 -> 3073 | 25.6 -> 24.2 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 780227 -> 817307 | 10186 -> 9064 | 5296 -> 5296 | 3153 -> 3153 | 59.3 -> 47.6 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 9808 -> 10971, cli_footprint_kib 4065 -> 3985, cli_maxrss_kib 7120 -> 7024, frees 27688 -> 27687, malloc_bytes 13401480 -> 13401147, mallocs 38479 -> 38478
- acp launch: cli_cpu_us 10582 -> 9739, cli_footprint_kib 3361 -> 3409, cli_maxrss_kib 6976 -> 7024, frees 2931 -> 2930, malloc_bytes 635321 -> 635260, mallocs 3657 -> 3656, read_bytes.jobs 1012 -> 1015, write_bytes.jobs 1034 -> 1037
- acp approve: cli_cpu_us 8478 -> 7045, cli_footprint_kib 3137 -> 3073, cli_maxrss_kib 5248 -> 5184, malloc_bytes 242304 -> 242307
- claude new: cli_cpu_us 8837 -> 8056, cli_footprint_kib 3217 -> 3137, cli_maxrss_kib 5392 -> 5344, malloc_bytes 308809 -> 309148, mallocs 1204 -> 1205
- claude launch: cli_cpu_us 10499 -> 10336, cli_footprint_kib 3441 -> 3425, cli_maxrss_kib 7056 -> 7040, malloc_bytes 521588 -> 521664, mallocs 3049 -> 3053, read_bytes.jobs 927 -> 928, write_bytes.jobs 930 -> 931
- claude approve: cli_cpu_us 9026 -> 8086, cli_footprint_kib 3025 -> 3073, cli_maxrss_kib 5152 -> 5184, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 9452 -> 8565, cli_footprint_kib 3169 -> 3121, cli_maxrss_kib 5360 -> 5296, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 9819 -> 9171, cli_footprint_kib 3377 -> 3393, malloc_bytes 613276 -> 612959, mallocs 3371 -> 3365
- codex approve: cli_cpu_us 7167 -> 7539, cli_footprint_kib 3073 -> 3009, cli_maxrss_kib 5200 -> 5120, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 9185 -> 7300, cli_footprint_kib 3073 -> 3041, cli_maxrss_kib 5200 -> 5184, malloc_bytes 242304 -> 242243, mallocs 733 -> 732
- cli version: cli_cpu_us 6486 -> 7425, cli_footprint_kib 2625 -> 2641, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 8018 -> 7040, cli_footprint_kib 3121 -> 3105, cli_maxrss_kib 5264 -> 5248, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 8035 -> 8234, cli_footprint_kib 3297 -> 3313, frees 9878 -> 9873, malloc_bytes 3481170 -> 3463812, mallocs 14495 -> 14492, read_bytes.ipc 5213 -> 5214, read_bytes.store 45877 -> 45844, write_bytes.jobs 3584 -> 3582, write_bytes.store 112178 -> 112170
- acp move: cli_cpu_us 9338 -> 8358, cli_maxrss_kib 5808 -> 5792, frees 6504 -> 6492, malloc_bytes 1976222 -> 1975487, mallocs 7939 -> 7930, read_bytes.store 1113 -> 1112, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 11078 -> 10134, cli_footprint_kib 3425 -> 3441, cli_maxrss_kib 7072 -> 7104, frees 9219 -> 9213, malloc_bytes 4659977 -> 4659339, mallocs 11225 -> 11219, read_bytes.ipc 5867 -> 5866, read_bytes.store 16150 -> 16114, read_calls.ipc 192 -> 201, write_bytes.jobs 3553 -> 3554, write_bytes.store 6151 -> 6154
- acp pull: cli_cpu_us 11266 -> 9269, cli_footprint_kib 3473 -> 3457, cli_maxrss_kib 7168 -> 7152, frees 23563 -> 23586, malloc_bytes 9090699 -> 9195396, mallocs 30352 -> 30762, opens 425 -> 427, read_bytes.ipc 11993 -> 11994, read_bytes.jobs 8552 -> 8534, read_bytes.store 28287 -> 28167, read_calls.ipc 522 -> 530, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8413 -> 8385, write_bytes.store 87925 -> 94540, write_calls.store 55 -> 57
- claude suspend: cli_cpu_us 7130 -> 7633, cli_footprint_kib 3249 -> 3313, cli_maxrss_kib 5536 -> 5600, frees 14810 -> 14804, malloc_bytes 3694986 -> 3699951, mallocs 20430 -> 20472, read_bytes.store 9501 -> 9522, read_calls.ipc 185 -> 184, write_bytes.jobs 5312 -> 5315, write_bytes.store 62982 -> 87724, write_calls.store 37 -> 49
- claude move: cli_cpu_us 8742 -> 7837, cli_footprint_kib 3361 -> 3329, cli_maxrss_kib 5808 -> 5776, frees 6418 -> 6424, malloc_bytes 1966741 -> 1966803, mallocs 7851 -> 7855, opens 75 -> 73, read_bytes.jobs 4086 -> 4074, read_bytes.state 148 -> 0, read_bytes.store 1204 -> 1209, read_calls.state 4 -> 0, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 10571 -> 14021, frees 9520 -> 9517, malloc_bytes 4794658 -> 4794034, mallocs 11664 -> 11661, opens 260 -> 258, read_bytes.jobs 10819 -> 10817, read_bytes.store 16905 -> 16933, read_calls.ipc 204 -> 211, write_bytes.jobs 5253 -> 5268, write_bytes.store 6149 -> 6152
- claude pull: cli_cpu_us 12297 -> 11519, cli_footprint_kib 3505 -> 3521, cli_maxrss_kib 7456 -> 7472, frees 29542 -> 29529, malloc_bytes 9681856 -> 9680350, mallocs 38521 -> 38496, opens 470 -> 468, read_bytes.jobs 18505 -> 18534, read_bytes.store 29624 -> 29690, read_calls.ipc 533 -> 534, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 11993 -> 12034, write_bytes.store 102950 -> 111201, write_calls.store 61 -> 65
- codex suspend: cli_cpu_us 7983 -> 9459, cli_footprint_kib 3265 -> 3249, cli_maxrss_kib 5584 -> 5536, frees 11367 -> 11316, malloc_bytes 3676885 -> 3719505, mallocs 15920 -> 15946, read_bytes.store 9816 -> 9827, write_bytes.jobs 6447 -> 6434, write_bytes.store 88074 -> 112794, write_calls.store 49 -> 61
- codex move: cli_cpu_us 9657 -> 8671, cli_footprint_kib 3313 -> 3329, cli_maxrss_kib 5760 -> 5776, frees 6433 -> 6445, malloc_bytes 1967022 -> 1967164, mallocs 7863 -> 7872, read_bytes.jobs 4086 -> 4098, read_bytes.store 1231 -> 1233, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 13748 -> 12652, cli_footprint_kib 3457 -> 3505, cli_maxrss_kib 7360 -> 7392, frees 9648 -> 9650, malloc_bytes 4896532 -> 4896679, mallocs 11757 -> 11753, opens 302 -> 304, read_bytes.jobs 15357 -> 15356, read_bytes.state 122 -> 270, read_bytes.store 17420 -> 17439, read_calls.state 4 -> 8, write_bytes.jobs 6426 -> 6422, write_bytes.store 6154 -> 6151
- codex pull: cli_cpu_us 13977 -> 11214, cli_footprint_kib 3569 -> 3521, cli_maxrss_kib 7488 -> 7440, frees 25543 -> 25542, malloc_bytes 9729570 -> 9730124, opens 519 -> 521, read_bytes.ipc 11658 -> 11659, read_bytes.jobs 24062 -> 24056, read_bytes.store 30522 -> 30544, read_calls.ipc 539 -> 541, write_bytes.ipc 4181 -> 4180, write_bytes.jobs 14254 -> 14252, write_bytes.store 113185 -> 93475, write_calls.store 67 -> 55
- cold start: cli_cpu_us 25608 -> 24210, cli_footprint_kib 3057 -> 3073, cli_maxrss_kib 5184 -> 5200, frees 2316 -> 2314, malloc_bytes 1051633 -> 1051469, mallocs 3264 -> 3261
- restart: cli_cpu_us 59349 -> 47594, cli_processes 5 -> 4, frees 7036 -> 6082, malloc_bytes 2431566 -> 2140047, mallocs 10186 -> 9064, opens 277 -> 261, read_bytes.store 749876 -> 786956, read_calls.store 184 -> 193, write_bytes.ipc 3035 -> 1850, write_calls.ipc 45 -> 27

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13944 | 15888 | 13952 | 15776 | 0.994 | 0.992 to 1.000 |
| b | 130 | 15520 | 15984 | 15472 | 15920 | 0.996 | 0.996 to 0.998 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5329 | 6385 | 5377 | 6321 | 0.990 | 0.979 to 1.003 |
| b | 130 | 6033 | 6481 | 5953 | 6401 | 0.985 | 0.984 to 0.990 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10064 | 10416 | 10064 | 10448 | 1.002 | 0.997 to 1.005 |
| daemon b | 15360 | 16304 | 15504 | 16672 | 1.008 | 1.002 to 1.009 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3184 | 3569 | 3200 | 3585 | 1.005 | 0.995 to 1.022 |
| daemon b | 5793 | 6737 | 5921 | 7073 | 1.020 | 1.003 to 1.023 |
