### Perf gate: fail

5 of 62 checks break the performance rule (CONTRIBUTING.md): acp new; Daemon resident size, daemon a, KiB per counted op; Daemon resident size, daemon b, KiB per counted op; Daemon physical footprint, daemon a, KiB per counted op; Daemon physical footprint, daemon b, KiB per counted op.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 198.3 | 261.6 | 1.291 | 1.005 to 1.584 | 1.001 to 1.708 | **fail** |
| acp launch | 15 | 32.9 | 33.0 | 0.995 | 0.968 to 1.166 | 0.860 to 1.173 | pass |
| acp approve | 15 | 7.8 | 7.3 | 0.994 | 0.930 to 1.058 | 0.919 to 1.294 | pass |
| claude new | 15 | 255.1 | 195.3 | 0.811 | 0.783 to 1.042 | 0.601 to 1.225 | pass |
| claude launch | 15 | 142.9 | 161.3 | 0.995 | 0.846 to 1.331 | 0.621 to 1.913 | pass |
| claude approve | 15 | 9.4 | 8.5 | 1.006 | 0.799 to 1.275 | 0.655 to 1.292 | pass |
| codex new | 15 | 266.7 | 266.7 | 1.068 | 0.664 to 1.106 | 0.661 to 1.652 | pass |
| codex launch | 15 | 195.7 | 207.3 | 1.008 | 0.674 to 1.761 | 0.604 to 1.772 | pass |
| codex approve | 15 | 7.9 | 8.2 | 0.982 | 0.900 to 1.118 | 0.869 to 1.200 | pass |
| ledger append | 15 | 8.6 | 8.9 | 1.031 | 0.963 to 1.051 | 0.929 to 1.071 | pass |
| cli version | 15 | 3.7 | 3.7 | 1.023 | 0.909 to 1.139 | 0.905 to 1.441 | pass |
| cli ls | 15 | 4.4 | 4.7 | 0.983 | 0.932 to 1.027 | 0.919 to 1.125 | pass |
| acp suspend | 15 | 516.4 | 533.0 | 1.058 | 0.983 to 1.098 | 0.966 to 1.131 | pass |
| acp move | 15 | 329.1 | 336.6 | 1.023 | 0.924 to 1.119 | 0.910 to 1.226 | pass |
| acp resume | 15 | 968.5 | 933.4 | 0.979 | 0.842 to 1.070 | 0.825 to 1.114 | pass |
| acp pull | 15 | 1746.6 | 1766.9 | 1.023 | 0.971 to 1.068 | 0.940 to 1.112 | pass |
| claude suspend | 15 | 731.4 | 664.7 | 0.947 | 0.843 to 1.081 | 0.832 to 1.083 | pass |
| claude move | 15 | 326.0 | 335.5 | 1.008 | 0.929 to 1.138 | 0.914 to 1.146 | pass |
| claude resume | 15 | 1425.2 | 1384.3 | 0.953 | 0.838 to 1.091 | 0.819 to 1.109 | pass |
| claude pull | 15 | 2406.4 | 2240.8 | 0.990 | 0.895 to 1.009 | 0.888 to 1.060 | pass |
| codex suspend | 15 | 708.6 | 672.4 | 0.973 | 0.865 to 1.027 | 0.855 to 1.168 | pass |
| codex move | 15 | 325.7 | 323.5 | 1.049 | 0.954 to 1.113 | 0.941 to 1.130 | pass |
| codex resume | 15 | 1415.3 | 1516.9 | 1.051 | 0.951 to 1.163 | 0.941 to 1.165 | pass |
| codex pull | 15 | 2553.7 | 2421.1 | 0.949 | 0.880 to 0.997 | 0.874 to 1.028 | pass |
| cold start | 15 | 105.8 | 94.7 | 0.764 | 0.666 to 1.107 | 0.452 to 1.200 | pass |
| restart | 15 | 159.1 | 151.7 | 1.009 | 0.837 to 1.115 | 0.757 to 1.168 | pass |

#### Memory

The family interval is the 95% interval across all 10 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10112.0 | 10064.0 | 0.998 | 0.995 to 1.002 | 0.992 to 1.002 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15376.0 | 15360.0 | 0.996 | 0.988 to 0.998 | 0.984 to 0.999 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3248.0 | 3200.0 | 0.995 | 0.975 to 1.005 | 0.968 to 1.010 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5792.0 | 5793.0 | 0.992 | 0.974 to 1.000 | 0.966 to 1.000 | pass |
| Daemon resident size, daemon a, KiB per counted op | 130 | 13840.0 | 14040.0 | 1.004 | 1.002 to 1.008 | 1.002 to 1.010 | **fail** |
| Daemon resident size, daemon b, KiB per counted op | 130 | 15280.0 | 15856.0 | 1.027 | 1.026 to 1.029 | 1.026 to 1.030 | **fail** |
| Daemon physical footprint, daemon a, KiB per counted op | 130 | 5249.0 | 5489.0 | 1.019 | 1.011 to 1.024 | 1.010 to 1.029 | **fail** |
| Daemon physical footprint, daemon b, KiB per counted op | 130 | 5825.0 | 6401.0 | 1.075 | 1.071 to 1.076 | 1.070 to 1.077 | **fail** |
| CLI peak resident size, KiB per counted op | 130 | 5680.0 | 5648.0 | 1.000 | 1.000 to 1.002 | 0.998 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3329.0 | 1.000 | 0.995 to 1.005 | 0.995 to 1.005 | pass |

#### Counters

Medians over the counted runs, anchor -> candidate. An op fails when the candidate starts more processes or makes more full or barrier flushes than the anchor.

| op | process starts | full and barrier flushes | verdict |
|---|---:|---:|---|
| acp new | 3 -> 3 | 8 -> 8 | pass |
| acp launch | 4 -> 4 | 15 -> 15 | pass |
| acp approve | 0 -> 0 | 3 -> 3 | pass |
| claude new | 3 -> 3 | 8 -> 8 | pass |
| claude launch | 4 -> 4 | 15 -> 15 | pass |
| claude approve | 0 -> 0 | 3 -> 3 | pass |
| codex new | 3 -> 3 | 8 -> 8 | pass |
| codex launch | 4 -> 4 | 17 -> 17 | pass |
| codex approve | 0 -> 0 | 3 -> 3 | pass |
| ledger append | 0 -> 0 | 3 -> 3 | pass |
| cli version | 0 -> 0 | 0 -> 0 | pass |
| cli ls | 0 -> 0 | 0 -> 0 | pass |
| acp suspend | 35 -> 35 | 39 -> 39 | pass |
| acp move | 42 -> 42 | 15 -> 15 | pass |
| acp resume | 36 -> 36 | 24 -> 24 | pass |
| acp pull | 115 -> 115 | 81 -> 81 | pass |
| claude suspend | 35 -> 35 | 41 -> 41 | pass |
| claude move | 42 -> 42 | 15 -> 15 | pass |
| claude resume | 38 -> 38 | 24 -> 24 | pass |
| claude pull | 118 -> 118 | 83 -> 83 | pass |
| codex suspend | 37 -> 37 | 40 -> 40 | pass |
| codex move | 42 -> 42 | 15 -> 15 | pass |
| codex resume | 39 -> 39 | 24 -> 24 | pass |
| codex pull | 121 -> 121 | 82 -> 82 | pass |
| cold start | 0 -> 0 | 8 -> 8 | pass |
| restart | 0 -> 0 | 44 -> 44 | pass |
