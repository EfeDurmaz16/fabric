### Perf gate: pass

All 62 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 237.5 | 262.7 | 1.073 | 0.868 to 1.508 | 0.814 to 1.662 | pass |
| acp launch | 15 | 32.3 | 33.1 | 1.008 | 0.962 to 1.055 | 0.915 to 1.133 | pass |
| acp approve | 15 | 7.4 | 7.3 | 1.021 | 0.952 to 1.070 | 0.915 to 1.111 | pass |
| claude new | 15 | 242.8 | 257.1 | 1.051 | 0.943 to 1.545 | 0.879 to 1.734 | pass |
| claude launch | 15 | 228.8 | 188.2 | 0.729 | 0.609 to 0.987 | 0.562 to 1.698 | pass |
| claude approve | 15 | 7.8 | 8.2 | 1.013 | 0.936 to 1.214 | 0.911 to 1.631 | pass |
| codex new | 15 | 244.0 | 259.4 | 1.000 | 0.874 to 1.127 | 0.851 to 1.195 | pass |
| codex launch | 15 | 210.6 | 224.9 | 0.956 | 0.717 to 1.458 | 0.713 to 1.500 | pass |
| codex approve | 15 | 7.5 | 7.5 | 1.023 | 0.950 to 1.074 | 0.949 to 1.091 | pass |
| ledger append | 15 | 7.3 | 7.3 | 1.019 | 0.983 to 1.044 | 0.954 to 1.092 | pass |
| cli version | 15 | 3.2 | 3.1 | 1.022 | 0.938 to 1.071 | 0.921 to 1.092 | pass |
| cli ls | 15 | 4.0 | 4.1 | 1.031 | 0.995 to 1.089 | 0.979 to 1.119 | pass |
| acp suspend | 15 | 496.6 | 492.2 | 1.013 | 0.912 to 1.069 | 0.863 to 1.215 | pass |
| acp move | 15 | 313.8 | 316.9 | 1.016 | 0.902 to 1.061 | 0.869 to 1.114 | pass |
| acp resume | 15 | 888.7 | 933.2 | 1.015 | 0.948 to 1.062 | 0.940 to 1.104 | pass |
| acp pull | 15 | 1706.1 | 1595.1 | 0.978 | 0.918 to 1.000 | 0.898 to 1.026 | pass |
| claude suspend | 15 | 667.5 | 677.4 | 1.017 | 0.899 to 1.168 | 0.848 to 1.231 | pass |
| claude move | 15 | 290.7 | 299.9 | 1.064 | 0.820 to 1.202 | 0.760 to 1.324 | pass |
| claude resume | 15 | 1306.1 | 1373.0 | 1.080 | 0.993 to 1.095 | 0.977 to 1.158 | pass |
| claude pull | 15 | 2312.2 | 2361.1 | 1.002 | 0.973 to 1.067 | 0.938 to 1.075 | pass |
| codex suspend | 15 | 723.2 | 721.4 | 0.972 | 0.903 to 1.029 | 0.849 to 1.040 | pass |
| codex move | 15 | 318.7 | 309.2 | 0.995 | 0.906 to 1.052 | 0.883 to 1.131 | pass |
| codex resume | 15 | 1418.1 | 1399.0 | 1.000 | 0.915 to 1.085 | 0.908 to 1.092 | pass |
| codex pull | 15 | 2324.1 | 2318.3 | 0.971 | 0.941 to 0.983 | 0.896 to 1.117 | pass |
| cold start | 15 | 103.7 | 70.2 | 0.933 | 0.491 to 1.321 | 0.364 to 1.713 | pass |
| restart | 15 | 141.1 | 170.9 | 1.085 | 0.992 to 1.191 | 0.848 to 1.196 | pass |

#### Memory

The family interval is the 95% interval across all 10 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10128.0 | 10080.0 | 0.998 | 0.995 to 1.002 | 0.994 to 1.005 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15392.0 | 15472.0 | 1.003 | 0.989 to 1.005 | 0.988 to 1.007 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3232.0 | 3216.0 | 1.000 | 0.971 to 1.005 | 0.970 to 1.009 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5825.0 | 5905.0 | 1.005 | 0.972 to 1.014 | 0.970 to 1.016 | pass |
| Daemon resident size, daemon a, KiB per counted op | 130 | 13968.0 | 13984.0 | 0.998 | 0.995 to 0.999 | 0.995 to 0.999 | pass |
| Daemon resident size, daemon b, KiB per counted op | 130 | 15568.0 | 15344.0 | 0.988 | 0.987 to 0.989 | 0.987 to 0.989 | pass |
| Daemon physical footprint, daemon a, KiB per counted op | 130 | 5377.0 | 5377.0 | 0.988 | 0.984 to 0.991 | 0.983 to 0.994 | pass |
| Daemon physical footprint, daemon b, KiB per counted op | 130 | 6113.0 | 5905.0 | 0.971 | 0.971 to 0.974 | 0.969 to 0.974 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5640.0 | 5672.0 | 1.000 | 1.000 to 1.003 | 0.998 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3305.0 | 3313.0 | 1.000 | 1.000 to 1.005 | 0.995 to 1.005 | pass |

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
