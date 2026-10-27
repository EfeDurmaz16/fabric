### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 136.4 | 136.9 | 1.001 | 0.975 to 1.029 | not shown, median above 1 |
| acp launch | 15 | 72.2 | 64.6 | 0.896 | 0.877 to 0.943 | faster |
| acp approve | 15 | 15.0 | 15.1 | 1.009 | 0.943 to 1.057 | not shown, median above 1 |
| claude new | 15 | 133.1 | 134.0 | 1.011 | 0.987 to 1.035 | not shown, median above 1 |
| claude launch | 15 | 87.0 | 106.5 | 1.200 | 1.190 to 1.236 | slower |
| claude approve | 15 | 15.2 | 15.1 | 0.997 | 0.936 to 1.038 | not shown |
| codex new | 15 | 135.1 | 136.3 | 1.006 | 0.991 to 1.045 | not shown, median above 1 |
| codex launch | 15 | 101.9 | 118.2 | 1.184 | 1.142 to 1.222 | slower |
| codex approve | 15 | 15.0 | 15.9 | 1.052 | 0.997 to 1.079 | not shown, median above 1 |
| ledger append | 15 | 15.1 | 15.0 | 0.978 | 0.936 to 1.057 | not shown |
| cli version | 15 | 2.9 | 2.9 | 0.984 | 0.864 to 1.116 | not shown |
| cli ls | 15 | 3.5 | 3.6 | 1.025 | 1.006 to 1.042 | slower |
| acp suspend | 15 | 398.8 | 393.9 | 0.957 | 0.926 to 1.010 | not shown |
| acp move | 15 | 151.0 | 151.2 | 1.029 | 0.942 to 1.055 | not shown, median above 1 |
| acp resume | 15 | 655.1 | 717.0 | 1.009 | 0.969 to 1.068 | not shown, median above 1 |
| acp pull | 15 | 1279.0 | 1198.8 | 1.005 | 0.978 to 1.014 | not shown, median above 1 |
| claude suspend | 15 | 440.4 | 443.2 | 1.000 | 0.974 to 1.058 | not shown |
| claude move | 15 | 146.9 | 144.8 | 1.003 | 0.933 to 1.033 | not shown, median above 1 |
| claude resume | 15 | 873.0 | 835.6 | 0.992 | 0.962 to 1.032 | not shown |
| claude pull | 15 | 1446.7 | 1461.7 | 1.035 | 0.980 to 1.068 | not shown, median above 1 |
| codex suspend | 15 | 673.0 | 661.0 | 0.996 | 0.955 to 1.006 | not shown |
| codex move | 15 | 148.8 | 146.1 | 1.000 | 0.953 to 1.022 | not shown |
| codex resume | 15 | 1139.1 | 1131.9 | 1.021 | 0.998 to 1.027 | not shown, median above 1 |
| codex pull | 15 | 1907.9 | 1843.7 | 1.009 | 0.975 to 1.029 | not shown, median above 1 |
| cold start | 15 | 53.8 | 52.0 | 0.983 | 0.949 to 0.994 | faster |
| restart | 15 | 361.2 | 347.2 | 0.987 | 0.959 to 1.010 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38908 -> 38483 | 8080 -> 7800 | 9.8 -> 12.6 |
| acp launch | 4 -> 4 | 15 -> 12 | 0 -> 0 | 135 -> 135 | 1183 -> 682 | 4122 -> 4704 | 6952 -> 7376 | 12.3 -> 12.4 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1168 -> 1178 | 5064 -> 5128 | 9.9 -> 9.8 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1637 -> 1654 | 5184 -> 5152 | 10.7 -> 12.3 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1102 -> 1100 | 3498 -> 3600 | 6968 -> 7072 | 11.8 -> 11.0 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1170 -> 1175 | 5064 -> 5112 | 8.5 -> 9.5 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1635 -> 1656 | 5120 -> 5128 | 9.2 -> 10.6 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 684 -> 684 | 3816 -> 3924 | 7016 -> 7112 | 14.0 -> 12.8 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1184 -> 1176 | 5096 -> 5072 | 9.2 -> 9.6 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 74 -> 0 | 1168 -> 1179 | 5088 -> 5064 | 10.3 -> 10.1 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3168 -> 3184 | 5.9 -> 6.9 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1472 -> 1488 | 5184 -> 5200 | 8.8 -> 9.3 |
| acp suspend | 36 -> 36 | 39 -> 38 | 9 -> 9 | 522 -> 522 | 31820 -> 32120 | 14960 -> 12802 | 5264 -> 5384 | 8.5 -> 10.4 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5234 -> 5267 | 8268 -> 8337 | 5640 -> 5672 | 11.1 -> 12.3 |
| acp resume | 30 -> 31 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 14171 -> 14417 | 10638 -> 11001 | 6952 -> 7416 | 10.3 -> 12.9 |
| acp pull | 109 -> 110 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 31399 -> 32082 | 29828 -> 27930 | 7112 -> 7560 | 12.0 -> 9.9 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 12964 -> 13235 | 18314 -> 15718 | 5240 -> 5352 | 12.1 -> 8.7 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5280 -> 5311 | 8162 -> 8194 | 5656 -> 5680 | 11.6 -> 9.3 |
| claude resume | 32 -> 33 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 18618 -> 18777 | 10505 -> 10850 | 7392 -> 7440 | 11.3 -> 11.9 |
| claude pull | 112 -> 113 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 38192 -> 38811 | 34849 -> 32386 | 7528 -> 7736 | 14.1 -> 13.0 |
| codex suspend | 37 -> 37 | 39 -> 40 | 8 -> 8 | 522 -> 522 | 14155 -> 14320 | 16152 -> 12517 | 5264 -> 5344 | 10.2 -> 10.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5331 -> 5408 | 8154 -> 8193 | 5600 -> 5656 | 8.9 -> 10.2 |
| codex resume | 33 -> 34 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 22164 -> 22433 | 10676 -> 10997 | 7360 -> 7496 | 13.3 -> 11.2 |
| codex pull | 115 -> 116 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 42947 -> 43553 | 32000 -> 28704 | 7568 -> 7648 | 12.6 -> 12.0 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5809 -> 5822 | 5112 -> 5040 | 34.6 -> 34.2 |
| restart | 0 -> 0 | 35 -> 35 | 0 -> 0 | 463 -> 463 | 382924 -> 382924 | 15262 -> 14490 | 5280 -> 5352 | 76.7 -> 69.7 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 9812 -> 12556, cli_maxrss_kib 8080 -> 7800, frees 28090 -> 27762, malloc_bytes 13531422 -> 13423716, mallocs 38908 -> 38483
- acp launch: cli_cpu_us 12290 -> 12374, cli_maxrss_kib 6952 -> 7376, cmd:fabric-fake-agent 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 3364 -> 3746, fullfsync 15 -> 12, malloc_bytes 711940 -> 819110, mallocs 4122 -> 4704, opens 40 -> 37, read_bytes.jobs 1009 -> 508, read_calls.jobs 12 -> 10, renames 6 -> 5, spawn:fabric-fake-agent 1 -> 0, spawn:sh 0 -> 1, syncs.jobs 14 -> 11, write_bytes.ipc 968 -> 1024, write_bytes.jobs 1031 -> 576, write_calls.ipc 11 -> 14, write_calls.jobs 6 -> 5
- acp approve: cli_cpu_us 9872 -> 9850, cli_maxrss_kib 5064 -> 5128, frees 1017 -> 1026, malloc_bytes 316802 -> 318664, mallocs 1168 -> 1178
- claude new: cli_cpu_us 10698 -> 12328, cli_maxrss_kib 5184 -> 5152, frees 1382 -> 1399, malloc_bytes 383772 -> 385330, mallocs 1637 -> 1654
- claude launch: cli_cpu_us 11812 -> 11028, cli_maxrss_kib 6968 -> 7072, cmd:claude 1 -> 0, cmd:sh holding claude 0 -> 1, frees 3110 -> 3202, malloc_bytes 596655 -> 606900, mallocs 3498 -> 3600, read_bytes.jobs 928 -> 926, spawn:claude 1 -> 0, spawn:sh 0 -> 1, write_bytes.jobs 930 -> 930
- claude approve: cli_cpu_us 8504 -> 9519, cli_maxrss_kib 5064 -> 5112, frees 1020 -> 1025, malloc_bytes 316962 -> 317920, mallocs 1170 -> 1175
- codex new: cli_cpu_us 9246 -> 10596, cli_maxrss_kib 5120 -> 5128, frees 1381 -> 1402, malloc_bytes 383354 -> 385336, mallocs 1635 -> 1656
- codex launch: cli_cpu_us 14028 -> 12823, cli_maxrss_kib 7016 -> 7112, cmd:codex 1 -> 0, cmd:sh holding codex 0 -> 1, frees 3406 -> 3507, malloc_bytes 688394 -> 699866, mallocs 3816 -> 3924, read_calls.ipc 13 -> 14, spawn:codex 1 -> 0, spawn:sh 0 -> 1
- codex approve: cli_cpu_us 9184 -> 9603, cli_maxrss_kib 5096 -> 5072, frees 1034 -> 1026, malloc_bytes 317634 -> 318208, mallocs 1184 -> 1176
- ledger append: cli_cpu_us 10292 -> 10116, cli_maxrss_kib 5088 -> 5064, frees 1016 -> 1028, malloc_bytes 316972 -> 318368, mallocs 1168 -> 1179, opens 4 -> 3, read_bytes.state 74 -> 0, read_calls.state 2 -> 0
- cli version: cli_cpu_us 5858 -> 6870, cli_maxrss_kib 3168 -> 3184, malloc_bytes 87612 -> 87615
- cli ls: cli_cpu_us 8832 -> 9304, cli_maxrss_kib 5184 -> 5200, frees 1287 -> 1302, malloc_bytes 357977 -> 360088, mallocs 1472 -> 1488
- acp suspend: cli_cpu_us 8468 -> 10372, cli_maxrss_kib 5264 -> 5384, frees 10314 -> 9718, fullfsync 39 -> 38, malloc_bytes 3542377 -> 3504080, mallocs 14960 -> 12802, opens 142 -> 141, read_bytes.ipc 5268 -> 5324, read_bytes.store 29431 -> 29730, syncs.store 19 -> 18, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3584 -> 3582, write_bytes.store 87481 -> 87694
- acp move: cli_cpu_us 11102 -> 12295, cli_maxrss_kib 5640 -> 5672, frees 6783 -> 6846, malloc_bytes 1989096 -> 1995256, mallocs 8268 -> 8337, read_bytes.jobs 4111 -> 4099, read_bytes.store 1124 -> 1168, write_bytes.ipc 2697 -> 2509
- acp resume: cli_cpu_us 10298 -> 12864, cli_maxrss_kib 6952 -> 7416, cmd:fabric-fake-agent 1 -> 0, cmd:git symbolic-ref 1 -> 2, cmd:sh holding fabric-fake-agent 0 -> 1, frees 8548 -> 8878, malloc_bytes 4090550 -> 4133846, mallocs 10638 -> 11001, opens 207 -> 208, read_bytes.jobs 3556 -> 3564, read_bytes.store 10493 -> 10730, read_calls.ipc 153 -> 156, spawn:fabric-fake-agent 1 -> 0, spawn:git 22 -> 23, spawn:sh 0 -> 1, spawns 30 -> 31, write_bytes.ipc 2046 -> 2076, write_bytes.jobs 3548 -> 3560, write_bytes.store 3724 -> 3722, write_calls.ipc 28 -> 29
- acp pull: cli_cpu_us 12002 -> 9904, cli_maxrss_kib 7112 -> 7560, cmd:fabric-fake-agent 2 -> 1, cmd:git symbolic-ref 3 -> 4, cmd:sh holding fabric-fake-agent 0 -> 1, frees 22936 -> 22737, malloc_bytes 8534358 -> 8498770, mallocs 29828 -> 27930, opens 411 -> 412, read_bytes.ipc 10636 -> 10692, read_bytes.jobs 8528 -> 8542, read_bytes.store 22622 -> 23292, read_calls.ipc 479 -> 482, spawn:fabric-fake-agent 2 -> 1, spawn:git 56 -> 57, spawn:sh 0 -> 1, spawns 109 -> 110, write_bytes.ipc 4624 -> 4614, write_bytes.jobs 8378 -> 8400, write_bytes.store 55048 -> 55285, write_calls.ipc 84 -> 85
- claude suspend: cli_cpu_us 12142 -> 8732, cli_maxrss_kib 5240 -> 5352, frees 13294 -> 12407, malloc_bytes 3611466 -> 3593865, mallocs 18314 -> 15718, read_bytes.ipc 5236 -> 5292, read_bytes.store 9482 -> 9752, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4487 -> 4484, write_bytes.store 50613 -> 63189, write_calls.store 31 -> 37
- claude move: cli_cpu_us 11576 -> 9330, cli_maxrss_kib 5656 -> 5680, frees 6710 -> 6742, malloc_bytes 1976828 -> 1981622, mallocs 8162 -> 8194, read_bytes.jobs 4078 -> 4064, read_bytes.store 1202 -> 1247
- claude resume: cli_cpu_us 11308 -> 11867, cli_maxrss_kib 7392 -> 7440, cmd:claude 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding claude 0 -> 1, frees 8670 -> 8978, malloc_bytes 4101652 -> 4138488, mallocs 10505 -> 10850, read_bytes.ipc 3947 -> 3948, read_bytes.jobs 7466 -> 7470, read_bytes.state 196 -> 122, read_bytes.store 10956 -> 11186, read_calls.ipc 159 -> 160, read_calls.state 6 -> 4, spawn:claude 3 -> 2, spawn:git 22 -> 23, spawn:sh 0 -> 1, spawns 32 -> 33, write_bytes.ipc 1852 -> 1779, write_bytes.jobs 4422 -> 4416, write_bytes.store 3723 -> 3722
- claude pull: cli_cpu_us 14116 -> 13000, cli_maxrss_kib 7528 -> 7736, cmd:claude 8 -> 7, cmd:git symbolic-ref 3 -> 4, cmd:sh holding claude 0 -> 1, frees 26790 -> 26210, malloc_bytes 8869282 -> 8836573, mallocs 34849 -> 32386, opens 453 -> 454, read_bytes.ipc 10136 -> 10190, read_bytes.jobs 14337 -> 14352, read_bytes.store 23682 -> 24284, read_calls.ipc 490 -> 491, spawn:claude 8 -> 7, spawn:git 56 -> 57, spawn:sh 0 -> 1, spawns 112 -> 113, write_bytes.ipc 4278 -> 4122, write_bytes.jobs 10338 -> 10357, write_bytes.store 67566 -> 55427, write_calls.store 42 -> 36
- codex suspend: cli_cpu_us 10232 -> 10330, cli_maxrss_kib 5264 -> 5344, frees 11630 -> 10218, fullfsync 39 -> 40, malloc_bytes 3697724 -> 3622864, mallocs 16152 -> 12517, opens 153 -> 152, read_bytes.ipc 5250 -> 5306, read_bytes.state 235 -> 87, read_bytes.store 9822 -> 10136, read_calls.state 6 -> 2, syncs.store 17 -> 18, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5247 -> 5257, write_bytes.store 67480 -> 55354, write_calls.store 39 -> 33
- codex move: cli_cpu_us 8869 -> 10192, cli_maxrss_kib 5600 -> 5656, frees 6708 -> 6746, malloc_bytes 1976376 -> 1981664, mallocs 8154 -> 8193, read_bytes.jobs 4096 -> 4120, read_bytes.store 1235 -> 1288
- codex resume: cli_cpu_us 13258 -> 11202, cli_maxrss_kib 7360 -> 7496, cmd:codex 3 -> 2, cmd:git symbolic-ref 1 -> 2, cmd:sh holding codex 0 -> 1, frees 8853 -> 9142, malloc_bytes 4210028 -> 4245043, mallocs 10676 -> 10997, opens 287 -> 288, read_bytes.jobs 10612 -> 10618, read_bytes.store 11430 -> 11694, read_calls.ipc 164 -> 165, spawn:codex 3 -> 2, spawn:git 22 -> 23, spawn:sh 0 -> 1, spawns 33 -> 34, write_bytes.ipc 1786 -> 1713, write_bytes.jobs 5232 -> 5241
- codex pull: cli_cpu_us 12605 -> 11964, cli_maxrss_kib 7568 -> 7648, cmd:codex 9 -> 8, cmd:git symbolic-ref 3 -> 4, cmd:sh holding codex 0 -> 1, frees 24669 -> 23568, malloc_bytes 9014723 -> 8978280, mallocs 32000 -> 28704, read_bytes.ipc 10298 -> 10354, read_bytes.jobs 18132 -> 18134, read_bytes.state 248 -> 174, read_bytes.store 24568 -> 25244, read_calls.ipc 498 -> 500, read_calls.state 6 -> 4, spawn:codex 9 -> 8, spawn:git 56 -> 57, spawn:sh 0 -> 1, spawns 115 -> 116, write_bytes.ipc 4212 -> 4056, write_bytes.jobs 11886 -> 11888, write_bytes.store 72069 -> 76418, write_calls.store 44 -> 46
- cold start: cli_cpu_us 34600 -> 34226, cli_maxrss_kib 5112 -> 5040, frees 4374 -> 4357, malloc_bytes 1629571 -> 1639416, mallocs 5809 -> 5822
- restart: cli_cpu_us 76724 -> 69686, cli_maxrss_kib 5280 -> 5352, cli_processes 8 -> 8, frees 11496 -> 10794, malloc_bytes 3808726 -> 3640514, mallocs 15262 -> 14490, opens 221 -> 214, write_bytes.ipc 3316 -> 2834, write_calls.ipc 48 -> 40

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 11280 | 11584 | 11264 | 11552 | 0.997 | 0.990 to 1.000 |
| daemon b | 15776 | 16912 | 15968 | 17104 | 1.013 | 1.010 to 1.023 |
