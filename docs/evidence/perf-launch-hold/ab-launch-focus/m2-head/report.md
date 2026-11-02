### Timed pairs

Medians in ms; ratio is the median of the pairs' candidate/anchor ratios, with its 95% bootstrap interval. Slower or faster only when the interval excludes 1.

| op | pairs | anchor | candidate | ratio | 95% interval | verdict |
|---|---:|---:|---:|---:|---|---|
| acp new | 100 | 298.0 | 286.5 | 0.963 | 0.915 to 1.047 | not shown |
| acp launch | 100 | 130.6 | 135.8 | 0.959 | 0.903 to 1.038 | not shown |
| claude new | 100 | 289.2 | 277.5 | 0.942 | 0.868 to 0.982 | faster |
| claude launch | 100 | 167.5 | 158.5 | 0.968 | 0.919 to 1.009 | not shown |
| codex new | 100 | 296.9 | 269.2 | 0.942 | 0.895 to 1.022 | not shown |
| codex launch | 100 | 221.7 | 226.6 | 1.043 | 0.969 to 1.081 | not shown, median above 1 |

### Counted ops

Medians over the counted runs, anchor -> candidate, of all `fabric` processes of the side while the op ran.

| op | spawns | full flushes | fsyncs | journal+ledger bytes written | state bytes read | allocations | CLI peak RSS KiB | CLI CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| acp new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 143 -> 143 | 143 -> 143 | 20100 -> 20510 | 15248 -> 14512 | 19.1 -> 18.4 |
| acp launch | 4 -> 4 | 12 -> 12 | 0 -> 0 | 135 -> 135 | 683 -> 683 | 4135 -> 4144 | 15360 -> 15536 | 13.3 -> 13.6 |
| claude new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1685 -> 1686 | 11104 -> 10960 | 10.4 -> 10.1 |
| claude launch | 4 -> 4 | 15 -> 15 | 0 -> 0 | 116 -> 116 | 1104 -> 1245 | 3622 -> 3617 | 14752 -> 14832 | 16.0 -> 14.9 |
| codex new | 3 -> 3 | 8 -> 8 | 0 -> 0 | 124 -> 124 | 124 -> 124 | 1685 -> 1683 | 11072 -> 11008 | 8.8 -> 10.3 |
| codex launch | 4 -> 4 | 17 -> 17 | 0 -> 0 | 116 -> 116 | 685 -> 685 | 3956 -> 3945 | 14752 -> 14640 | 13.6 -> 13.1 |

### Counters that differ

Every counter whose median differs between the builds, per op, anchor -> candidate. `spawn:<program>` counts the starts of one program; `<counter>.<class>` splits reads, writes and flushes (`syncs`) by the file they touch.

- acp new: cli_cpu_us 19141 -> 18377, cli_maxrss_kib 15248 -> 14512, frees 14611 -> 14655, malloc_bytes 6912504 -> 7015679, mallocs 20100 -> 20510
- acp launch: cli_cpu_us 13342 -> 13621, cli_maxrss_kib 15360 -> 15536, cmd:dash holding fabric-fake-agent 0 -> 1, cmd:sh holding fabric-fake-agent 1 -> 0, frees 3713 -> 3720, malloc_bytes 674025 -> 674570, mallocs 4135 -> 4144, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 1025 -> 1026, write_calls.ipc 14 -> 15
- claude new: cli_cpu_us 10434 -> 10096, cli_maxrss_kib 11104 -> 10960, frees 1422 -> 1423, malloc_bytes 393085 -> 392744, mallocs 1685 -> 1686
- claude launch: cli_cpu_us 15974 -> 14947, cli_maxrss_kib 14752 -> 14832, cmd:dash holding claude 0 -> 1, cmd:sh holding claude 1 -> 0, malloc_bytes 627709 -> 627817, mallocs 3622 -> 3617, opens 43 -> 44, read_bytes.jobs 930 -> 923, read_bytes.state 174 -> 322, read_calls.state 4 -> 8, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 692 -> 693, write_bytes.jobs 933 -> 926, write_calls.ipc 11 -> 12
- codex new: cli_cpu_us 8836 -> 10257, cli_maxrss_kib 11072 -> 11008, frees 1424 -> 1422, malloc_bytes 392997 -> 392528, mallocs 1685 -> 1683
- codex launch: cli_cpu_us 13630 -> 13085, cli_maxrss_kib 14752 -> 14640, cmd:dash holding codex 0 -> 1, cmd:sh holding codex 1 -> 0, frees 3535 -> 3532, malloc_bytes 708436 -> 707824, mallocs 3956 -> 3945, opens 46 -> 45, spawn:dash 0 -> 1, spawn:sh 1 -> 0, write_bytes.ipc 530 -> 531, write_calls.ipc 11 -> 12

### Daemon resident size

KiB after each timed run: median and largest per build, and the median of the pairs' ratios with its 95% bootstrap interval.

| daemon | anchor median | anchor max | candidate median | candidate max | ratio | 95% interval |
|---|---:|---:|---:|---:|---:|---|
| daemon a | 22592 | 23360 | 22656 | 23216 | 1.003 | 1.002 to 1.004 |
| daemon b | 17056 | 18016 | 17152 | 17616 | 1.007 | 1.006 to 1.008 |
