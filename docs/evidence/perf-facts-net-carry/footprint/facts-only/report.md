### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 15 | 152.7 | 153.7 | 0.999 | 0.987 to 1.010 | not shown |
| acp launch | 15 | 61.0 | 60.9 | 1.005 | 0.987 to 1.021 | not shown, median above 1 |
| acp approve | 15 | 15.3 | 15.8 | 0.988 | 0.949 to 1.056 | not shown |
| claude new | 15 | 137.0 | 136.2 | 0.999 | 0.968 to 1.016 | not shown |
| claude launch | 15 | 109.0 | 108.2 | 0.988 | 0.945 to 1.088 | not shown |
| claude approve | 15 | 17.5 | 17.2 | 1.006 | 0.937 to 1.036 | not shown, median above 1 |
| codex new | 15 | 137.9 | 137.3 | 1.006 | 0.959 to 1.033 | not shown, median above 1 |
| codex launch | 15 | 120.9 | 122.8 | 1.025 | 0.916 to 1.103 | not shown, median above 1 |
| codex approve | 15 | 17.4 | 17.6 | 1.011 | 0.995 to 1.029 | not shown, median above 1 |
| ledger append | 15 | 17.0 | 16.8 | 0.994 | 0.941 to 1.019 | not shown |
| cli version | 15 | 3.4 | 3.2 | 0.940 | 0.862 to 1.004 | not shown |
| cli ls | 15 | 5.4 | 5.5 | 0.971 | 0.918 to 1.084 | not shown |
| acp suspend | 15 | 354.6 | 352.1 | 1.002 | 0.989 to 1.013 | not shown, median above 1 |
| acp move | 15 | 159.8 | 155.9 | 0.990 | 0.959 to 1.024 | not shown |
| acp resume | 15 | 653.0 | 652.9 | 1.007 | 0.994 to 1.021 | not shown, median above 1 |
| acp pull | 15 | 1152.8 | 1161.7 | 0.994 | 0.970 to 1.008 | not shown |
| claude suspend | 15 | 428.6 | 426.5 | 0.988 | 0.985 to 1.013 | not shown |
| claude move | 15 | 162.7 | 160.0 | 1.013 | 0.956 to 1.080 | not shown, median above 1 |
| claude resume | 15 | 850.0 | 836.1 | 1.004 | 0.974 to 1.014 | not shown, median above 1 |
| claude pull | 15 | 1428.1 | 1446.7 | 1.000 | 0.996 to 1.011 | not shown, median above 1 |
| codex suspend | 15 | 646.1 | 647.8 | 0.993 | 0.988 to 1.010 | not shown |
| codex move | 15 | 159.1 | 156.9 | 0.993 | 0.962 to 1.002 | not shown |
| codex resume | 15 | 1064.6 | 1065.6 | 1.004 | 0.978 to 1.012 | not shown, median above 1 |
| codex pull | 15 | 1909.6 | 1855.4 | 0.998 | 0.993 to 1.014 | not shown |
| cold start | 15 | 49.3 | 57.3 | 1.032 | 0.986 to 1.229 | not shown, median above 1 |
| restart | 15 | 359.5 | 368.8 | 1.006 | 0.996 to 1.028 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38519 -> 38516 | 14768 -> 14712 | 20.0 -> 24.2 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4748 -> 4735 | 15424 -> 15384 | 13.3 -> 13.0 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1236 -> 1220 | 10944 -> 10792 | 9.6 -> 9.2 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1690 -> 1682 | 11096 -> 10976 | 10.7 -> 10.3 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1100 -> 1098 | 3626 -> 3628 | 14800 -> 14768 | 12.5 -> 12.3 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1221 | 10976 -> 10784 | 9.6 -> 9.2 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1702 -> 1684 | 11000 -> 10944 | 9.5 -> 10.0 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3966 -> 3964 | 14696 -> 14728 | 13.2 -> 12.7 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1224 -> 1218 | 10944 -> 10808 | 9.4 -> 9.6 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1216 -> 1216 | 10976 -> 10800 | 9.5 -> 9.5 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4864 | 5.5 -> 5.8 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1522 -> 1514 | 11008 -> 10952 | 8.5 -> 9.0 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 32158 -> 32109 | 12841 -> 12814 | 11648 -> 11560 | 9.8 -> 9.9 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5280 -> 5281 | 8362 -> 8172 | 12304 -> 12224 | 10.6 -> 11.3 |
| acp resume | 31 -> 31 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 14510 -> 14434 | 11050 -> 11031 | 15664 -> 15600 | 12.4 -> 13.0 |
| acp pull | 110 -> 110 | 80 -> 81 | 59 -> 59 | 1376 -> 1376 | 32096 -> 31944 | 28156 -> 28376 | 15744 -> 15696 | 13.8 -> 13.9 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13254 -> 13240 | 15528 -> 15530 | 11656 -> 11584 | 9.8 -> 10.2 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5315 -> 5316 | 8218 -> 8015 | 12272 -> 12216 | 10.6 -> 10.5 |
| claude resume | 33 -> 33 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 18778 -> 18776 | 10890 -> 10876 | 15296 -> 15344 | 16.4 -> 15.4 |
| claude pull | 113 -> 113 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 38868 -> 38869 | 32443 -> 32182 | 15464 -> 15376 | 15.8 -> 15.5 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14269 -> 14296 | 12542 -> 12981 | 11696 -> 11544 | 9.9 -> 10.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5338 -> 5376 | 8228 -> 8026 | 12328 -> 12184 | 10.8 -> 10.6 |
| codex resume | 34 -> 34 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 22459 -> 22476 | 11046 -> 11030 | 15352 -> 15328 | 14.6 -> 14.8 |
| codex pull | 116 -> 116 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 43419 -> 43518 | 28753 -> 28502 | 15472 -> 15448 | 15.8 -> 14.8 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 5572 -> 5952 | 10832 -> 10704 | 31.9 -> 32.7 |
| restart | 0 -> 0 | 35 -> 35 | 0 -> 0 | 463 -> 463 | 393254 -> 387044 | 14785 -> 15537 | 11224 -> 11064 | 70.8 -> 73.2 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 19982 -> 24244, cli_maxrss_kib 14768 -> 14712, frees 27790 -> 27784, malloc_bytes 13432062 -> 13431241, mallocs 38519 -> 38516, opens 11 -> 10, read_calls.ipc 6 -> 4
- acp launch: cli_cpu_us 13348 -> 12976, cli_maxrss_kib 15424 -> 15384, frees 3776 -> 3766, malloc_bytes 827900 -> 826132, mallocs 4748 -> 4735, read_calls.ipc 11 -> 12
- acp approve: cli_cpu_us 9568 -> 9217, cli_maxrss_kib 10944 -> 10792, frees 1076 -> 1062, malloc_bytes 328091 -> 326022, mallocs 1236 -> 1220
- claude new: cli_cpu_us 10694 -> 10256, cli_maxrss_kib 11096 -> 10976, frees 1425 -> 1417, malloc_bytes 393297 -> 392156, mallocs 1690 -> 1682, opens 11 -> 10, read_calls.ipc 6 -> 4
- claude launch: cli_cpu_us 12548 -> 12288, cli_maxrss_kib 14800 -> 14768, frees 3218 -> 3220, malloc_bytes 614492 -> 613728, mallocs 3626 -> 3628, read_bytes.jobs 926 -> 924, write_bytes.jobs 928 -> 926
- claude approve: cli_cpu_us 9566 -> 9168, cli_maxrss_kib 10976 -> 10784, frees 1050 -> 1063, malloc_bytes 325611 -> 325990, mallocs 1208 -> 1221
- codex new: cli_cpu_us 9530 -> 10016, cli_maxrss_kib 11000 -> 10944, frees 1438 -> 1420, malloc_bytes 393605 -> 391872, mallocs 1702 -> 1684, opens 11 -> 10, read_calls.ipc 6 -> 4
- codex launch: cli_cpu_us 13200 -> 12715, cli_maxrss_kib 14696 -> 14728, frees 3538 -> 3536, malloc_bytes 708824 -> 707475, mallocs 3966 -> 3964
- codex approve: cli_cpu_us 9400 -> 9568, cli_maxrss_kib 10944 -> 10808, frees 1066 -> 1060, malloc_bytes 326859 -> 325830, mallocs 1224 -> 1218
- ledger append: cli_cpu_us 9478 -> 9516, cli_maxrss_kib 10976 -> 10800, frees 1057 -> 1056, malloc_bytes 326059 -> 325830, mallocs 1216 -> 1216
- cli version: cli_cpu_us 5518 -> 5757, cli_maxrss_kib 4928 -> 4864, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 8490 -> 8972, cli_maxrss_kib 11008 -> 10952, frees 1329 -> 1322, malloc_bytes 367334 -> 366401, mallocs 1522 -> 1514
- acp suspend: cli_cpu_us 9788 -> 9906, cli_maxrss_kib 11648 -> 11560, frees 9746 -> 9730, malloc_bytes 3521118 -> 3509634, mallocs 12841 -> 12814, opens 142 -> 139, read_bytes.store 29768 -> 29720, read_calls.ipc 187 -> 183, write_bytes.jobs 3587 -> 3590, write_bytes.store 87701 -> 87679
- acp move: cli_cpu_us 10632 -> 11296, cli_maxrss_kib 12304 -> 12224, frees 6862 -> 6713, malloc_bytes 2008069 -> 1985643, mallocs 8362 -> 8172, opens 73 -> 33, read_bytes.jobs 4107 -> 4115, read_bytes.store 1174 -> 1166, read_calls.ipc 124 -> 76, write_bytes.ipc 2510 -> 2510
- acp resume: cli_cpu_us 12434 -> 12975, cli_maxrss_kib 15664 -> 15600, frees 8914 -> 8895, malloc_bytes 4143945 -> 4140857, mallocs 11050 -> 11031, opens 209 -> 206, read_bytes.jobs 3560 -> 3560, read_bytes.state 196 -> 166, read_bytes.store 10754 -> 10708, read_calls.ipc 156 -> 152, read_calls.state 6 -> 5, write_bytes.jobs 3555 -> 3554, write_bytes.store 3723 -> 3721
- acp pull: cli_cpu_us 13826 -> 13896, cli_maxrss_kib 15744 -> 15696, frees 22778 -> 22616, fullfsync 80 -> 81, malloc_bytes 8563598 -> 8650392, mallocs 28156 -> 28376, opens 410 -> 366, read_bytes.ipc 10694 -> 10694, read_bytes.jobs 8539 -> 8546, read_bytes.store 23383 -> 23224, read_calls.ipc 482 -> 428, syncs.store 17 -> 18, write_bytes.ipc 4616 -> 4616, write_bytes.jobs 8394 -> 8404, write_bytes.store 55308 -> 55264
- claude suspend: cli_cpu_us 9766 -> 10220, cli_maxrss_kib 11656 -> 11584, frees 12409 -> 12418, malloc_bytes 3548000 -> 3546688, mallocs 15528 -> 15530, opens 145 -> 142, read_bytes.store 9772 -> 9758, read_calls.ipc 185 -> 181, write_bytes.jobs 4484 -> 4480, write_bytes.store 63196 -> 50827, write_calls.store 37 -> 31
- claude move: cli_cpu_us 10560 -> 10484, cli_maxrss_kib 12272 -> 12216, frees 6762 -> 6596, malloc_bytes 1994502 -> 1970224, mallocs 8218 -> 8015, opens 73 -> 33, read_bytes.jobs 4068 -> 4066, read_bytes.store 1247 -> 1250, read_calls.ipc 124 -> 76, write_bytes.ipc 2362 -> 2362
- claude resume: cli_cpu_us 16434 -> 15395, cli_maxrss_kib 15296 -> 15344, frees 9005 -> 8992, malloc_bytes 4147544 -> 4144840, mallocs 10890 -> 10876, opens 244 -> 242, read_bytes.ipc 3951 -> 3949, read_bytes.store 11190 -> 11188, read_calls.ipc 162 -> 158, write_bytes.jobs 4424 -> 4416, write_bytes.store 3725 -> 3722
- claude pull: cli_cpu_us 15818 -> 15508, cli_maxrss_kib 15464 -> 15376, frees 26252 -> 26040, malloc_bytes 8853266 -> 8821216, mallocs 32443 -> 32182, opens 455 -> 409, read_bytes.ipc 10194 -> 10194, read_bytes.jobs 14328 -> 14335, read_bytes.store 24292 -> 24286, read_calls.ipc 492 -> 434, write_bytes.ipc 4122 -> 4123, write_bytes.jobs 10327 -> 10343, write_bytes.store 63676 -> 55419, write_calls.store 40 -> 36
- codex suspend: cli_cpu_us 9938 -> 10302, cli_maxrss_kib 11696 -> 11544, frees 10243 -> 10256, malloc_bytes 3622402 -> 3735880, mallocs 12542 -> 12981, opens 152 -> 149, read_bytes.store 10084 -> 10112, read_calls.ipc 190 -> 186, write_bytes.jobs 5233 -> 5250, write_bytes.store 55329 -> 67689, write_calls.store 33 -> 39
- codex move: cli_cpu_us 10778 -> 10554, cli_maxrss_kib 12328 -> 12184, frees 6774 -> 6614, malloc_bytes 1994534 -> 1969754, mallocs 8228 -> 8026, opens 73 -> 33, read_bytes.jobs 4066 -> 4098, read_bytes.store 1272 -> 1278, read_calls.ipc 124 -> 76, write_bytes.ipc 2362 -> 2362
- codex resume: cli_cpu_us 14600 -> 14807, cli_maxrss_kib 15352 -> 15328, frees 9180 -> 9164, malloc_bytes 4255476 -> 4252470, mallocs 11046 -> 11030, opens 289 -> 287, read_bytes.ipc 4102 -> 4100, read_bytes.jobs 10616 -> 10618, read_bytes.store 11646 -> 11662, read_calls.ipc 166 -> 161, write_bytes.jobs 5230 -> 5240, write_bytes.store 3722 -> 3724
- codex pull: cli_cpu_us 15796 -> 14824, cli_maxrss_kib 15472 -> 15448, frees 23606 -> 23400, malloc_bytes 8993918 -> 8963362, mallocs 28753 -> 28502, opens 505 -> 460, read_bytes.jobs 18136 -> 18122, read_bytes.state 174 -> 248, read_bytes.store 25109 -> 25148, read_calls.ipc 499 -> 442, read_calls.state 4 -> 6, write_bytes.ipc 4056 -> 4057, write_bytes.jobs 11894 -> 11868, write_bytes.store 80522 -> 80516
- cold start: cli_cpu_us 31874 -> 32726, cli_maxrss_kib 10832 -> 10704, cli_processes 4 -> 4, frees 4140 -> 4454, malloc_bytes 1530063 -> 1670927, mallocs 5572 -> 5952, write_bytes.ipc 396 -> 487, write_calls.ipc 6 -> 7
- restart: cli_cpu_us 70750 -> 73174, cli_maxrss_kib 11224 -> 11064, cli_processes 8 -> 8, frees 11017 -> 11674, malloc_bytes 3704590 -> 3886194, mallocs 14785 -> 15537, opens 216 -> 221, read_bytes.state 766 -> 736, read_bytes.store 372896 -> 366716, read_calls.state 19 -> 18, read_calls.store 92 -> 91, write_bytes.ipc 2933 -> 3316, write_calls.ipc 42 -> 48

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 5120 | 5392 | 5104 | 5408 | 0.997 | 0.979 to 1.006 |
| daemon a peak | 5120 | 5392 | 5104 | 5408 | 0.997 | 0.979 to 1.006 |
| daemon b | 12464 | 13552 | 12368 | 13504 | 0.995 | 0.992 to 1.000 |
| daemon b peak | 12464 | 13712 | 12480 | 13552 | 0.995 | 0.992 to 1.001 |
