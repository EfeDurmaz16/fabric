### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 257.9 | 231.6 | 0.926 | 0.737 to 1.094 | not shown |
| acp launch | 15 | 29.4 | 31.2 | 1.033 | 0.957 to 1.104 | not shown, median above 1 |
| acp approve | 15 | 6.1 | 6.5 | 1.034 | 0.973 to 1.050 | not shown, median above 1 |
| claude new | 15 | 243.6 | 257.3 | 1.041 | 0.878 to 1.212 | not shown, median above 1 |
| claude launch | 15 | 210.5 | 167.5 | 0.920 | 0.637 to 1.000 | not shown |
| claude approve | 15 | 8.2 | 7.8 | 0.978 | 0.947 to 1.066 | not shown |
| codex new | 15 | 257.6 | 259.0 | 1.011 | 0.873 to 1.082 | not shown, median above 1 |
| codex launch | 15 | 197.9 | 243.8 | 1.066 | 0.931 to 1.142 | not shown, median above 1 |
| codex approve | 15 | 9.7 | 10.2 | 0.967 | 0.906 to 1.059 | not shown |
| ledger append | 15 | 8.9 | 8.5 | 0.955 | 0.806 to 1.032 | not shown |
| cli version | 15 | 3.9 | 3.5 | 0.955 | 0.896 to 1.083 | not shown |
| cli ls | 15 | 4.6 | 4.5 | 1.000 | 0.974 to 1.084 | not shown |
| acp suspend | 15 | 478.3 | 524.5 | 0.978 | 0.925 to 1.139 | not shown |
| acp move | 15 | 310.3 | 344.0 | 1.036 | 0.940 to 1.175 | not shown, median above 1 |
| acp resume | 15 | 984.6 | 969.4 | 1.038 | 0.923 to 1.124 | not shown, median above 1 |
| acp pull | 15 | 1716.2 | 1805.7 | 0.959 | 0.923 to 1.039 | not shown |
| claude suspend | 15 | 653.1 | 695.5 | 1.022 | 0.978 to 1.144 | not shown, median above 1 |
| claude move | 15 | 307.1 | 343.8 | 1.087 | 0.948 to 1.130 | not shown, median above 1 |
| claude resume | 15 | 1410.4 | 1359.0 | 0.972 | 0.912 to 1.096 | not shown |
| claude pull | 15 | 2266.7 | 2382.8 | 1.015 | 0.979 to 1.115 | not shown, median above 1 |
| codex suspend | 15 | 743.6 | 723.4 | 1.017 | 0.922 to 1.063 | not shown, median above 1 |
| codex move | 15 | 290.9 | 309.9 | 1.054 | 0.942 to 1.200 | not shown, median above 1 |
| codex resume | 15 | 1381.6 | 1372.0 | 1.019 | 0.929 to 1.096 | not shown, median above 1 |
| codex pull | 15 | 2430.5 | 2466.1 | 0.980 | 0.917 to 1.071 | not shown |
| cold start | 15 | 65.4 | 82.5 | 1.120 | 0.941 to 1.904 | not shown, median above 1 |
| restart | 15 | 180.5 | 162.4 | 0.978 | 0.816 to 1.208 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI peak footprint KiB | CLI CPU ms | instructions M |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38480 -> 38479 | 7120 -> 7152 | 4049 -> 4097 | 10.0 -> 9.2 | 0.0 -> 0.0 |
| acp launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 135 -> 135 | 1183 -> 1187 | 3649 -> 3656 | 7008 -> 7024 | 3393 -> 3393 | 10.2 -> 8.9 | 0.0 -> 0.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5248 -> 5184 | 3121 -> 3041 | 7.2 -> 7.3 | 0.0 -> 0.0 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1205 | 5360 -> 5296 | 3169 -> 3089 | 8.3 -> 9.7 | 0.0 -> 0.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1250 -> 1101 | 3059 -> 3047 | 7024 -> 7056 | 3393 -> 3409 | 9.3 -> 8.8 | 0.0 -> 0.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5248 -> 5200 | 3105 -> 3057 | 7.5 -> 8.0 | 0.0 -> 0.0 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1204 -> 1204 | 5344 -> 5392 | 3137 -> 3185 | 9.1 -> 7.5 | 0.0 -> 0.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3365 -> 3365 | 7104 -> 7120 | 3377 -> 3377 | 11.3 -> 10.3 | 0.0 -> 0.0 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 731 -> 731 | 5200 -> 5184 | 3057 -> 3041 | 8.1 -> 7.8 | 0.0 -> 0.0 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 732 -> 732 | 5184 -> 5200 | 3057 -> 3073 | 7.3 -> 8.8 | 0.0 -> 0.0 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3552 -> 3552 | 2625 -> 2609 | 9.7 -> 6.4 | 0.0 -> 0.0 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1173 -> 1173 | 5248 -> 5232 | 3089 -> 3073 | 8.3 -> 7.6 | 0.0 -> 0.0 |
| acp suspend | 35 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 48260 -> 48160 | 14496 -> 14480 | 5584 -> 5616 | 3249 -> 3281 | 7.1 -> 8.1 | 0.0 -> 0.0 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5227 -> 5246 | 7941 -> 7935 | 5776 -> 5776 | 3313 -> 3313 | 9.2 -> 9.0 | 0.0 -> 0.0 |
| acp resume | 36 -> 36 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 19862 -> 19823 | 11220 -> 11218 | 7088 -> 7104 | 3409 -> 3441 | 10.2 -> 10.5 | 0.0 -> 0.0 |
| acp pull | 115 -> 115 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 37171 -> 37174 | 30765 -> 30778 | 7168 -> 7184 | 3473 -> 3489 | 10.0 -> 10.2 | 0.0 -> 0.0 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13834 -> 13853 | 20444 -> 20475 | 5584 -> 5552 | 3265 -> 3233 | 8.2 -> 7.7 | 0.0 -> 0.0 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5269 -> 5280 | 7865 -> 7865 | 5776 -> 5776 | 3329 -> 3313 | 7.7 -> 10.6 | 0.0 -> 0.0 |
| claude resume | 38 -> 38 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 27862 -> 27879 | 11663 -> 11651 | 7424 -> 7408 | 3489 -> 3489 | 11.8 -> 12.1 | 0.0 -> 0.0 |
| claude pull | 118 -> 118 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 48500 -> 48438 | 38521 -> 38534 | 7472 -> 7440 | 3521 -> 3473 | 11.3 -> 12.1 | 0.0 -> 0.0 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 15208 -> 15203 | 16014 -> 15802 | 5568 -> 5552 | 3265 -> 3233 | 9.2 -> 11.2 | 0.0 -> 0.0 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5332 -> 5318 | 7870 -> 7861 | 5792 -> 5760 | 3313 -> 3281 | 7.5 -> 7.9 | 0.0 -> 0.0 |
| codex resume | 39 -> 39 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 32936 -> 32933 | 11755 -> 11749 | 7408 -> 7360 | 3505 -> 3457 | 11.2 -> 11.3 | 0.0 -> 0.0 |
| codex pull | 121 -> 121 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 54842 -> 54971 | 33147 -> 33165 | 7456 -> 7456 | 3505 -> 3505 | 10.2 -> 13.4 | 0.0 -> 0.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45370 -> 45222 | 3263 -> 3694 | 5200 -> 5216 | 3105 -> 3089 | 25.8 -> 25.0 | 0.0 -> 0.0 |
| restart | 0 -> 0 | 44 -> 44 | 0 -> 0 | 463 -> 463 | 809067 -> 771987 | 10098 -> 8893 | 5248 -> 5312 | 3089 -> 3153 | 38.1 -> 27.6 | 0.0 -> 0.0 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 10035 -> 9212, cli_footprint_kib 4049 -> 4097, cli_maxrss_kib 7120 -> 7152, malloc_bytes 13401541 -> 13401419, mallocs 38480 -> 38479
- acp launch: cli_cpu_us 10237 -> 8921, cli_maxrss_kib 7008 -> 7024, frees 2923 -> 2930, malloc_bytes 635008 -> 635375, mallocs 3649 -> 3656, read_bytes.jobs 1009 -> 1013, write_bytes.jobs 1031 -> 1035
- acp approve: cli_cpu_us 7222 -> 7314, cli_footprint_kib 3121 -> 3041, cli_maxrss_kib 5248 -> 5184, malloc_bytes 242240 -> 242243
- claude new: cli_cpu_us 8350 -> 9695, cli_footprint_kib 3169 -> 3089, cli_maxrss_kib 5360 -> 5296, malloc_bytes 308809 -> 308876, mallocs 1204 -> 1205
- claude launch: cli_cpu_us 9313 -> 8840, cli_footprint_kib 3393 -> 3409, cli_maxrss_kib 7024 -> 7056, frees 2704 -> 2692, malloc_bytes 522214 -> 521281, mallocs 3059 -> 3047, opens 45 -> 43, read_bytes.jobs 928 -> 927, read_bytes.state 322 -> 174, read_calls.state 8 -> 4, write_bytes.jobs 931 -> 930
- claude approve: cli_cpu_us 7491 -> 8017, cli_footprint_kib 3105 -> 3057, cli_maxrss_kib 5248 -> 5200, malloc_bytes 241984 -> 241987
- codex new: cli_cpu_us 9067 -> 7496, cli_footprint_kib 3137 -> 3185, cli_maxrss_kib 5344 -> 5392, malloc_bytes 308753 -> 308756
- codex launch: cli_cpu_us 11301 -> 10321, cli_maxrss_kib 7104 -> 7120, malloc_bytes 612964 -> 612967
- codex approve: cli_cpu_us 8127 -> 7755, cli_footprint_kib 3057 -> 3041, cli_maxrss_kib 5200 -> 5184, malloc_bytes 241984 -> 241987
- ledger append: cli_cpu_us 7305 -> 8769, cli_footprint_kib 3057 -> 3073, cli_maxrss_kib 5184 -> 5200, malloc_bytes 242240 -> 242243
- cli version: cli_cpu_us 9665 -> 6433, cli_footprint_kib 2625 -> 2609, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 8288 -> 7647, cli_footprint_kib 3089 -> 3073, cli_maxrss_kib 5248 -> 5232, malloc_bytes 297439 -> 297442
- acp suspend: cli_cpu_us 7104 -> 8050, cli_footprint_kib 3249 -> 3281, cli_maxrss_kib 5584 -> 5616, frees 9877 -> 9869, malloc_bytes 3481660 -> 3482434, mallocs 14496 -> 14480, read_bytes.store 45871 -> 45771, read_calls.ipc 184 -> 182, write_bytes.jobs 3588 -> 3599, write_bytes.store 95726 -> 95692
- acp move: cli_cpu_us 9153 -> 9040, frees 6508 -> 6503, malloc_bytes 1976156 -> 1975458, mallocs 7941 -> 7935, read_bytes.jobs 4103 -> 4135, read_bytes.store 1124 -> 1111, write_bytes.ipc 2666 -> 2665
- acp resume: cli_cpu_us 10211 -> 10516, cli_footprint_kib 3409 -> 3441, cli_maxrss_kib 7088 -> 7104, frees 9214 -> 9212, malloc_bytes 4659371 -> 4659264, mallocs 11220 -> 11218, read_bytes.ipc 5860 -> 5858, read_bytes.store 16180 -> 16141, write_bytes.jobs 3558 -> 3556, write_bytes.store 6148 -> 6150
- acp pull: cli_cpu_us 10044 -> 10187, cli_footprint_kib 3473 -> 3489, cli_maxrss_kib 7168 -> 7184, frees 23595 -> 23594, malloc_bytes 9195404 -> 9213977, mallocs 30765 -> 30778, read_bytes.ipc 11994 -> 11995, read_bytes.jobs 8547 -> 8542, read_bytes.store 28302 -> 28310, read_calls.ipc 523 -> 516, write_bytes.ipc 4593 -> 4592, write_bytes.jobs 8406 -> 8405, write_bytes.store 69831 -> 94589, write_calls.store 45 -> 57
- claude suspend: cli_cpu_us 8209 -> 7748, cli_footprint_kib 3265 -> 3233, cli_maxrss_kib 5584 -> 5552, frees 14818 -> 14811, malloc_bytes 3698396 -> 3699757, mallocs 20444 -> 20475, read_bytes.ipc 5236 -> 5237, read_bytes.store 9513 -> 9532, read_calls.ipc 183 -> 185, write_bytes.jobs 5305 -> 5314, write_bytes.store 87716 -> 62996, write_calls.store 49 -> 37
- claude move: cli_cpu_us 7679 -> 10596, cli_footprint_kib 3329 -> 3313, frees 6442 -> 6437, malloc_bytes 1966866 -> 1967393, read_bytes.jobs 4062 -> 4070, read_bytes.store 1207 -> 1210, write_bytes.ipc 2330 -> 2329
- claude resume: cli_cpu_us 11834 -> 12084, cli_maxrss_kib 7424 -> 7408, frees 9519 -> 9514, malloc_bytes 4794597 -> 4793444, mallocs 11663 -> 11651, opens 260 -> 258, read_bytes.ipc 5338 -> 5337, read_bytes.jobs 10820 -> 10818, read_bytes.store 16920 -> 16939, read_calls.ipc 206 -> 214, write_bytes.jobs 5255 -> 5281, write_bytes.store 6148 -> 6150
- claude pull: cli_cpu_us 11307 -> 12097, cli_footprint_kib 3521 -> 3473, cli_maxrss_kib 7472 -> 7440, frees 29543 -> 29567, malloc_bytes 9681773 -> 9685421, mallocs 38521 -> 38534, opens 470 -> 468, read_bytes.ipc 11494 -> 11493, read_bytes.jobs 18515 -> 18506, read_bytes.state 322 -> 174, read_bytes.store 29663 -> 29758, read_calls.ipc 519 -> 528, read_calls.state 8 -> 4, write_bytes.ipc 4247 -> 4246, write_bytes.jobs 12010 -> 11990, write_bytes.store 102967 -> 102987
- codex suspend: cli_cpu_us 9234 -> 11179, cli_footprint_kib 3265 -> 3233, cli_maxrss_kib 5568 -> 5552, frees 11309 -> 11306, malloc_bytes 3718945 -> 3676655, mallocs 16014 -> 15802, read_bytes.store 9836 -> 9831, write_bytes.jobs 6429 -> 6436, write_bytes.store 88088 -> 112794, write_calls.store 49 -> 61
- codex move: cli_cpu_us 7541 -> 7890, cli_footprint_kib 3313 -> 3281, cli_maxrss_kib 5792 -> 5760, frees 6443 -> 6428, malloc_bytes 1967113 -> 1967154, mallocs 7870 -> 7861, read_bytes.jobs 4098 -> 4086, read_bytes.store 1234 -> 1232, write_bytes.ipc 2330 -> 2329
- codex resume: cli_cpu_us 11174 -> 11259, cli_footprint_kib 3505 -> 3457, cli_maxrss_kib 7408 -> 7360, malloc_bytes 4896369 -> 4895968, mallocs 11755 -> 11749, read_bytes.jobs 15360 -> 15364, read_bytes.store 17454 -> 17447, read_calls.ipc 207 -> 211, write_bytes.jobs 6422 -> 6433, write_bytes.store 6145 -> 6153
- codex pull: cli_cpu_us 10235 -> 13359, frees 25555 -> 25563, malloc_bytes 9729964 -> 9731349, mallocs 33147 -> 33165, opens 519 -> 521, read_bytes.ipc 11659 -> 11656, read_bytes.jobs 24060 -> 24053, read_bytes.state 174 -> 322, read_bytes.store 30608 -> 30596, read_calls.ipc 544 -> 530, read_calls.state 4 -> 8, write_bytes.ipc 4181 -> 4180, write_bytes.store 113187 -> 103342, write_calls.store 67 -> 61
- cold start: cli_cpu_us 25768 -> 24979, cli_footprint_kib 3105 -> 3089, cli_maxrss_kib 5200 -> 5216, cli_processes 3 -> 4, frees 2315 -> 2645, malloc_bytes 1051569 -> 1260744, mallocs 3263 -> 3694, opens 23 -> 21, read_bytes.state 198 -> 50, read_calls.state 6 -> 2, write_bytes.ipc 335 -> 487, write_calls.ipc 5 -> 7
- restart: cli_cpu_us 38070 -> 27587, cli_footprint_kib 3089 -> 3153, cli_maxrss_kib 5248 -> 5312, cli_processes 4 -> 3, frees 6646 -> 5625, malloc_bytes 2359776 -> 2060251, mallocs 10098 -> 8893, opens 277 -> 261, read_bytes.store 778716 -> 741636, read_calls.store 191 -> 182, write_bytes.ipc 1838 -> 1307, write_calls.ipc 27 -> 20

### Daemon resident size, counted runs

KiB after each counted op, every page in memory, the clean pages of the binary too, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 13840 | 15648 | 13816 | 15584 | 1.000 | 0.999 to 1.001 |
| b | 130 | 15448 | 15968 | 15472 | 16032 | 0.996 | 0.996 to 0.997 |

### Daemon physical footprint, counted runs

KiB after each counted op, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it, both builds with the counter library loaded: the number of ops, median and largest per build, and the median of the ops' ratios with its 95% bootstrap interval.

| daemon | ops | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---:|---|
| a | 130 | 5217 | 6161 | 5209 | 6097 | 1.000 | 0.997 to 1.003 |
| b | 130 | 5945 | 6465 | 5969 | 6529 | 0.990 | 0.989 to 0.992 |

### Daemon resident size

KiB after each timed run, every page in memory, the clean pages of the binary too: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 10096 | 10432 | 10080 | 10448 | 0.998 | 0.997 to 1.006 |
| daemon b | 15568 | 16448 | 15408 | 16528 | 0.994 | 0.990 to 1.003 |

### Daemon physical footprint

KiB after each timed run, the pages the daemon dirtied, compressed ones included, as `footprint(1)` reports them; the clean pages of the binary are not in it: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 3233 | 3569 | 3249 | 3617 | 0.995 | 0.990 to 1.025 |
| daemon b | 6017 | 6865 | 5857 | 6961 | 0.985 | 0.976 to 1.008 |
