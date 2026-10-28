# Counters: branch base 8d35276 against 6d57d63

Load-insensitive counters only: every timing of this run was taken while
the machine was loaded and is not kept. Medians over the counted runs,
anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | barriers | plain fsyncs | allocations | CLI peak RSS KiB | daemon a RSS KiB | daemon b RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 4 | 0 -> 3 | 0 -> 0 | 38481 -> 38487.5 | 8088 -> 8160 | 13448 -> 13120 | 14760 -> 14168 |
| acp launch | 4 -> 4 | 12 -> 6 | 0 -> 5 | 0 -> 0 | 4897.5 -> 4896 | 7368 -> 7488 | 13008 -> 13456 | 14448 -> 14184 |
| acp approve | 0 -> 0 | 3 -> 1 | 0 -> 1 | 0 -> 0 | 1176.5 -> 1177 | 5048 -> 5192 | 13016 -> 13456 | 14440 -> 14040 |
| claude new | 3 -> 3 | 8 -> 4 | 0 -> 3 | 0 -> 0 | 1654 -> 1651 | 5080 -> 5232 | 12544 -> 13488 | 14392 -> 14000 |
| claude launch | 4 -> 4 | 15 -> 8 | 0 -> 6 | 0 -> 0 | 3595 -> 3592.5 | 7056 -> 7304 | 12528 -> 12696 | 14384 -> 13936 |
| claude approve | 0 -> 0 | 3 -> 1 | 0 -> 1 | 0 -> 0 | 1193.5 -> 1185 | 4984 -> 5144 | 12488 -> 12320 | 13688 -> 13936 |
| codex new | 3 -> 3 | 8 -> 4 | 0 -> 3 | 0 -> 0 | 1655.5 -> 1650 | 5104 -> 5272 | 12128 -> 12224 | 12960 -> 12936 |
| codex launch | 4 -> 4 | 17 -> 9 | 0 -> 7 | 0 -> 0 | 3924 -> 3924.5 | 7168 -> 7216 | 12272 -> 12024 | 12832 -> 13288 |
| codex approve | 0 -> 0 | 3 -> 1 | 0 -> 1 | 0 -> 0 | 1183.5 -> 1175 | 5056 -> 5192 | 11824 -> 12008 | 12880 -> 13080 |
| ledger append | 0 -> 0 | 3 -> 1 | 0 -> 1 | 0 -> 0 | 1176 -> 1180.5 | 5080 -> 5152 | 11744 -> 12008 | 12880 -> 13080 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 189 -> 189 | 3200 -> 3192 | 11744 -> 12008 | 12856 -> 13080 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1474.5 -> 1473 | 5224 -> 5320 | 11744 -> 12056 | 12840 -> 13056 |
| acp suspend | 36 -> 36 | 39 -> 21 | 0 -> 10 | 9 -> 13 | 12813 -> 12825.5 | 5200 -> 5456 | 14280 -> 14592 | 13712 -> 13960 |
| acp move | 42 -> 42 | 15 -> 7 | 0 -> 4 | 0 -> 0 | 8338 -> 8345.5 | 5600 -> 5752 | 14744 -> 14992 | 14128 -> 14440 |
| acp resume | 31 -> 31 | 24 -> 14 | 0 -> 9 | 51 -> 51 | 11009 -> 11008 | 7400 -> 7048 | 14752 -> 14320 | 14728 -> 15024 |
| acp pull | 110 -> 110 | 81 -> 43 | 0 -> 24 | 59 -> 63 | 28596 -> 28389 | 7112 -> 7104 | 14944 -> 15000 | 14936 -> 14960 |
| claude suspend | 35 -> 35 | 41 -> 21 | 0 -> 10 | 8 -> 14 | 15724.5 -> 15495 | 5200 -> 5296 | 14944 -> 14992 | 14080 -> 14256 |
| claude move | 42 -> 42 | 15 -> 7 | 0 -> 4 | 0 -> 0 | 8199 -> 8192.5 | 5680 -> 5728 | 15056 -> 15168 | 14424 -> 14552 |
| claude resume | 33 -> 33 | 24 -> 14 | 0 -> 9 | 55 -> 55 | 10847 -> 10843.5 | 6440 -> 7104 | 13312 -> 13312 | 14608 -> 15008 |
| claude pull | 113 -> 113 | 83 -> 43 | 0 -> 24 | 63 -> 69 | 32404.5 -> 32387 | 6968 -> 7792 | 14448 -> 14752 | 14240 -> 14952 |
| codex suspend | 37 -> 37 | 40 -> 19 | 0 -> 9 | 8 -> 16 | 12750.5 -> 12509.5 | 5176 -> 5344 | 14936 -> 15064 | 14112 -> 14448 |
| codex move | 42 -> 42 | 15 -> 7 | 0 -> 4 | 0 -> 0 | 8197 -> 8189 | 5624 -> 5744 | 14888 -> 15128 | 14168 -> 14536 |
| codex resume | 34 -> 34 | 24 -> 14 | 0 -> 9 | 59 -> 59 | 10993 -> 11001.5 | 6488 -> 7072 | 12224 -> 14168 | 14168 -> 14584 |
| codex pull | 116 -> 116 | 82 -> 41 | 0 -> 23 | 67 -> 75 | 28724 -> 29167 | 7136 -> 7120 | 14728 -> 14656 | 14800 -> 14128 |
| cold start | 0 -> 0 | 8 -> 8 | 0 -> 0 | 5 -> 5 | 21267.5 -> 25646.5 | 5032 -> 5176 | 14576 -> 14400 | 14664 -> 13448 |
| restart | 0 -> 0 | 35 -> 6 | 0 -> 14.5 | 0 -> 0 | 21289 -> 21180.5 | 5328 -> 5456 | 11056 -> 11224 | 14848 -> 14368 |

## Counters that differ

Every counter whose median differs, per op, anchor -> candidate. `spawn:<program>`
counts the starts of one program; `<counter>.<class>` splits reads, writes and
flushes (`syncs`) by the file they touch.

- acp new: barrierfsync 0 -> 3, cli_maxrss_kib 8088 -> 8160, daemon_rss_kib.a 13448 -> 13120, daemon_rss_kib.b 14760 -> 14168, frees 27853.5 -> 27764.5, fullfsync 8 -> 4, malloc_bytes 13423586 -> 13424589, mallocs 38481 -> 38487.5, opens 11 -> 10, syncs.jobs 5 -> 4
- acp launch: barrierfsync 0 -> 5, cli_maxrss_kib 7368 -> 7488, daemon_rss_kib.a 13008 -> 13456, daemon_rss_kib.b 14448 -> 14184, frees 3755 -> 3754, fullfsync 12 -> 6, malloc_bytes 869883 -> 869654, mallocs 4897.5 -> 4896, opens 37 -> 36, syncs.jobs 11 -> 10
- acp approve: barrierfsync 0 -> 1, cli_maxrss_kib 5048 -> 5192, daemon_rss_kib.a 13016 -> 13456, daemon_rss_kib.b 14440 -> 14040, frees 1025 -> 1025.5, fullfsync 3 -> 1, malloc_bytes 318342 -> 318377, mallocs 1176.5 -> 1177, opens 3 -> 2, syncs.jobs 2 -> 1
- claude new: barrierfsync 0 -> 3, cli_maxrss_kib 5080 -> 5232, daemon_rss_kib.a 12544 -> 13488, daemon_rss_kib.b 14392 -> 14000, frees 1399.5 -> 1396.5, fullfsync 8 -> 4, malloc_bytes 385563 -> 385188, mallocs 1654 -> 1651, opens 12 -> 10, read_bytes.state 148 -> 0, read_calls.state 4 -> 0, syncs.jobs 5 -> 4
- claude launch: barrierfsync 0 -> 6, cli_maxrss_kib 7056 -> 7304, daemon_rss_kib.a 12528 -> 12696, daemon_rss_kib.b 14384 -> 13936, frees 3198 -> 3196, fullfsync 15 -> 8, malloc_bytes 606285.5 -> 606216, mallocs 3595 -> 3592.5, opens 43 -> 42, read_bytes.jobs 933.5 -> 927.5, syncs.jobs 14 -> 13, write_bytes.jobs 936.5 -> 930.5
- claude approve: barrierfsync 0 -> 1, cli_maxrss_kib 4984 -> 5144, daemon_rss_kib.a 12488 -> 12320, daemon_rss_kib.b 13688 -> 13936, frees 1043.5 -> 1034.5, fullfsync 3 -> 1, malloc_bytes 319102 -> 318791.5, mallocs 1193.5 -> 1185, opens 3 -> 2.5, read_bytes.state 0 -> 61, read_calls.state 0 -> 2, syncs.jobs 2 -> 1
- codex new: barrierfsync 0 -> 3, cli_maxrss_kib 5104 -> 5272, daemon_rss_kib.a 12128 -> 12224, daemon_rss_kib.b 12960 -> 12936, frees 1402 -> 1397, fullfsync 8 -> 4, malloc_bytes 385811 -> 384818, mallocs 1655.5 -> 1650, opens 13 -> 10, read_bytes.state 148 -> 0, read_calls.state 4 -> 0, syncs.jobs 5 -> 4
- codex launch: barrierfsync 0 -> 7, cli_maxrss_kib 7168 -> 7216, daemon_rss_kib.a 12272 -> 12024, daemon_rss_kib.b 12832 -> 13288, frees 3506 -> 3507.5, fullfsync 17 -> 9, malloc_bytes 700065 -> 699982.5, mallocs 3924 -> 3924.5, read_bytes.state 174 -> 248, read_calls.ipc 12 -> 12.5, read_calls.state 4 -> 6, syncs.jobs 16 -> 15
- codex approve: barrierfsync 0 -> 1, cli_maxrss_kib 5056 -> 5192, daemon_rss_kib.a 11824 -> 12008, daemon_rss_kib.b 12880 -> 13080, frees 1033.5 -> 1025, fullfsync 3 -> 1, malloc_bytes 318462 -> 317921, mallocs 1183.5 -> 1175, opens 3 -> 2, syncs.jobs 2 -> 1
- ledger append: barrierfsync 0 -> 1, cli_maxrss_kib 5080 -> 5152, daemon_rss_kib.a 11744 -> 12008, daemon_rss_kib.b 12880 -> 13080, frees 1025 -> 1029, fullfsync 3 -> 1, malloc_bytes 318174 -> 318689, mallocs 1176 -> 1180.5, opens 3 -> 2, syncs.jobs 2 -> 1
- cli version: cli_maxrss_kib 3200 -> 3192, daemon_rss_kib.a 11744 -> 12008, daemon_rss_kib.b 12856 -> 13080, malloc_bytes 87612 -> 87615
- cli ls: cli_maxrss_kib 5224 -> 5320, daemon_rss_kib.a 11744 -> 12056, daemon_rss_kib.b 12840 -> 13056, frees 1289 -> 1288, malloc_bytes 359221 -> 358904, mallocs 1474.5 -> 1473
- acp suspend: barrierfsync 0 -> 10, cli_maxrss_kib 5200 -> 5456, daemon_rss_kib.a 14280 -> 14592, daemon_rss_kib.b 13712 -> 13960, frees 9724 -> 9728.5, fsync 9 -> 13, fullfsync 39 -> 21, malloc_bytes 3513501 -> 3514728.5, mallocs 12813 -> 12825.5, opens 142 -> 139, read_bytes.ipc 5325 -> 5324.5, read_bytes.state 87 -> 161, read_bytes.store 29796 -> 29771, read_calls.ipc 186 -> 189, read_calls.state 2 -> 4, syncs.jobs 22 -> 18, write_bytes.jobs 3578 -> 3596, write_bytes.store 87708 -> 87705
- acp move: barrierfsync 0 -> 4, cli_maxrss_kib 5600 -> 5752, daemon_rss_kib.a 14744 -> 14992, daemon_rss_kib.b 14128 -> 14440, frees 6848.5 -> 6853.5, fullfsync 15 -> 7, malloc_bytes 1995586.5 -> 1995906, mallocs 8338 -> 8345.5, opens 73 -> 69, read_bytes.jobs 4089 -> 4127, read_bytes.store 1178 -> 1175.5, syncs.jobs 9 -> 5
- acp resume: barrierfsync 0 -> 9, cli_maxrss_kib 7400 -> 7048, daemon_rss_kib.a 14752 -> 14320, daemon_rss_kib.b 14728 -> 15024, frees 8878.5 -> 8877.5, fullfsync 24 -> 14, malloc_bytes 4134700 -> 4134654.5, mallocs 11009 -> 11008, opens 209 -> 208, read_bytes.ipc 4469 -> 4473, read_bytes.jobs 3555 -> 3559, read_bytes.store 10778.5 -> 10757.5, read_calls.ipc 151 -> 156, syncs.jobs 31 -> 30, write_bytes.jobs 3542 -> 3553.5, write_bytes.store 3723.5 -> 3721
- acp pull: barrierfsync 0 -> 24, cli_maxrss_kib 7112 -> 7104, daemon_rss_kib.a 14944 -> 15000, daemon_rss_kib.b 14936 -> 14960, frees 22788 -> 22774.5, fsync 59 -> 63, fullfsync 81 -> 43, malloc_bytes 8667487.5 -> 8614597.5, mallocs 28596 -> 28389, opens 413 -> 403, read_bytes.ipc 10692.5 -> 10694, read_bytes.jobs 8540.5 -> 8530, read_bytes.store 23430 -> 23341.5, read_calls.ipc 483.5 -> 475.5, syncs.jobs 64 -> 54, write_bytes.jobs 8398.5 -> 8381.5, write_bytes.store 55310 -> 55282.5
- claude suspend: barrierfsync 0 -> 10, cli_maxrss_kib 5200 -> 5296, daemon_rss_kib.a 14944 -> 14992, daemon_rss_kib.b 14080 -> 14256, frees 12406 -> 12384, fsync 8 -> 14, fullfsync 41 -> 21, malloc_bytes 3595429.5 -> 3540064.5, mallocs 15724.5 -> 15495, opens 145 -> 141, read_bytes.store 9764 -> 9800.5, read_calls.ipc 182.5 -> 181.5, syncs.jobs 24 -> 20, write_bytes.jobs 4473.5 -> 4477, write_bytes.store 50832 -> 50842
- claude move: barrierfsync 0 -> 4, cli_maxrss_kib 5680 -> 5728, daemon_rss_kib.a 15056 -> 15168, daemon_rss_kib.b 14424 -> 14552, frees 6746.5 -> 6735, fullfsync 15 -> 7, malloc_bytes 1982713.5 -> 1981994, mallocs 8199 -> 8192.5, opens 73 -> 69, read_bytes.jobs 4066 -> 4070, read_bytes.store 1251 -> 1253.5, syncs.jobs 9 -> 5
- claude resume: barrierfsync 0 -> 9, cli_maxrss_kib 6440 -> 7104, daemon_rss_kib.b 14608 -> 15008, frees 8975 -> 8973.5, fullfsync 24 -> 14, malloc_bytes 4138620.5 -> 4138018, mallocs 10847 -> 10843.5, opens 246 -> 244, read_bytes.ipc 3947 -> 3951, read_bytes.jobs 7466.5 -> 7466, read_bytes.state 270 -> 196, read_bytes.store 11193 -> 11207.5, read_calls.ipc 156 -> 161.5, read_calls.state 8 -> 6, syncs.jobs 35 -> 34, write_bytes.jobs 4429.5 -> 4422, write_bytes.store 3722 -> 3720
- claude pull: barrierfsync 0 -> 24, cli_maxrss_kib 6968 -> 7792, daemon_rss_kib.a 14448 -> 14752, daemon_rss_kib.b 14240 -> 14952, frees 26218 -> 26213, fsync 63 -> 69, fullfsync 83 -> 43, malloc_bytes 8837831 -> 8837138.5, mallocs 32404.5 -> 32387, opens 456 -> 446, read_bytes.ipc 10193.5 -> 10194, read_bytes.jobs 14321 -> 14335, read_bytes.store 24298.5 -> 24357.5, read_calls.ipc 488 -> 490, syncs.jobs 70 -> 60, write_bytes.jobs 10316.5 -> 10335.5, write_bytes.store 55422 -> 80156.5, write_calls.store 36 -> 48
- codex suspend: barrierfsync 0 -> 9, cli_maxrss_kib 5176 -> 5344, daemon_rss_kib.a 14936 -> 15064, daemon_rss_kib.b 14112 -> 14448, frees 10233.5 -> 10217.5, fsync 8 -> 16, fullfsync 40 -> 19, malloc_bytes 3676616 -> 3614218, mallocs 12750.5 -> 12509.5, opens 154 -> 150, read_bytes.ipc 5306.5 -> 5307, read_bytes.store 10104 -> 10145.5, read_calls.ipc 193 -> 190, syncs.jobs 26 -> 22, write_bytes.jobs 5253 -> 5241.5, write_bytes.store 67690 -> 63585, write_calls.store 39 -> 37
- codex move: barrierfsync 0 -> 4, cli_maxrss_kib 5624 -> 5744, daemon_rss_kib.a 14888 -> 15128, daemon_rss_kib.b 14168 -> 14536, frees 6750.5 -> 6745, fullfsync 15 -> 7, malloc_bytes 1981794.5 -> 1981513, mallocs 8197 -> 8189, opens 73 -> 69, read_bytes.jobs 4092 -> 4100, read_bytes.store 1279.5 -> 1283.5, syncs.jobs 9 -> 5
- codex resume: barrierfsync 0 -> 9, cli_maxrss_kib 6488 -> 7072, daemon_rss_kib.a 12224 -> 14168, daemon_rss_kib.b 14168 -> 14584, frees 9138.5 -> 9141, fullfsync 24 -> 14, malloc_bytes 4245127.5 -> 4245785.5, mallocs 10993 -> 11001.5, read_bytes.ipc 4097.5 -> 4101, read_bytes.jobs 10612 -> 10618.5, read_bytes.state 196 -> 270, read_bytes.store 11663 -> 11685, read_calls.ipc 154 -> 156, read_calls.state 6 -> 8, syncs.jobs 39 -> 38, write_bytes.ipc 1712 -> 1712.5, write_bytes.jobs 5230.5 -> 5248, write_bytes.store 3723.5 -> 3725
- codex pull: barrierfsync 0 -> 23, cli_maxrss_kib 7136 -> 7120, daemon_rss_kib.a 14728 -> 14656, daemon_rss_kib.b 14800 -> 14128, frees 23576 -> 23600.5, fsync 67 -> 75, fullfsync 82 -> 41, malloc_bytes 8980780 -> 9086617, mallocs 28724 -> 29167, opens 506 -> 498, read_bytes.ipc 10356.5 -> 10357.5, read_bytes.jobs 18117.5 -> 18113.5, read_bytes.state 248 -> 396, read_bytes.store 25153 -> 25228, read_calls.ipc 497.5 -> 494.5, read_calls.state 6 -> 10, syncs.jobs 76 -> 66, write_bytes.jobs 11863 -> 11864.5, write_bytes.store 84637.5 -> 59939.5, write_calls.store 50 -> 38
- cold start: cli_maxrss_kib 5032 -> 5176, cli_processes 22 -> 27, daemon_rss_kib.a 14576 -> 14400, daemon_rss_kib.b 14664 -> 13448, frees 17550.5 -> 21303.5, malloc_bytes 6748050.5 -> 8171116.5, mallocs 21267.5 -> 25646.5, write_bytes.ipc 3774 -> 4650, write_calls.ipc 43 -> 53
- restart: barrierfsync 0 -> 14.5, cli_maxrss_kib 5328 -> 5456, daemon_rss_kib.a 11056 -> 11224, daemon_rss_kib.b 14848 -> 14368, frees 16556.5 -> 16471, fullfsync 35 -> 6, malloc_bytes 5636369.5 -> 5615616, mallocs 21289 -> 21180.5, opens 239.5 -> 221, read_bytes.state 766.5 -> 736, read_bytes.store 368776 -> 364656, read_calls.state 19 -> 18, read_calls.store 91.5 -> 90.5, syncs.jobs 29 -> 14.5, write_bytes.ipc 5403.5 -> 5202.5, write_calls.ipc 76 -> 73
