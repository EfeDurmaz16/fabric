### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 511.5 | 535.2 | 1.005 | 0.795 to 1.141 | not shown, median above 1 |
| acp launch | 20 | 165.2 | 167.8 | 0.979 | 0.797 to 1.232 | not shown |
| acp approve | 20 | 42.1 | 40.0 | 1.057 | 0.896 to 1.125 | not shown, median above 1 |
| claude new | 20 | 487.1 | 439.9 | 0.950 | 0.718 to 1.114 | not shown |
| claude launch | 20 | 193.2 | 219.9 | 1.053 | 1.004 to 1.095 | slower |
| claude approve | 20 | 43.7 | 39.0 | 0.983 | 0.735 to 1.213 | not shown |
| codex new | 20 | 432.7 | 421.1 | 1.037 | 0.878 to 1.364 | not shown, median above 1 |
| codex launch | 20 | 287.2 | 254.6 | 0.885 | 0.812 to 0.910 | faster |
| codex approve | 20 | 43.2 | 39.8 | 0.875 | 0.800 to 1.024 | not shown |
| ledger append | 20 | 33.7 | 36.8 | 1.030 | 0.960 to 1.201 | not shown, median above 1 |
| cli version | 20 | 3.5 | 3.2 | 0.943 | 0.883 to 1.108 | not shown |
| cli ls | 20 | 6.5 | 6.4 | 0.989 | 0.959 to 1.036 | not shown |
| acp suspend | 20 | 1042.6 | 1017.0 | 0.985 | 0.898 to 1.018 | not shown |
| acp move | 20 | 278.9 | 322.7 | 1.087 | 0.925 to 1.220 | not shown, median above 1 |
| acp resume | 20 | 1882.8 | 2109.0 | 1.076 | 0.942 to 1.176 | not shown, median above 1 |
| acp pull | 20 | 3234.3 | 3045.9 | 1.000 | 0.946 to 1.058 | not shown |
| claude suspend | 20 | 1162.8 | 1120.7 | 0.974 | 0.898 to 1.046 | not shown |
| claude move | 20 | 294.4 | 272.7 | 0.956 | 0.898 to 1.005 | not shown |
| claude resume | 20 | 1974.9 | 2018.0 | 1.048 | 0.995 to 1.063 | not shown, median above 1 |
| claude pull | 20 | 3632.4 | 3303.8 | 0.959 | 0.915 to 0.992 | faster |
| codex suspend | 20 | 1483.8 | 1419.2 | 0.976 | 0.905 to 1.092 | not shown |
| codex move | 20 | 298.5 | 337.2 | 1.097 | 0.951 to 1.286 | not shown, median above 1 |
| codex resume | 20 | 2412.8 | 2240.0 | 0.943 | 0.868 to 0.988 | faster |
| codex pull | 20 | 4239.5 | 4133.8 | 1.011 | 0.981 to 1.065 | not shown, median above 1 |
| cold start | 20 | 247.2 | 321.4 | 1.075 | 0.880 to 2.227 | not shown, median above 1 |
| restart | 20 | 1520.1 | 1428.0 | 0.950 | 0.903 to 1.008 | not shown |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38523 -> 38524 | 15440 -> 15048 | 24.1 -> 24.6 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4924 -> 4926 | 15464 -> 15512 | 19.6 -> 22.4 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1210 | 10944 -> 10864 | 12.3 -> 11.5 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1688 -> 1688 | 11048 -> 11032 | 12.5 -> 14.8 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1101 | 3618 -> 3629 | 14792 -> 14776 | 18.7 -> 16.4 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1210 | 10912 -> 10816 | 13.8 -> 13.8 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 198 | 1688 -> 1688 | 11024 -> 10920 | 12.3 -> 13.4 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3960 -> 3954 | 14744 -> 14712 | 19.6 -> 24.8 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1207 -> 1214 | 10968 -> 10784 | 12.1 -> 13.4 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1209 -> 1208 | 10952 -> 10848 | 12.0 -> 12.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4936 -> 4880 | 8.8 -> 8.4 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1508 -> 1513 | 11072 -> 10992 | 11.6 -> 12.1 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 32166 -> 32268 | 12846 -> 12848 | 11640 -> 11632 | 12.2 -> 12.2 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5272 -> 5257 | 8378 -> 8376 | 12320 -> 12256 | 15.1 -> 15.6 |
| acp resume | 31 -> 31 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 14590 -> 14402 | 11038 -> 11047 | 14272 -> 15000 | 23.5 -> 21.7 |
| acp pull | 110 -> 110 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 32201 -> 32120 | 27976 -> 28648 | 14448 -> 14424 | 20.5 -> 25.6 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13270 -> 13262 | 15516 -> 15522 | 11448 -> 11336 | 13.3 -> 15.3 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5324 -> 5380 | 8236 -> 8240 | 12288 -> 12288 | 13.9 -> 14.7 |
| claude resume | 33 -> 33 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 19013 -> 18862 | 10890 -> 10902 | 14648 -> 14768 | 20.1 -> 24.9 |
| claude pull | 113 -> 113 | 83 -> 82 | 63 -> 63 | 1338 -> 1338 | 39060 -> 38974 | 32470 -> 32664 | 14752 -> 14848 | 22.3 -> 20.9 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14424 -> 14400 | 12546 -> 12995 | 11656 -> 11632 | 11.8 -> 12.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5352 -> 5374 | 8223 -> 8255 | 12224 -> 12272 | 13.3 -> 12.6 |
| codex resume | 34 -> 34 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 22510 -> 22572 | 11042 -> 11040 | 14832 -> 14792 | 17.4 -> 18.1 |
| codex pull | 116 -> 116 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 43769 -> 43668 | 29241 -> 29006 | 14776 -> 14960 | 22.9 -> 20.5 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45326 -> 45222 | 17051 -> 17944 | 10856 -> 10752 | 177.4 -> 184.2 |
| restart | 0 -> 0 | 35 -> 35 | 0 -> 0 | 463 -> 463 | 380864 -> 387074 | 21205 -> 20674 | 11224 -> 11192 | 153.9 -> 157.9 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 24068 -> 24614, cli_maxrss_kib 15440 -> 15048, frees 27790 -> 27792, malloc_bytes 13432256 -> 13432075, mallocs 38523 -> 38524
- acp launch: cli_cpu_us 19554 -> 22406, cli_maxrss_kib 15464 -> 15512, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:sh holding fabric-fake-agent 1 -> 0, frees 3772 -> 3774, malloc_bytes 877652 -> 877358, mallocs 4924 -> 4926, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 1025 -> 1026, write_calls.ipc 14 -> 15
- acp approve: cli_cpu_us 12318 -> 11461, cli_maxrss_kib 10944 -> 10864, frees 1049 -> 1050, malloc_bytes 325419 -> 326078, mallocs 1208 -> 1210
- claude new: cli_cpu_us 12499 -> 14759, cli_maxrss_kib 11048 -> 11032, frees 1423 -> 1424, malloc_bytes 393253 -> 393138, mallocs 1688 -> 1688
- claude launch: cli_cpu_us 18720 -> 16350, cli_maxrss_kib 14792 -> 14776, cmd:dash holding claude 0 -> 1, cmd:sh holding claude 1 -> 0, frees 3213 -> 3224, malloc_bytes 613864 -> 615000, mallocs 3618 -> 3629, opens 43 -> 42, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 692 -> 693, write_calls.ipc 11 -> 12
- claude approve: cli_cpu_us 13786 -> 13754, cli_maxrss_kib 10912 -> 10816, frees 1049 -> 1052, malloc_bytes 325547 -> 325574, mallocs 1208 -> 1210
- codex new: cli_cpu_us 12299 -> 13423, cli_maxrss_kib 11024 -> 10920, frees 1424 -> 1424, malloc_bytes 392965 -> 393242, mallocs 1688 -> 1688, opens 11 -> 12, read_bytes.state 0 -> 74, read_calls.state 0 -> 2
- codex launch: cli_cpu_us 19558 -> 24838, cli_maxrss_kib 14744 -> 14712, cmd:dash holding codex 0 -> 1, cmd:sh holding codex 1 -> 0, frees 3532 -> 3533, malloc_bytes 708440 -> 708148, mallocs 3960 -> 3954, opens 46 -> 45, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: cli_cpu_us 12076 -> 13354, cli_maxrss_kib 10968 -> 10784, frees 1049 -> 1055, malloc_bytes 325291 -> 326006, mallocs 1207 -> 1214
- ledger append: cli_cpu_us 11986 -> 12459, cli_maxrss_kib 10952 -> 10848, malloc_bytes 326059 -> 325622, mallocs 1209 -> 1208
- cli version: cli_cpu_us 8824 -> 8428, cli_maxrss_kib 4936 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 11562 -> 12095, cli_maxrss_kib 11072 -> 10992, frees 1316 -> 1320, malloc_bytes 366406 -> 366865, mallocs 1508 -> 1513
- acp suspend: cli_cpu_us 12192 -> 12221, cli_maxrss_kib 11640 -> 11632, malloc_bytes 3513348 -> 3521578, mallocs 12846 -> 12848, opens 142 -> 144, read_bytes.state 87 -> 235, read_bytes.store 29777 -> 29731, read_calls.state 2 -> 6, write_bytes.jobs 3584 -> 3582, write_bytes.store 87695 -> 87689
- acp move: cli_cpu_us 15147 -> 15556, cli_maxrss_kib 12320 -> 12256, frees 6881 -> 6873, malloc_bytes 2008797 -> 2008569, mallocs 8378 -> 8376, read_bytes.jobs 4097 -> 4087, read_bytes.store 1174 -> 1170
- acp resume: cli_cpu_us 23452 -> 21673, cli_maxrss_kib 14272 -> 15000, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:sh holding fabric-fake-agent 1 -> 0, frees 8900 -> 8910, malloc_bytes 4143400 -> 4143218, mallocs 11038 -> 11047, opens 210 -> 208, read_bytes.ipc 4472 -> 4472, read_bytes.jobs 3560 -> 3556, read_bytes.state 270 -> 122, read_bytes.store 10760 -> 10724, read_calls.ipc 151 -> 152, read_calls.state 8 -> 4, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 2076 -> 2077, write_bytes.jobs 3556 -> 3554, write_bytes.store 3724 -> 3721, write_calls.ipc 29 -> 30
- acp pull: cli_cpu_us 20466 -> 25556, cli_maxrss_kib 14448 -> 14424, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:sh holding fabric-fake-agent 1 -> 0, frees 22778 -> 22829, malloc_bytes 8515532 -> 8682924, mallocs 27976 -> 28648, read_bytes.ipc 10694 -> 10693, read_bytes.jobs 8534 -> 8542, read_bytes.store 23345 -> 23256, read_calls.ipc 478 -> 478, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 4616 -> 4617, write_bytes.jobs 8392 -> 8396, write_bytes.store 55280 -> 55268, write_calls.ipc 85 -> 86
- claude suspend: cli_cpu_us 13254 -> 15298, cli_maxrss_kib 11448 -> 11336, frees 12402 -> 12409, malloc_bytes 3547349 -> 3547400, mallocs 15516 -> 15522, read_bytes.store 9788 -> 9780, read_calls.ipc 185 -> 184, write_bytes.jobs 4484 -> 4476, write_bytes.store 50831 -> 63201, write_calls.store 31 -> 37
- claude move: cli_cpu_us 13852 -> 14680, frees 6774 -> 6773, malloc_bytes 1995222 -> 1996600, mallocs 8236 -> 8240, opens 73 -> 74, read_bytes.jobs 4074 -> 4054, read_bytes.state 0 -> 74, read_bytes.store 1250 -> 1252, read_calls.ipc 124 -> 124, read_calls.state 0 -> 2
- claude resume: cli_cpu_us 20104 -> 24905, cli_maxrss_kib 14648 -> 14768, cmd:dash holding claude 0 -> 1, cmd:sh holding claude 1 -> 0, frees 9006 -> 9010, malloc_bytes 4148593 -> 4148925, mallocs 10890 -> 10902, opens 247 -> 244, read_bytes.ipc 3950 -> 3952, read_bytes.jobs 7467 -> 7468, read_bytes.state 344 -> 196, read_bytes.store 11202 -> 11198, read_calls.ipc 157 -> 158, read_calls.state 10 -> 6, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 1779 -> 1780, write_bytes.jobs 4425 -> 4422, write_calls.ipc 28 -> 29
- claude pull: cli_cpu_us 22280 -> 20914, cli_maxrss_kib 14752 -> 14848, cmd:dash holding claude 0 -> 1, cmd:sh holding claude 1 -> 0, frees 26269 -> 26264, fullfsync 83 -> 82, malloc_bytes 8855625 -> 8907052, mallocs 32470 -> 32664, opens 457 -> 454, read_bytes.jobs 14338 -> 14331, read_bytes.state 396 -> 322, read_bytes.store 24325 -> 24321, read_calls.ipc 489 -> 492, read_calls.state 10 -> 8, spawn:dash 0 -> 1, spawn:sh 1 -> 0, syncs.store 18 -> 17, write_bytes.jobs 10347 -> 10330, write_bytes.store 55432 -> 55432, write_calls.ipc 80 -> 81
- codex suspend: cli_cpu_us 11804 -> 12336, cli_maxrss_kib 11656 -> 11632, frees 10250 -> 10264, malloc_bytes 3631232 -> 3738108, mallocs 12546 -> 12995, read_bytes.store 10164 -> 10141, read_calls.ipc 190 -> 188, write_bytes.jobs 5236 -> 5240, write_bytes.store 63586 -> 67705, write_calls.store 37 -> 39
- codex move: cli_cpu_us 13264 -> 12611, cli_maxrss_kib 12224 -> 12272, frees 6771 -> 6798, malloc_bytes 1994252 -> 1996732, mallocs 8223 -> 8255, read_bytes.jobs 4064 -> 4090, read_bytes.store 1288 -> 1284
- codex resume: cli_cpu_us 17430 -> 18116, cli_maxrss_kib 14832 -> 14792, cmd:dash holding codex 0 -> 1, cmd:sh holding codex 1 -> 0, malloc_bytes 4254835 -> 4254782, mallocs 11042 -> 11040, read_bytes.ipc 4101 -> 4102, read_bytes.jobs 10611 -> 10614, read_bytes.state 196 -> 270, read_bytes.store 11704 -> 11688, read_calls.ipc 164 -> 162, read_calls.state 6 -> 8, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 1712 -> 1714, write_bytes.jobs 5214 -> 5238, write_bytes.store 3723 -> 3722, write_calls.ipc 28 -> 29
- codex pull: cli_cpu_us 22938 -> 20523, cli_maxrss_kib 14776 -> 14960, cmd:dash holding codex 0 -> 1, cmd:sh holding codex 1 -> 0, frees 23657 -> 23612, malloc_bytes 9112920 -> 9058808, mallocs 29241 -> 29006, opens 508 -> 506, read_bytes.ipc 10357 -> 10358, read_bytes.jobs 18130 -> 18136, read_bytes.state 396 -> 322, read_bytes.store 25242 -> 25210, read_calls.ipc 488 -> 496, read_calls.state 10 -> 8, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 4056 -> 4058, write_bytes.jobs 11880 -> 11891, write_bytes.store 72286 -> 76408, write_calls.ipc 80 -> 81, write_calls.store 44 -> 46
- cold start: cli_cpu_us 177449 -> 184225, cli_maxrss_kib 10856 -> 10752, cli_processes 16 -> 18, frees 13914 -> 14674, malloc_bytes 5301012 -> 5593774, mallocs 17051 -> 17944, opens 22 -> 21, read_bytes.state 154 -> 50, read_calls.state 5 -> 2, write_bytes.ipc 2744 -> 2945, write_calls.ipc 32 -> 34
- restart: cli_cpu_us 153878 -> 157854, cli_maxrss_kib 11224 -> 11192, frees 16552 -> 16086, malloc_bytes 5491947 -> 5424853, mallocs 21205 -> 20674, opens 246 -> 236, read_bytes.state 736 -> 766, read_bytes.store 360536 -> 366716, read_calls.state 18 -> 19, read_calls.store 90 -> 91, write_bytes.ipc 5638 -> 5038, write_calls.ipc 80 -> 71

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 20824 | 21408 | 20776 | 21664 | 1.007 | 1.003 to 1.013 |
| daemon b | 27248 | 28672 | 27304 | 28992 | 1.002 | 0.971 to 1.014 |
