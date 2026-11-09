### Perf gate: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 219.7 | 208.3 | 0.949 | 0.729 to 1.306 | 0.646 to 1.465 | pass |
| acp launch | 15 | 34.3 | 29.8 | 1.030 | 0.913 to 1.102 | 0.882 to 1.169 | pass |
| acp approve | 15 | 6.8 | 7.2 | 1.016 | 0.961 to 1.053 | 0.953 to 1.068 | pass |
| claude new | 15 | 206.0 | 237.1 | 1.146 | 0.975 to 1.305 | 0.792 to 1.743 | pass |
| claude launch | 15 | 160.2 | 182.1 | 1.217 | 0.933 to 1.470 | 0.664 to 1.837 | pass |
| claude approve | 15 | 8.0 | 8.6 | 1.022 | 0.857 to 1.261 | 0.839 to 1.671 | pass |
| codex new | 15 | 218.7 | 259.9 | 1.122 | 1.033 to 1.420 | 0.870 to 1.598 | pass; its 95% interval alone is above 1 |
| codex launch | 15 | 214.4 | 232.4 | 1.077 | 0.801 to 1.392 | 0.616 to 2.133 | pass |
| codex approve | 15 | 9.5 | 8.6 | 1.041 | 0.815 to 1.182 | 0.772 to 1.248 | pass |
| ledger append | 15 | 7.6 | 8.0 | 1.035 | 0.993 to 1.141 | 0.975 to 1.165 | pass |
| cli version | 15 | 3.4 | 3.6 | 1.031 | 0.984 to 1.082 | 0.941 to 1.398 | pass |
| cli ls | 15 | 4.1 | 4.4 | 1.003 | 0.935 to 1.087 | 0.910 to 1.100 | pass |
| acp suspend | 15 | 521.1 | 509.3 | 1.059 | 0.924 to 1.089 | 0.860 to 1.171 | pass |
| acp move | 15 | 305.4 | 330.4 | 1.049 | 0.981 to 1.079 | 0.796 to 1.172 | pass |
| acp resume | 15 | 955.5 | 961.8 | 1.038 | 0.947 to 1.148 | 0.921 to 1.217 | pass |
| acp pull | 15 | 1813.9 | 1632.7 | 0.937 | 0.825 to 1.006 | 0.825 to 1.037 | pass |
| claude suspend | 15 | 665.3 | 685.2 | 1.046 | 0.949 to 1.089 | 0.874 to 1.110 | pass |
| claude move | 15 | 313.3 | 314.6 | 0.985 | 0.894 to 1.111 | 0.851 to 1.192 | pass |
| claude resume | 15 | 1353.6 | 1399.0 | 1.010 | 0.987 to 1.087 | 0.960 to 1.087 | pass |
| claude pull | 15 | 2252.9 | 2201.7 | 0.998 | 0.895 to 1.044 | 0.891 to 1.102 | pass |
| codex suspend | 15 | 690.2 | 715.8 | 0.958 | 0.921 to 1.141 | 0.882 to 1.187 | pass |
| codex move | 15 | 295.3 | 314.9 | 1.011 | 0.987 to 1.197 | 0.908 to 1.200 | pass |
| codex resume | 15 | 1384.3 | 1363.7 | 0.953 | 0.880 to 1.006 | 0.822 to 1.025 | pass |
| codex pull | 15 | 2342.5 | 2493.1 | 1.009 | 0.949 to 1.069 | 0.884 to 1.175 | pass |
| cold start | 15 | 83.7 | 78.9 | 1.309 | 0.803 to 1.427 | 0.653 to 2.010 | pass |
| restart | 15 | 156.2 | 163.9 | 0.946 | 0.870 to 1.050 | 0.819 to 1.066 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10112.0 | 1.000 | 0.997 to 1.005 | 0.995 to 1.005 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15472.0 | 15456.0 | 0.999 | 0.995 to 1.005 | 0.995 to 1.014 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3248.0 | 3232.0 | 1.005 | 0.989 to 1.027 | 0.980 to 1.028 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5905.0 | 5889.0 | 0.997 | 0.986 to 1.014 | 0.986 to 1.039 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5672.0 | 5648.0 | 1.000 | 1.000 to 1.000 | 0.998 to 1.002 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3313.0 | 3305.0 | 1.000 | 0.995 to 1.002 | 0.995 to 1.005 | pass |

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
