### Perf gate: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 254.7 | 246.9 | 0.996 | 0.672 to 1.200 | 0.610 to 1.418 | pass |
| acp launch | 15 | 28.1 | 25.1 | 0.968 | 0.840 to 1.084 | 0.804 to 1.156 | pass |
| acp approve | 15 | 6.4 | 6.3 | 0.969 | 0.936 to 1.018 | 0.922 to 1.100 | pass |
| claude new | 15 | 242.1 | 242.5 | 0.938 | 0.852 to 1.217 | 0.692 to 1.283 | pass |
| claude launch | 15 | 184.7 | 149.2 | 0.926 | 0.706 to 0.960 | 0.572 to 1.129 | pass |
| claude approve | 15 | 7.6 | 7.5 | 0.986 | 0.880 to 1.047 | 0.658 to 1.100 | pass |
| codex new | 15 | 254.7 | 242.9 | 0.967 | 0.893 to 1.238 | 0.843 to 1.350 | pass |
| codex launch | 15 | 218.8 | 249.9 | 1.127 | 0.839 to 1.323 | 0.663 to 1.438 | pass |
| codex approve | 15 | 8.0 | 8.2 | 1.000 | 0.972 to 1.186 | 0.804 to 1.206 | pass |
| ledger append | 15 | 8.0 | 7.5 | 0.941 | 0.823 to 1.035 | 0.762 to 1.040 | pass |
| cli version | 15 | 3.6 | 3.8 | 1.006 | 0.929 to 1.116 | 0.887 to 1.286 | pass |
| cli ls | 15 | 4.3 | 4.2 | 0.995 | 0.956 to 1.016 | 0.932 to 1.134 | pass |
| acp suspend | 15 | 481.4 | 473.9 | 1.023 | 0.941 to 1.079 | 0.878 to 1.224 | pass |
| acp move | 15 | 311.2 | 300.4 | 0.910 | 0.830 to 1.044 | 0.819 to 1.071 | pass |
| acp resume | 15 | 837.2 | 794.5 | 0.916 | 0.886 to 1.070 | 0.861 to 1.121 | pass |
| acp pull | 15 | 1650.3 | 1631.7 | 1.009 | 0.955 to 1.043 | 0.930 to 1.075 | pass |
| claude suspend | 15 | 616.4 | 640.6 | 1.026 | 0.973 to 1.114 | 0.822 to 1.222 | pass |
| claude move | 15 | 306.5 | 303.1 | 0.948 | 0.907 to 1.019 | 0.881 to 1.102 | pass |
| claude resume | 15 | 1357.5 | 1266.0 | 0.904 | 0.876 to 0.929 | 0.860 to 0.995 | pass |
| claude pull | 15 | 2385.4 | 2264.4 | 0.951 | 0.913 to 0.967 | 0.902 to 1.027 | pass |
| codex suspend | 15 | 696.8 | 663.6 | 0.938 | 0.903 to 1.039 | 0.888 to 1.044 | pass |
| codex move | 15 | 292.7 | 314.1 | 1.012 | 0.956 to 1.104 | 0.922 to 1.134 | pass |
| codex resume | 15 | 1327.4 | 1349.5 | 0.973 | 0.953 to 1.061 | 0.896 to 1.088 | pass |
| codex pull | 15 | 2238.3 | 2265.9 | 1.033 | 1.011 to 1.082 | 0.969 to 1.165 | pass; its 95% interval alone is above 1 |
| cold start | 15 | 83.7 | 77.5 | 1.012 | 0.855 to 1.419 | 0.609 to 1.527 | pass |
| restart | 15 | 142.5 | 146.7 | 1.091 | 0.939 to 1.362 | 0.833 to 1.372 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10080.0 | 0.998 | 0.998 to 1.003 | 0.995 to 1.005 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15328.0 | 15344.0 | 1.005 | 1.000 to 1.007 | 1.000 to 1.010 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3248.0 | 3216.0 | 1.000 | 1.000 to 1.010 | 0.995 to 1.014 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5744.0 | 5760.0 | 1.016 | 1.000 to 1.018 | 1.000 to 1.026 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5656.0 | 5648.0 | 1.000 | 0.997 to 1.000 | 0.997 to 1.000 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3313.0 | 3321.0 | 1.000 | 0.995 to 1.002 | 0.995 to 1.005 | pass |

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
