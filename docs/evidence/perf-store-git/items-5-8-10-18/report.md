### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38957 -> 38521 | 14832 -> 14816 |
| acp launch | 4 -> 4 | 15 -> 12 | 0 -> 0 | 135 -> 135 | 1183 -> 683 | 4119 -> 4920 | 14432 -> 15472 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1209 | 10496 -> 10800 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1672 -> 1690 | 10784 -> 10912 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1104 -> 1099 | 3528 -> 3622 | 14224 -> 14704 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1209 | 10496 -> 10800 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1671 -> 1688 | 10800 -> 10896 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3851 -> 3960 | 14096 -> 14688 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1191 -> 1208 | 10560 -> 10800 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1195 -> 1210 | 10544 -> 10800 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4944 -> 4848 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1537 -> 1552 | 10816 -> 10992 |
| acp suspend | 36 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40030 -> 41165 | 15024 -> 12735 | 11296 -> 11568 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5227 -> 5268 | 8354 -> 8434 | 11968 -> 12304 |
| acp resume | 32 -> 21 | 24 -> 24 | 51 -> 52 | 167 -> 167 | 16240 -> 16391 | 11172 -> 9669 | 14624 -> 15648 |
| acp pull | 111 -> 98 | 81 -> 81 | 59 -> 60 | 1376 -> 1376 | 33276 -> 34979 | 30390 -> 26526 | 14736 -> 15824 |
| claude suspend | 35 -> 34 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13285 -> 14326 | 19221 -> 16273 | 11264 -> 11584 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5406 -> 5317 | 8254 -> 8287 | 11888 -> 12240 |
| claude resume | 34 -> 23 | 24 -> 24 | 55 -> 56 | 148 -> 148 | 21671 -> 22073 | 11099 -> 9568 | 14752 -> 15360 |
| claude pull | 114 -> 101 | 83 -> 83 | 63 -> 64 | 1338 -> 1338 | 41617 -> 43182 | 36331 -> 31677 | 14928 -> 15376 |
| codex suspend | 37 -> 36 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14446 -> 15552 | 16258 -> 12460 | 11280 -> 11616 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5486 -> 5381 | 8231 -> 8281 | 11952 -> 12320 |
| codex resume | 35 -> 24 | 24 -> 24 | 59 -> 60 | 148 -> 148 | 25790 -> 26148 | 11233 -> 9701 | 14736 -> 15344 |
| codex pull | 117 -> 104 | 82 -> 82 | 67 -> 68 | 1338 -> 1338 | 47055 -> 48377 | 32664 -> 28057 | 13632 -> 14240 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 13803 -> 18368 | 10448 -> 10688 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 505745 -> 497505 | 25336 -> 23663 | 11040 -> 11184 |


### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_maxrss_kib 14832 -> 14816, frees 28129 -> 27789, malloc_bytes 13540008 -> 13431907, mallocs 38957 -> 38521
- acp launch: cli_maxrss_kib 14432 -> 15472, cmd:fabric-fake-agent 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 3354 -> 3769, fullfsync 15 -> 12, malloc_bytes 717249 -> 877092, mallocs 4119 -> 4920, opens 40 -> 37, read_bytes.jobs 1009 -> 509, read_calls.jobs 12 -> 10, renames 6 -> 5, spawn:fabric-fake-agent 1 -> 0, spawn:sh 0 -> 1, syncs.jobs 14 -> 11, write_bytes.ipc 969 -> 1025, write_bytes.jobs 1031 -> 577, write_calls.ipc 11 -> 14, write_calls.jobs 6 -> 5
- acp approve: cli_maxrss_kib 10496 -> 10800, frees 1033 -> 1049, malloc_bytes 323639 -> 325886, mallocs 1192 -> 1209
- claude new: cli_maxrss_kib 10784 -> 10912, frees 1407 -> 1425, malloc_bytes 391277 -> 393456, mallocs 1672 -> 1690
- claude launch: cli_maxrss_kib 14224 -> 14704, cmd:claude 1 -> 0, cmd:sh holding claude 0 -> 1, frees 3131 -> 3215, malloc_bytes 604543 -> 614015, mallocs 3528 -> 3622, read_bytes.jobs 930 -> 925, spawn:claude 1 -> 0, spawn:sh 0 -> 1, write_bytes.jobs 933 -> 928
- claude approve: cli_maxrss_kib 10496 -> 10800, frees 1033 -> 1051, malloc_bytes 323415 -> 325637, mallocs 1192 -> 1209
- codex new: cli_maxrss_kib 10800 -> 10896, frees 1408 -> 1424, malloc_bytes 390769 -> 393192, mallocs 1671 -> 1688
- codex launch: cli_maxrss_kib 14096 -> 14688, cmd:codex 1 -> 0, cmd:sh holding codex 0 -> 1, frees 3427 -> 3532, malloc_bytes 697139 -> 708251, mallocs 3851 -> 3960, spawn:codex 1 -> 0, spawn:sh 0 -> 1
- codex approve: cli_maxrss_kib 10560 -> 10800, frees 1033 -> 1049, malloc_bytes 323383 -> 325550, mallocs 1191 -> 1208
- ledger append: cli_maxrss_kib 10544 -> 10800, frees 1035 -> 1049, malloc_bytes 324279 -> 325998, mallocs 1195 -> 1210
- cli version: cli_maxrss_kib 4944 -> 4848, malloc_bytes 88361 -> 88364
- cli ls: cli_maxrss_kib 10816 -> 10992, frees 1339 -> 1353, malloc_bytes 368970 -> 371265, mallocs 1537 -> 1552
- acp suspend: cli_maxrss_kib 11296 -> 11568, cmd:git bundle 3 -> 2, frees 10358 -> 9671, malloc_bytes 3568149 -> 3518711, mallocs 15024 -> 12735, read_bytes.ipc 5269 -> 5199, read_bytes.store 37641 -> 38776, read_calls.ipc 187 -> 181, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3581 -> 3576, write_bytes.store 95728 -> 95947
- acp move: cli_maxrss_kib 11968 -> 12304, frees 6852 -> 6932, malloc_bytes 2018896 -> 2026227, mallocs 8354 -> 8434, read_bytes.jobs 4107 -> 4091, read_bytes.store 1120 -> 1177, write_bytes.ipc 2698 -> 2510
- acp resume: cli_maxrss_kib 14624 -> 15648, cmd:fabric-fake-agent 1 -> 0, cmd:git bundle 10 -> 5, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:sh holding fabric-fake-agent 0 -> 1, frees 8969 -> 7778, fsync 51 -> 52, malloc_bytes 4355045 -> 4134231, mallocs 11172 -> 9669, opens 214 -> 203, read_bytes.ipc 4937 -> 3025, read_bytes.jobs 3555 -> 3557, read_bytes.outside 554 -> 691, read_bytes.state 270 -> 122, read_bytes.store 12415 -> 12712, read_calls.ipc 171 -> 92, read_calls.outside 13 -> 15, read_calls.state 8 -> 4, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 13, spawn:sh 0 -> 1, spawns 32 -> 21, syncs.outside 43 -> 44, write_bytes.ipc 2046 -> 2175, write_bytes.jobs 3554 -> 3552, write_bytes.outside 483 -> 640, write_bytes.store 4536 -> 4526, write_calls.ipc 28 -> 29, write_calls.outside 11 -> 12
- acp pull: cli_maxrss_kib 14736 -> 15824, cmd:fabric-fake-agent 2 -> 1, cmd:git bundle 15 -> 8, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:sh holding fabric-fake-agent 0 -> 1, frees 23381 -> 21455, fsync 59 -> 60, malloc_bytes 8833046 -> 8531210, mallocs 30390 -> 26526, opens 415 -> 407, read_bytes.ipc 11101 -> 8786, read_bytes.jobs 8539 -> 8545, read_bytes.outside 1632 -> 1769, read_bytes.state 174 -> 322, read_bytes.store 24563 -> 26112, read_calls.ipc 493 -> 407, read_calls.outside 31 -> 33, read_calls.state 4 -> 8, read_calls.store 217 -> 218, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 45, spawn:sh 0 -> 1, spawns 111 -> 98, syncs.outside 48 -> 49, write_bytes.ipc 4625 -> 4715, write_bytes.jobs 8392 -> 8402, write_bytes.outside 747 -> 904, write_bytes.store 59991 -> 60201, write_calls.ipc 84 -> 85, write_calls.outside 12 -> 13
- claude suspend: cli_maxrss_kib 11264 -> 11584, cmd:git bundle 3 -> 2, frees 13985 -> 12966, malloc_bytes 3668067 -> 3585250, mallocs 19221 -> 16273, read_bytes.ipc 5237 -> 5167, read_bytes.store 9522 -> 10563, read_calls.ipc 185 -> 179, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4750 -> 4760, write_bytes.store 79468 -> 54959, write_calls.store 45 -> 33
- claude move: cli_maxrss_kib 11888 -> 12240, frees 6783 -> 6818, malloc_bytes 2009787 -> 2013714, mallocs 8254 -> 8287, opens 75 -> 73, read_bytes.jobs 4050 -> 4066, read_bytes.state 148 -> 0, read_bytes.store 1208 -> 1251, read_calls.state 4 -> 0
- claude resume: cli_maxrss_kib 14752 -> 15360, cmd:claude 3 -> 2, cmd:git bundle 10 -> 5, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:sh holding claude 0 -> 1, frees 9137 -> 7925, fsync 55 -> 56, malloc_bytes 4372422 -> 4145199, mallocs 11099 -> 9568, opens 248 -> 241, read_bytes.ipc 4416 -> 2503, read_bytes.jobs 8584 -> 8587, read_bytes.outside 3026 -> 3163, read_bytes.state 122 -> 270, read_bytes.store 12965 -> 13216, read_calls.ipc 174 -> 102, read_calls.outside 16 -> 18, read_calls.state 4 -> 8, spawn:claude 3 -> 2, spawn:git 24 -> 13, spawn:sh 0 -> 1, spawns 34 -> 23, syncs.outside 43 -> 44, write_bytes.ipc 1852 -> 1878, write_bytes.jobs 4701 -> 4715, write_bytes.outside 483 -> 640, write_bytes.store 4532 -> 4526, write_calls.outside 11 -> 12
- claude pull: cli_maxrss_kib 14928 -> 15376, cmd:claude 8 -> 7, cmd:git bundle 15 -> 8, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:sh holding claude 0 -> 1, frees 27944 -> 25586, fsync 63 -> 64, malloc_bytes 9184065 -> 8850814, mallocs 36331 -> 31677, opens 458 -> 450, read_bytes.ipc 10601 -> 8286, read_bytes.jobs 15719 -> 15746, read_bytes.outside 5735 -> 5872, read_bytes.state 174 -> 322, read_bytes.store 25724 -> 27114, read_calls.ipc 507 -> 413, read_calls.outside 33 -> 35, read_calls.state 4 -> 8, read_calls.store 217 -> 218, spawn:claude 8 -> 7, spawn:git 58 -> 45, spawn:sh 0 -> 1, spawns 114 -> 101, syncs.outside 48 -> 49, write_bytes.ipc 4279 -> 4222, write_bytes.jobs 10880 -> 10914, write_bytes.outside 617 -> 774, write_bytes.store 68385 -> 68575, write_calls.outside 12 -> 13
- codex suspend: cli_maxrss_kib 11280 -> 11616, cmd:git bundle 3 -> 2, frees 11737 -> 10187, malloc_bytes 3738934 -> 3651676, mallocs 16258 -> 12460, opens 152 -> 153, read_bytes.ipc 5251 -> 5181, read_bytes.state 87 -> 174, read_bytes.store 9865 -> 10884, read_calls.ipc 190 -> 184, read_calls.state 2 -> 4, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5643 -> 5648, write_bytes.store 88088 -> 80043, write_calls.store 49 -> 45
- codex move: cli_maxrss_kib 11952 -> 12320, frees 6778 -> 6822, malloc_bytes 2007250 -> 2012887, mallocs 8231 -> 8281, opens 75 -> 73, read_bytes.jobs 4098 -> 4106, read_bytes.state 148 -> 0, read_bytes.store 1240 -> 1275, read_calls.state 4 -> 0
- codex resume: cli_maxrss_kib 14736 -> 15344, cmd:codex 3 -> 2, cmd:git bundle 10 -> 5, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 1 -> 0, cmd:sh holding codex 0 -> 1, frees 9294 -> 8078, fsync 59 -> 60, malloc_bytes 4478328 -> 4250870, mallocs 11233 -> 9701, opens 292 -> 285, read_bytes.ipc 4566 -> 2653, read_bytes.jobs 12199 -> 12195, read_bytes.outside 9297 -> 9434, read_bytes.state 122 -> 270, read_bytes.store 13469 -> 13683, read_calls.ipc 179 -> 107, read_calls.outside 15 -> 17, read_calls.state 4 -> 8, spawn:codex 3 -> 2, spawn:git 24 -> 13, spawn:sh 0 -> 1, spawns 35 -> 24, syncs.outside 43 -> 44, write_bytes.ipc 1786 -> 1812, write_bytes.jobs 5630 -> 5623, write_bytes.outside 483 -> 640, write_bytes.store 4535 -> 4528, write_calls.outside 11 -> 12
- codex pull: cli_maxrss_kib 13632 -> 14240, cmd:codex 9 -> 8, cmd:git bundle 15 -> 8, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 3 -> 2, cmd:sh holding codex 0 -> 1, frees 25214 -> 22394, fsync 67 -> 68, malloc_bytes 9334068 -> 9211787, mallocs 32664 -> 28057, opens 511 -> 501, read_bytes.ipc 10766 -> 8450, read_bytes.jobs 20098 -> 20111, read_bytes.outside 18389 -> 18526, read_bytes.store 26635 -> 27944, read_calls.ipc 501 -> 417, read_calls.outside 33 -> 35, read_calls.store 219 -> 220, spawn:codex 9 -> 8, spawn:git 58 -> 45, spawn:sh 0 -> 1, spawns 117 -> 104, syncs.outside 45 -> 46, write_bytes.ipc 4213 -> 4156, write_bytes.jobs 12655 -> 12672, write_bytes.outside 504 -> 661, write_bytes.store 85241 -> 93665, write_calls.outside 11 -> 12, write_calls.store 51 -> 55
- cold start: cli_maxrss_kib 10448 -> 10688, cli_processes 13 -> 18, frees 11147 -> 15035, malloc_bytes 4263436 -> 5735205, mallocs 13803 -> 18368, write_bytes.ipc 2116 -> 3021, write_calls.ipc 25 -> 35
- restart: cli_maxrss_kib 11040 -> 11184, frees 19637 -> 18138, malloc_bytes 6519282 -> 6152930, mallocs 25336 -> 23663, opens 258 -> 284, read_bytes.store 482076 -> 473836, read_calls.store 119 -> 117, write_bytes.ipc 6143 -> 6275, write_calls.ipc 85 -> 89
