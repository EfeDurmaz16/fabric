### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 20 | 444.4 | 419.9 | 0.857 | 0.707 to 1.191 | not shown |
| acp launch | 20 | 170.1 | 136.1 | 0.762 | 0.729 to 0.822 | faster |
| acp approve | 20 | 38.4 | 32.6 | 0.951 | 0.821 to 0.987 | faster |
| claude new | 20 | 411.4 | 350.2 | 1.047 | 0.758 to 1.148 | not shown, median above 1 |
| claude launch | 20 | 167.4 | 202.4 | 1.118 | 1.048 to 1.251 | slower |
| claude approve | 20 | 38.9 | 39.7 | 1.072 | 0.839 to 1.384 | not shown, median above 1 |
| codex new | 20 | 536.8 | 385.9 | 1.031 | 0.781 to 1.229 | not shown, median above 1 |
| codex launch | 20 | 218.7 | 242.3 | 1.067 | 0.926 to 1.235 | not shown, median above 1 |
| codex approve | 20 | 39.5 | 37.1 | 0.893 | 0.833 to 1.022 | not shown |
| ledger append | 20 | 40.7 | 40.0 | 1.070 | 0.931 to 1.284 | not shown, median above 1 |
| cli version | 20 | 4.7 | 4.9 | 0.969 | 0.881 to 0.994 | faster |
| cli ls | 20 | 8.0 | 8.0 | 0.971 | 0.916 to 1.094 | not shown |
| acp suspend | 20 | 1041.7 | 1005.9 | 0.982 | 0.909 to 1.102 | not shown |
| acp move | 20 | 325.6 | 306.4 | 1.022 | 0.897 to 1.117 | not shown, median above 1 |
| acp resume | 20 | 1841.0 | 1751.7 | 1.017 | 0.901 to 1.171 | not shown, median above 1 |
| acp pull | 20 | 3105.4 | 3288.5 | 1.092 | 0.995 to 1.192 | not shown, median above 1 |
| claude suspend | 20 | 1040.9 | 1027.9 | 1.012 | 0.915 to 1.149 | not shown, median above 1 |
| claude move | 20 | 285.9 | 314.7 | 1.125 | 1.063 to 1.194 | slower |
| claude resume | 20 | 1934.8 | 2045.1 | 1.076 | 0.966 to 1.160 | not shown, median above 1 |
| claude pull | 20 | 3000.8 | 3356.3 | 1.132 | 1.093 to 1.201 | slower |
| codex suspend | 20 | 3531.0 | 3065.8 | 0.912 | 0.821 to 1.042 | not shown |
| codex move | 20 | 286.5 | 268.9 | 0.956 | 0.805 to 1.133 | not shown |
| codex resume | 20 | 3671.3 | 3774.7 | 1.002 | 0.891 to 1.073 | not shown, median above 1 |
| codex pull | 20 | 6820.3 | 7163.3 | 1.032 | 0.942 to 1.097 | not shown, median above 1 |
| cold start | 20 | 205.7 | 241.6 | 1.271 | 1.098 to 1.637 | slower |
| restart | 20 | 1060.2 | 1145.9 | 1.096 | 1.033 to 1.160 | slower |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 38956 -> 38523 | 14656 -> 14928 | 24.6 -> 21.2 |
| acp launch | 4 -> 4 | 15 -> 12 | 0 -> 0 | 135 -> 135 | 1184 -> 683 | 4130 -> 4529 | 14480 -> 15504 | 15.1 -> 16.1 |
| acp approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1198 -> 1210 | 10528 -> 10816 | 9.8 -> 10.2 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1680 -> 1687 | 10784 -> 11008 | 10.2 -> 11.0 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1101 -> 1102 | 3532 -> 3624 | 14224 -> 14768 | 12.9 -> 12.5 |
| claude approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1196 -> 1209 | 10544 -> 10848 | 9.9 -> 9.1 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1671 -> 1686 | 10736 -> 10944 | 12.7 -> 9.8 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3846 -> 3960 | 14128 -> 14784 | 14.9 -> 16.6 |
| codex approve | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1192 -> 1208 | 10528 -> 10864 | 10.3 -> 9.6 |
| ledger append | 0 -> 0 | 3 -> 3 | 0 -> 0 | 98 -> 98 | 0 -> 0 | 1198 -> 1222 | 10544 -> 10848 | 10.1 -> 10.2 |
| cli version | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 201 -> 201 | 4928 -> 4848 | 5.6 -> 5.5 |
| cli ls | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 0 -> 0 | 1539 -> 1553 | 10800 -> 11024 | 10.2 -> 10.2 |
| acp suspend | 36 -> 36 | 39 -> 39 | 9 -> 9 | 522 -> 522 | 40064 -> 40371 | 15015 -> 12858 | 11232 -> 11664 | 11.0 -> 9.7 |
| acp move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5216 -> 5283 | 8350 -> 8418 | 11856 -> 12304 | 10.2 -> 11.1 |
| acp resume | 32 -> 33 | 24 -> 24 | 51 -> 51 | 167 -> 167 | 16038 -> 16550 | 11168 -> 11550 | 14576 -> 15664 | 14.9 -> 14.5 |
| acp pull | 111 -> 112 | 81 -> 81 | 59 -> 59 | 1376 -> 1376 | 33305 -> 34156 | 30388 -> 28712 | 13424 -> 14512 | 18.0 -> 16.6 |
| claude suspend | 35 -> 35 | 41 -> 41 | 8 -> 8 | 522 -> 522 | 13308 -> 13535 | 19229 -> 16401 | 11280 -> 11600 | 11.2 -> 11.7 |
| claude move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5276 -> 5314 | 8235 -> 8275 | 11808 -> 12256 | 10.4 -> 10.4 |
| claude resume | 34 -> 35 | 24 -> 24 | 55 -> 55 | 148 -> 148 | 21706 -> 22086 | 11101 -> 11451 | 14768 -> 15392 | 18.2 -> 16.0 |
| claude pull | 114 -> 115 | 83 -> 83 | 63 -> 63 | 1338 -> 1338 | 41807 -> 42355 | 36348 -> 33871 | 13616 -> 14128 | 16.9 -> 18.3 |
| codex suspend | 37 -> 37 | 40 -> 40 | 8 -> 8 | 522 -> 522 | 14444 -> 14915 | 16221 -> 12597 | 11232 -> 11616 | 12.0 -> 11.3 |
| codex move | 42 -> 42 | 15 -> 15 | 0 -> 0 | 520 -> 520 | 5338 -> 5388 | 8271 -> 8268 | 11888 -> 12288 | 12.1 -> 12.5 |
| codex resume | 35 -> 36 | 24 -> 24 | 59 -> 59 | 148 -> 148 | 25935 -> 26254 | 11242 -> 11585 | 14736 -> 14352 | 18.7 -> 17.9 |
| codex pull | 117 -> 118 | 82 -> 82 | 67 -> 67 | 1338 -> 1338 | 47149 -> 47814 | 32678 -> 29386 | 13664 -> 14272 | 19.5 -> 25.3 |
| cold start | 0 -> 0 | 8 -> 8 | 5 -> 5 | 0 -> 0 | 45283 -> 45222 | 21196 -> 19357 | 10496 -> 10816 | 191.8 -> 205.1 |
| restart | 0 -> 0 | 38 -> 38 | 0 -> 0 | 463 -> 463 | 493385 -> 485145 | 26670 -> 22735 | 11008 -> 11168 | 212.4 -> 185.1 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 24596 -> 21207, cli_maxrss_kib 14656 -> 14928, frees 28129 -> 27791, malloc_bytes 13540408 -> 13432491, mallocs 38956 -> 38523
- acp launch: cli_cpu_us 15055 -> 16077, cli_maxrss_kib 14480 -> 15504, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, frees 3362 -> 3743, fullfsync 15 -> 12, malloc_bytes 718541 -> 775174, mallocs 4130 -> 4529, opens 40 -> 37, read_bytes.jobs 1010 -> 509, read_calls.jobs 12 -> 10, renames 6 -> 5, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, syncs.jobs 14 -> 11, write_bytes.ipc 969 -> 1026, write_bytes.jobs 1032 -> 577, write_calls.ipc 11 -> 15, write_calls.jobs 6 -> 5
- acp approve: cli_cpu_us 9843 -> 10163, cli_maxrss_kib 10528 -> 10816, frees 1039 -> 1051, malloc_bytes 324023 -> 325958, mallocs 1198 -> 1210
- claude new: cli_cpu_us 10167 -> 11027, cli_maxrss_kib 10784 -> 11008, frees 1414 -> 1423, malloc_bytes 392201 -> 392748, mallocs 1680 -> 1687
- claude launch: cli_cpu_us 12885 -> 12500, cli_maxrss_kib 14224 -> 14768, cmd:claude 1 -> 0, cmd:dash holding claude 0 -> 1, frees 3128 -> 3221, malloc_bytes 604478 -> 614447, mallocs 3532 -> 3624, opens 43 -> 42, read_bytes.jobs 927 -> 928, spawn:claude 1 -> 0, spawn:dash 0 -> 1, write_bytes.ipc 692 -> 693, write_bytes.jobs 930 -> 931, write_calls.ipc 11 -> 12
- claude approve: cli_cpu_us 9890 -> 9142, cli_maxrss_kib 10544 -> 10848, frees 1038 -> 1051, malloc_bytes 323895 -> 325494, mallocs 1196 -> 1209
- codex new: cli_cpu_us 12736 -> 9764, cli_maxrss_kib 10736 -> 10944, frees 1407 -> 1423, malloc_bytes 391217 -> 393168, mallocs 1671 -> 1686
- codex launch: cli_cpu_us 14859 -> 16567, cli_maxrss_kib 14128 -> 14784, cmd:codex 1 -> 0, cmd:dash holding codex 0 -> 1, frees 3422 -> 3533, malloc_bytes 696477 -> 708180, mallocs 3846 -> 3960, opens 46 -> 45, spawn:codex 1 -> 0, spawn:dash 0 -> 1, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12
- codex approve: cli_cpu_us 10251 -> 9576, cli_maxrss_kib 10528 -> 10864, frees 1033 -> 1050, malloc_bytes 323895 -> 325430, mallocs 1192 -> 1208
- ledger append: cli_cpu_us 10061 -> 10195, cli_maxrss_kib 10544 -> 10848, frees 1037 -> 1060, malloc_bytes 325879 -> 327222, mallocs 1198 -> 1222
- cli version: cli_cpu_us 5608 -> 5485, cli_maxrss_kib 4928 -> 4848, malloc_bytes 88361 -> 88364
- cli ls: cli_cpu_us 10232 -> 10231, cli_maxrss_kib 10800 -> 11024, frees 1341 -> 1355, malloc_bytes 369098 -> 370953, mallocs 1539 -> 1553
- acp suspend: cli_cpu_us 11044 -> 9703, cli_maxrss_kib 11232 -> 11664, frees 10361 -> 9768, malloc_bytes 3568141 -> 3514943, mallocs 15015 -> 12858, read_bytes.ipc 5269 -> 5325, read_bytes.store 37675 -> 37982, read_calls.ipc 187 -> 186, write_bytes.ipc 1111 -> 1028, write_bytes.jobs 3583 -> 3592, write_bytes.store 95698 -> 95943
- acp move: cli_cpu_us 10219 -> 11118, cli_maxrss_kib 11856 -> 12304, frees 6848 -> 6919, malloc_bytes 2018006 -> 2024685, mallocs 8350 -> 8418, read_bytes.jobs 4099 -> 4107, read_bytes.store 1117 -> 1176, write_bytes.ipc 2698 -> 2510
- acp resume: cli_cpu_us 14894 -> 14458, cli_maxrss_kib 14576 -> 15664, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 1 -> 0, cmd:git symbolic-ref 1 -> 2, frees 8965 -> 9312, malloc_bytes 4354297 -> 4399205, mallocs 11168 -> 11550, opens 212 -> 215, read_bytes.ipc 4937 -> 4938, read_bytes.jobs 3558 -> 3559, read_bytes.state 122 -> 270, read_bytes.store 12358 -> 12721, read_calls.ipc 172 -> 173, read_calls.state 4 -> 8, spawn:dash 0 -> 1, spawn:fabric-fake-agent 1 -> 0, spawn:git 24 -> 25, spawns 32 -> 33, write_bytes.ipc 2046 -> 2077, write_bytes.jobs 3556 -> 3549, write_bytes.store 4530 -> 4531, write_calls.ipc 28 -> 30
- acp pull: cli_cpu_us 17993 -> 16577, cli_maxrss_kib 13424 -> 14512, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:fabric-fake-agent 2 -> 1, cmd:git symbolic-ref 3 -> 4, frees 23387 -> 23234, malloc_bytes 8814875 -> 8831448, mallocs 30388 -> 28712, opens 415 -> 418, read_bytes.ipc 11102 -> 11156, read_bytes.jobs 8534 -> 8536, read_bytes.store 24449 -> 25298, read_calls.ipc 493 -> 491, spawn:dash 0 -> 1, spawn:fabric-fake-agent 2 -> 1, spawn:git 58 -> 59, spawns 111 -> 112, write_bytes.ipc 4625 -> 4617, write_bytes.jobs 8388 -> 8389, write_bytes.store 59958 -> 60212, write_calls.ipc 84 -> 86
- claude suspend: cli_cpu_us 11195 -> 11717, cli_maxrss_kib 11280 -> 11600, frees 13996 -> 13075, malloc_bytes 3679295 -> 3597017, mallocs 19229 -> 16401, read_bytes.ipc 5237 -> 5293, read_bytes.store 9545 -> 9772, write_bytes.ipc 1131 -> 1048, write_bytes.jobs 4753 -> 4763, write_bytes.store 54762 -> 54967
- claude move: cli_cpu_us 10351 -> 10361, cli_maxrss_kib 11808 -> 12256, frees 6776 -> 6811, malloc_bytes 2007809 -> 2013009, mallocs 8235 -> 8275, read_bytes.jobs 4066 -> 4062, read_bytes.store 1210 -> 1252
- claude resume: cli_cpu_us 18194 -> 16012, cli_maxrss_kib 14768 -> 15392, cmd:claude 3 -> 2, cmd:dash holding claude 0 -> 1, cmd:git symbolic-ref 1 -> 2, frees 9139 -> 9459, malloc_bytes 4372546 -> 4410398, mallocs 11101 -> 11451, read_bytes.jobs 8588 -> 8582, read_bytes.state 122 -> 270, read_bytes.store 12996 -> 13234, read_calls.ipc 178 -> 180, read_calls.state 4 -> 8, spawn:claude 3 -> 2, spawn:dash 0 -> 1, spawn:git 24 -> 25, spawns 34 -> 35, write_bytes.ipc 1852 -> 1780, write_bytes.jobs 4695 -> 4693, write_bytes.store 4531 -> 4528, write_calls.ipc 28 -> 29
- claude pull: cli_cpu_us 16886 -> 18254, cli_maxrss_kib 13616 -> 14128, cmd:claude 8 -> 7, cmd:dash holding claude 0 -> 1, cmd:git symbolic-ref 3 -> 4, frees 27953 -> 27369, malloc_bytes 9185966 -> 9152123, mallocs 36348 -> 33871, read_bytes.ipc 10602 -> 10658, read_bytes.jobs 15713 -> 15707, read_bytes.store 25772 -> 26326, read_calls.ipc 503 -> 492, spawn:claude 8 -> 7, spawn:dash 0 -> 1, spawn:git 58 -> 59, spawns 114 -> 115, write_bytes.ipc 4279 -> 4124, write_bytes.jobs 10879 -> 10867, write_bytes.store 78251 -> 70215, write_calls.ipc 80 -> 81, write_calls.store 49 -> 45
- codex suspend: cli_cpu_us 12022 -> 11252, cli_maxrss_kib 11232 -> 11616, frees 11708 -> 10293, malloc_bytes 3735793 -> 3679573, mallocs 16221 -> 12597, opens 152 -> 154, read_bytes.ipc 5250 -> 5307, read_bytes.state 87 -> 235, read_bytes.store 9863 -> 10186, read_calls.state 2 -> 6, write_bytes.ipc 1125 -> 1042, write_bytes.jobs 5642 -> 5638, write_bytes.store 88082 -> 63609, write_calls.store 49 -> 37
- codex move: cli_cpu_us 12086 -> 12483, cli_maxrss_kib 11888 -> 12288, frees 6814 -> 6809, malloc_bytes 2011949 -> 2013266, mallocs 8271 -> 8268, read_bytes.store 1244 -> 1294
- codex resume: cli_cpu_us 18709 -> 17870, cli_maxrss_kib 14736 -> 14352, cmd:codex 3 -> 2, cmd:dash holding codex 0 -> 1, cmd:git symbolic-ref 1 -> 2, frees 9301 -> 9615, malloc_bytes 4478728 -> 4515774, mallocs 11242 -> 11585, read_bytes.jobs 12200 -> 12196, read_bytes.store 13465 -> 13788, read_calls.ipc 177 -> 178, spawn:codex 3 -> 2, spawn:dash 0 -> 1, spawn:git 24 -> 25, spawns 35 -> 36, write_bytes.ipc 1786 -> 1714, write_bytes.jobs 5629 -> 5636, write_bytes.store 4531 -> 4529, write_calls.ipc 28 -> 29
- codex pull: cli_cpu_us 19534 -> 25334, cli_maxrss_kib 13664 -> 14272, cmd:codex 9 -> 8, cmd:dash holding codex 0 -> 1, cmd:git symbolic-ref 3 -> 4, frees 25224 -> 24126, malloc_bytes 9335617 -> 9299559, mallocs 32678 -> 29386, read_bytes.ipc 10766 -> 10821, read_bytes.jobs 20129 -> 20105, read_bytes.store 26698 -> 27387, read_calls.ipc 504 -> 506, spawn:codex 9 -> 8, spawn:dash 0 -> 1, spawn:git 58 -> 59, spawns 117 -> 118, write_bytes.ipc 4213 -> 4058, write_bytes.jobs 12704 -> 12679, write_bytes.store 75368 -> 85469, write_calls.ipc 80 -> 81, write_calls.store 45 -> 51
- cold start: cli_cpu_us 191774 -> 205081, cli_maxrss_kib 10496 -> 10816, cli_processes 21 -> 19, frees 17498 -> 15893, malloc_bytes 6610592 -> 6035769, mallocs 21196 -> 19357, opens 22 -> 21, read_bytes.state 111 -> 50, read_calls.state 4 -> 2, write_bytes.ipc 3564 -> 3231, write_calls.ipc 41 -> 37
- restart: cli_cpu_us 212353 -> 185065, cli_maxrss_kib 11008 -> 11168, cli_processes 19 -> 16, frees 20876 -> 17646, malloc_bytes 7139530 -> 6131312, mallocs 26670 -> 22735, opens 271 -> 245, read_bytes.store 469716 -> 461476, read_calls.store 116 -> 114, write_bytes.ipc 6941 -> 5291, write_calls.ipc 97 -> 74

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 19712 | 20896 | 20648 | 21648 | 1.042 | 1.032 to 1.083 |
| daemon b | 22224 | 24800 | 23240 | 26016 | 1.043 | 1.029 to 1.049 |
