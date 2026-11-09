### Perf gate: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 270.8 | 246.5 | 0.940 | 0.720 to 1.298 | 0.699 to 1.356 | pass |
| acp launch | 15 | 35.8 | 37.8 | 1.006 | 0.832 to 1.109 | 0.796 to 1.131 | pass |
| acp approve | 15 | 7.7 | 7.7 | 0.993 | 0.943 to 1.053 | 0.899 to 1.069 | pass |
| claude new | 15 | 240.7 | 262.5 | 1.055 | 0.996 to 1.218 | 0.855 to 1.223 | pass |
| claude launch | 15 | 191.1 | 199.6 | 1.085 | 0.823 to 1.193 | 0.785 to 1.369 | pass |
| claude approve | 15 | 8.3 | 8.8 | 0.975 | 0.954 to 1.037 | 0.848 to 1.338 | pass |
| codex new | 15 | 251.0 | 239.8 | 1.020 | 0.989 to 1.323 | 0.864 to 1.520 | pass |
| codex launch | 15 | 172.9 | 251.5 | 1.165 | 0.906 to 1.987 | 0.569 to 2.091 | pass |
| codex approve | 15 | 8.0 | 8.1 | 1.044 | 0.963 to 1.136 | 0.833 to 1.143 | pass |
| ledger append | 15 | 8.0 | 8.0 | 0.966 | 0.892 to 1.011 | 0.795 to 1.073 | pass |
| cli version | 15 | 3.6 | 3.6 | 1.084 | 0.999 to 1.129 | 0.869 to 1.134 | pass |
| cli ls | 15 | 4.2 | 4.5 | 1.002 | 0.986 to 1.057 | 0.957 to 1.062 | pass |
| acp suspend | 15 | 517.9 | 524.8 | 1.020 | 0.959 to 1.063 | 0.931 to 1.236 | pass |
| acp move | 15 | 326.7 | 348.9 | 1.119 | 1.014 to 1.122 | 0.972 to 1.251 | pass; its 95% interval alone is above 1 |
| acp resume | 15 | 1014.0 | 1014.3 | 1.012 | 0.911 to 1.114 | 0.904 to 1.129 | pass |
| acp pull | 15 | 1766.2 | 1833.2 | 0.976 | 0.935 to 1.062 | 0.895 to 1.121 | pass |
| claude suspend | 15 | 704.5 | 682.5 | 0.985 | 0.913 to 1.167 | 0.883 to 1.190 | pass |
| claude move | 15 | 324.5 | 320.9 | 1.055 | 0.868 to 1.203 | 0.857 to 1.220 | pass |
| claude resume | 15 | 1419.1 | 1470.5 | 0.993 | 0.975 to 1.079 | 0.951 to 1.191 | pass |
| claude pull | 15 | 2414.0 | 2467.2 | 1.031 | 0.965 to 1.089 | 0.951 to 1.089 | pass |
| codex suspend | 15 | 737.5 | 721.9 | 1.046 | 0.915 to 1.107 | 0.858 to 1.111 | pass |
| codex move | 15 | 319.0 | 312.4 | 0.959 | 0.870 to 1.107 | 0.817 to 1.114 | pass |
| codex resume | 15 | 1501.4 | 1491.5 | 1.022 | 0.901 to 1.044 | 0.861 to 1.075 | pass |
| codex pull | 15 | 2405.4 | 2389.3 | 0.987 | 0.941 to 1.079 | 0.923 to 1.097 | pass |
| cold start | 15 | 67.1 | 69.9 | 1.029 | 0.634 to 1.319 | 0.596 to 1.397 | pass |
| restart | 15 | 194.2 | 202.0 | 1.113 | 0.908 to 1.250 | 0.874 to 1.380 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10112.0 | 10064.0 | 0.997 | 0.994 to 1.002 | 0.990 to 1.002 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15472.0 | 15280.0 | 1.001 | 0.987 to 1.009 | 0.983 to 1.031 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3297.0 | 3216.0 | 0.980 | 0.970 to 1.004 | 0.968 to 1.004 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5905.0 | 5713.0 | 1.003 | 0.965 to 1.026 | 0.956 to 1.078 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5664.0 | 5656.0 | 1.000 | 1.000 to 1.002 | 0.998 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3321.0 | 1.000 | 1.000 to 1.005 | 0.995 to 1.005 | pass |

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
