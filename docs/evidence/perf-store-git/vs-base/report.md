### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38520 -> 38522 | 15088 -> 14736 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4556 -> 4530 | 15456 -> 15648 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1209 -> 1210 | 10992 -> 10976 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1691 -> 1691 | 11040 -> 11056 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1103 | 3623 -> 3626 | 14864 -> 14848 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1208 | 10976 -> 10880 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1687 -> 1689 | 11056 -> 11024 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3963 -> 3961 | 14704 -> 14768 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1207 -> 1207 | 10928 -> 10912 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1208 -> 1211 | 10928 -> 10960 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4880 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1558 -> 1553 | 11024 -> 11104 |
| acp suspend | 36 -> 35 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40564 -> 41278 | 12866 -> 12734 | 11680 -> 11728 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5301 -> 5297 | 8443 -> 8410 | 12288 -> 12400 |
| acp resume | 33 -> 18 | 24 -> 24 | 51 -> 61 | 167 -> 167 | 16441 -> 21995 | 11552 -> 8825 | 15648 -> 15776 |
| acp pull | 112 -> 94 | 81 -> 81 | 59 -> 72 | 1376 -> 1376 | 34291 -> 41068 | 28710 -> 25349 | 14416 -> 15888 |
| claude suspend | 35 -> 34 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13581 -> 14344 | 16400 -> 16258 | 11648 -> 11760 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5335 -> 5317 | 8284 -> 8306 | 12272 -> 12464 |
| claude resume | 35 -> 20 | 24 -> 24 | 55 -> 65 | 148 -> 148 | 22123 -> 27246 | 11453 -> 8698 | 15328 -> 15456 |
| claude pull | 115 -> 97 | 83 -> 83 | 63 -> 76 | 1338 -> 1338 | 42473 -> 48915 | 33850 -> 30508 | 15456 -> 15568 |
| codex suspend | 37 -> 36 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14903 -> 15565 | 12619 -> 12470 | 11664 -> 11712 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5391 -> 5378 | 8281 -> 8272 | 12272 -> 12432 |
| codex resume | 36 -> 21 | 24 -> 24 | 59 -> 69 | 148 -> 148 | 26245 -> 31301 | 11586 -> 8819 | 15280 -> 15424 |
| codex pull | 118 -> 100 | 82 -> 82 | 67 -> 80 | 1338 -> 1338 | 47785 -> 54180 | 29379 -> 26020 | 14080 -> 15600 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45222 -> 45222 | 12226 -> 10428 | 10848 -> 10816 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 501625 -> 513985 | 26009 -> 26736 | 11264 -> 11296 |


### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_maxrss_kib 15088 -> 14736, frees 27788 -> 27789, malloc_bytes 13431840 -> 13432419, mallocs 38520 -> 38522
- acp launch: cli_maxrss_kib 15456 -> 15648, frees 3769 -> 3743, malloc_bytes 777492 -> 776169, mallocs 4556 -> 4530
- acp approve: cli_maxrss_kib 10992 -> 10976, frees 1049 -> 1051, malloc_bytes 325883 -> 325886, mallocs 1209 -> 1210
- claude new: cli_maxrss_kib 11040 -> 11056, frees 1427 -> 1425, malloc_bytes 393389 -> 393476
- claude launch: cli_maxrss_kib 14864 -> 14848, frees 3215 -> 3219, malloc_bytes 614526 -> 614760, mallocs 3623 -> 3626, read_bytes.jobs 927 -> 929, write_bytes.jobs 930 -> 932
- claude approve: cli_maxrss_kib 10976 -> 10880, malloc_bytes 325355 -> 325422
- codex new: cli_maxrss_kib 11056 -> 11024, frees 1423 -> 1424, malloc_bytes 393125 -> 393468, mallocs 1687 -> 1689
- codex launch: cli_maxrss_kib 14704 -> 14768, frees 3535 -> 3533, malloc_bytes 708442 -> 708315, mallocs 3963 -> 3961, read_calls.ipc 13 -> 12
- codex approve: cli_maxrss_kib 10928 -> 10912, malloc_bytes 325291 -> 325294
- ledger append: cli_maxrss_kib 10928 -> 10960, malloc_bytes 325547 -> 325742, mallocs 1208 -> 1211
- cli version: cli_maxrss_kib 4928 -> 4880, malloc_bytes 88361 -> 88364
- cli ls: cli_maxrss_kib 11024 -> 11104, frees 1359 -> 1355, malloc_bytes 371646 -> 370881, mallocs 1558 -> 1553
- acp suspend: cli_maxrss_kib 11680 -> 11728, cmd:git bundle 3 -> 2, frees 9781 -> 9673, malloc_bytes 3514110 -> 3502176, mallocs 12866 -> 12734, opens 144 -> 142, read_bytes.ipc 5325 -> 5199, read_bytes.state 235 -> 87, read_bytes.store 38027 -> 38889, read_calls.ipc 187 -> 180, read_calls.state 6 -> 2, read_calls.store 74 -> 75, spawn:git 30 -> 29, spawns 36 -> 35, write_bytes.jobs 3583 -> 3588, write_bytes.store 95959 -> 95985
- acp move: cli_maxrss_kib 12288 -> 12400, frees 6937 -> 6910, malloc_bytes 2026147 -> 2023476, mallocs 8443 -> 8410, read_bytes.store 1186 -> 1182
- acp resume: cli_maxrss_kib 15648 -> 15776, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 2 -> 0, frees 9316 -> 7062, fsync 51 -> 61, malloc_bytes 4399148 -> 3687783, mallocs 11552 -> 8825, opens 213 -> 236, read_bytes.ipc 4937 -> 2412, read_bytes.jobs 3562 -> 3557, read_bytes.outside 554 -> 691, read_bytes.store 12757 -> 18316, read_calls.ipc 170 -> 78, read_calls.outside 13 -> 15, read_calls.store 125 -> 111, spawn:git 25 -> 10, spawns 33 -> 18, syncs.outside 43 -> 53, unlinks 23 -> 20, write_bytes.ipc 2076 -> 2175, write_bytes.jobs 3560 -> 3548, write_bytes.outside 483 -> 640, write_bytes.store 4537 -> 2480, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- acp pull: cli_maxrss_kib 14416 -> 15888, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 4 -> 2, frees 23230 -> 20462, fsync 59 -> 72, malloc_bytes 8832918 -> 7847121, mallocs 28710 -> 25349, opens 418 -> 443, read_bytes.ipc 11158 -> 8047, read_bytes.jobs 8548 -> 8536, read_bytes.outside 1632 -> 1769, read_bytes.store 25421 -> 32210, read_calls.ipc 495 -> 366, read_calls.outside 31 -> 33, read_calls.store 217 -> 195, spawn:git 59 -> 41, spawns 112 -> 94, syncs.outside 48 -> 61, unlinks 44 -> 40, write_bytes.ipc 4616 -> 4715, write_bytes.jobs 8406 -> 8394, write_bytes.outside 747 -> 904, write_bytes.store 60230 -> 57342, write_calls.outside 12 -> 13, write_calls.store 39 -> 37
- claude suspend: cli_maxrss_kib 11648 -> 11760, cmd:git bundle 3 -> 2, frees 13078 -> 12968, malloc_bytes 3595940 -> 3594469, mallocs 16400 -> 16258, read_bytes.ipc 5293 -> 5167, read_bytes.store 9818 -> 10581, read_calls.ipc 185 -> 178, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 35 -> 34, write_bytes.jobs 4761 -> 4769, write_bytes.store 54979 -> 63195, write_calls.store 33 -> 37
- claude move: cli_maxrss_kib 12272 -> 12464, frees 6818 -> 6839, malloc_bytes 2014057 -> 2015614, mallocs 8284 -> 8306, read_bytes.jobs 4078 -> 4066, read_bytes.store 1257 -> 1251
- claude resume: cli_maxrss_kib 15328 -> 15456, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 2 -> 0, frees 9459 -> 7181, fsync 55 -> 65, malloc_bytes 4410132 -> 3690919, mallocs 11453 -> 8698, opens 251 -> 270, read_bytes.ipc 4417 -> 1890, read_bytes.jobs 8582 -> 8581, read_bytes.outside 3026 -> 3163, read_bytes.state 270 -> 122, read_bytes.store 13271 -> 18543, read_calls.ipc 180 -> 84, read_calls.outside 16 -> 18, read_calls.state 8 -> 4, read_calls.store 125 -> 111, spawn:git 25 -> 10, spawns 35 -> 20, syncs.outside 43 -> 53, unlinks 25 -> 22, write_bytes.ipc 1779 -> 1878, write_bytes.jobs 4704 -> 4695, write_bytes.outside 483 -> 640, write_bytes.store 4530 -> 2480, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- claude pull: cli_maxrss_kib 15456 -> 15568, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 4 -> 2, frees 27347 -> 24601, fsync 63 -> 76, malloc_bytes 9151504 -> 8163588, mallocs 33850 -> 30508, opens 461 -> 486, read_bytes.ipc 10656 -> 7546, read_bytes.jobs 15701 -> 15726, read_bytes.outside 5735 -> 5872, read_bytes.store 26450 -> 32867, read_calls.ipc 506 -> 388, read_calls.outside 33 -> 35, read_calls.store 217 -> 195, spawn:git 59 -> 41, spawns 115 -> 97, syncs.outside 48 -> 61, unlinks 48 -> 44, write_bytes.ipc 4123 -> 4222, write_bytes.jobs 10856 -> 10891, write_bytes.outside 617 -> 774, write_bytes.store 70231 -> 90448, write_calls.outside 12 -> 13, write_calls.store 45 -> 53
- codex suspend: cli_maxrss_kib 11664 -> 11712, cmd:git bundle 3 -> 2, frees 10295 -> 10186, malloc_bytes 3681142 -> 3667605, mallocs 12619 -> 12470, opens 154 -> 152, read_bytes.ipc 5307 -> 5181, read_bytes.state 235 -> 87, read_bytes.store 10174 -> 10984, read_calls.ipc 190 -> 182, read_calls.state 6 -> 2, read_calls.store 67 -> 68, spawn:git 30 -> 29, spawns 37 -> 36, write_bytes.jobs 5639 -> 5644, write_bytes.store 88315 -> 80079, write_calls.store 49 -> 45
- codex move: cli_maxrss_kib 12272 -> 12432, frees 6827 -> 6818, malloc_bytes 2016314 -> 2012865, mallocs 8281 -> 8272, read_bytes.jobs 4102 -> 4086, read_bytes.store 1289 -> 1292
- codex resume: cli_maxrss_kib 15280 -> 15424, cmd:git bundle 10 -> 2, cmd:git config 4 -> 0, cmd:git rev-parse 1 -> 0, cmd:git symbolic-ref 2 -> 0, frees 9616 -> 7324, fsync 59 -> 69, malloc_bytes 4515713 -> 3796515, mallocs 11586 -> 8819, opens 295 -> 314, read_bytes.ipc 4567 -> 2040, read_bytes.jobs 12193 -> 12194, read_bytes.outside 9297 -> 9434, read_bytes.state 270 -> 122, read_bytes.store 13782 -> 18985, read_calls.ipc 181 -> 86, read_calls.outside 15 -> 17, read_calls.state 8 -> 4, read_calls.store 127 -> 113, spawn:git 25 -> 10, spawns 36 -> 21, syncs.outside 43 -> 53, unlinks 28 -> 25, write_bytes.ipc 1713 -> 1812, write_bytes.jobs 5625 -> 5622, write_bytes.outside 483 -> 640, write_bytes.store 4531 -> 2480, write_calls.outside 11 -> 12, write_calls.store 5 -> 4
- codex pull: cli_maxrss_kib 14080 -> 15600, cmd:git bundle 15 -> 4, cmd:git config 5 -> 1, cmd:git rev-parse 4 -> 3, cmd:git symbolic-ref 4 -> 2, frees 24120 -> 21360, fsync 67 -> 80, malloc_bytes 9298607 -> 8314689, mallocs 29379 -> 26020, opens 512 -> 537, read_bytes.ipc 10821 -> 7711, read_bytes.jobs 20110 -> 20114, read_bytes.outside 18389 -> 18526, read_bytes.store 27353 -> 33744, read_calls.ipc 517 -> 392, read_calls.outside 33 -> 35, read_calls.store 219 -> 197, spawn:git 59 -> 41, spawns 118 -> 100, syncs.outside 45 -> 58, unlinks 54 -> 50, write_bytes.ipc 4056 -> 4156, write_bytes.jobs 12682 -> 12676, write_bytes.outside 504 -> 661, write_bytes.store 85471 -> 74356, write_calls.outside 11 -> 12, write_calls.store 51 -> 45
- cold start: cli_maxrss_kib 10848 -> 10816, cli_processes 11 -> 9, frees 9810 -> 8270, malloc_bytes 3706862 -> 3126407, mallocs 12226 -> 10428, write_bytes.ipc 1754 -> 1421, write_calls.ipc 21 -> 17
- restart: cli_maxrss_kib 11264 -> 11296, frees 20536 -> 21193, malloc_bytes 6879880 -> 6966683, mallocs 26009 -> 26736, opens 271 -> 284, read_bytes.store 477956 -> 490316, read_calls.store 118 -> 121, write_bytes.ipc 6776 -> 7574, write_calls.ipc 95 -> 107
