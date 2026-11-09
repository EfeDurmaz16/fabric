### Perf gate: fail

The first run failed Daemon resident size, daemon b, KiB after each timed run; Daemon physical footprint, daemon b, KiB after each timed run, so CI ran both builds again; the confirmation failed Daemon resident size, daemon b, KiB after each timed run; Daemon physical footprint, daemon b, KiB after each timed run. A check fails only when it fails in both runs: Daemon resident size, daemon b, KiB after each timed run; Daemon physical footprint, daemon b, KiB after each timed run.

### First run: fail

2 of 58 checks break the performance rule (CONTRIBUTING.md): Daemon resident size, daemon b, KiB after each timed run; Daemon physical footprint, daemon b, KiB after each timed run.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 228.7 | 228.9 | 1.014 | 0.864 to 1.289 | 0.751 to 1.527 | pass |
| acp launch | 15 | 34.2 | 34.2 | 1.014 | 0.897 to 1.035 | 0.892 to 1.036 | pass |
| acp approve | 15 | 7.4 | 7.4 | 1.033 | 0.979 to 1.070 | 0.930 to 1.119 | pass |
| claude new | 15 | 222.7 | 231.0 | 0.929 | 0.822 to 1.012 | 0.803 to 1.263 | pass |
| claude launch | 15 | 211.9 | 199.9 | 1.019 | 0.628 to 1.334 | 0.565 to 1.639 | pass |
| claude approve | 15 | 9.3 | 9.5 | 1.025 | 0.832 to 1.140 | 0.716 to 1.302 | pass |
| codex new | 15 | 268.0 | 246.0 | 0.994 | 0.809 to 1.047 | 0.764 to 1.178 | pass |
| codex launch | 15 | 224.7 | 211.8 | 0.914 | 0.746 to 1.188 | 0.638 to 1.496 | pass |
| codex approve | 15 | 7.6 | 7.6 | 1.008 | 0.967 to 1.125 | 0.967 to 1.144 | pass |
| ledger append | 15 | 7.8 | 7.6 | 0.975 | 0.913 to 1.023 | 0.903 to 1.097 | pass |
| cli version | 15 | 3.3 | 3.3 | 0.943 | 0.881 to 0.955 | 0.871 to 1.034 | pass |
| cli ls | 15 | 4.3 | 4.4 | 1.039 | 0.997 to 1.066 | 0.978 to 1.068 | pass |
| acp suspend | 15 | 499.4 | 481.4 | 0.929 | 0.892 to 1.025 | 0.876 to 1.040 | pass |
| acp move | 15 | 299.7 | 319.5 | 1.062 | 0.929 to 1.173 | 0.878 to 1.192 | pass |
| acp resume | 15 | 874.7 | 925.3 | 0.920 | 0.900 to 1.035 | 0.864 to 1.088 | pass |
| acp pull | 15 | 1849.0 | 1824.9 | 0.957 | 0.900 to 1.048 | 0.852 to 1.055 | pass |
| claude suspend | 15 | 679.1 | 635.0 | 0.969 | 0.857 to 1.049 | 0.852 to 1.111 | pass |
| claude move | 15 | 329.6 | 319.1 | 1.026 | 0.886 to 1.128 | 0.864 to 1.192 | pass |
| claude resume | 15 | 1471.5 | 1420.4 | 1.057 | 0.995 to 1.107 | 0.971 to 1.148 | pass |
| claude pull | 15 | 2337.0 | 2405.5 | 0.981 | 0.944 to 1.060 | 0.941 to 1.063 | pass |
| codex suspend | 15 | 743.4 | 716.9 | 0.879 | 0.851 to 1.041 | 0.830 to 1.076 | pass |
| codex move | 15 | 318.9 | 326.9 | 1.022 | 0.897 to 1.138 | 0.887 to 1.181 | pass |
| codex resume | 15 | 1320.1 | 1418.1 | 0.952 | 0.890 to 1.108 | 0.883 to 1.154 | pass |
| codex pull | 15 | 2468.2 | 2497.0 | 1.046 | 1.007 to 1.089 | 0.989 to 1.163 | pass; its 95% interval alone is above 1 |
| cold start | 15 | 88.2 | 73.5 | 0.802 | 0.650 to 1.065 | 0.551 to 1.186 | pass |
| restart | 15 | 177.3 | 175.5 | 1.039 | 0.957 to 1.119 | 0.847 to 1.235 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10064.0 | 10064.0 | 1.002 | 0.997 to 1.005 | 0.994 to 1.011 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15360.0 | 15504.0 | 1.008 | 1.002 to 1.009 | 1.002 to 1.023 | **fail** |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3184.0 | 3200.0 | 1.005 | 0.995 to 1.022 | 0.990 to 1.033 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5793.0 | 5921.0 | 1.020 | 1.003 to 1.023 | 1.003 to 1.050 | **fail** |
| CLI peak resident size, KiB per counted op | 130 | 5664.0 | 5648.0 | 1.000 | 0.997 to 1.002 | 0.997 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3305.0 | 3313.0 | 1.000 | 0.995 to 1.005 | 0.992 to 1.005 | pass |

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

### Confirmation run: fail

2 of 58 checks break the performance rule (CONTRIBUTING.md): Daemon resident size, daemon b, KiB after each timed run; Daemon physical footprint, daemon b, KiB after each timed run.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 254.8 | 212.1 | 0.938 | 0.705 to 1.180 | 0.653 to 1.283 | pass |
| acp launch | 15 | 23.8 | 24.1 | 1.010 | 0.948 to 1.068 | 0.947 to 1.071 | pass |
| acp approve | 15 | 5.6 | 5.5 | 1.016 | 0.975 to 1.050 | 0.932 to 1.070 | pass |
| claude new | 15 | 239.0 | 257.8 | 1.035 | 0.972 to 1.276 | 0.805 to 1.312 | pass |
| claude launch | 15 | 174.5 | 199.0 | 1.031 | 0.947 to 1.187 | 0.927 to 1.332 | pass |
| claude approve | 15 | 9.1 | 7.6 | 0.994 | 0.826 to 1.214 | 0.758 to 1.243 | pass |
| codex new | 15 | 233.0 | 250.7 | 0.989 | 0.884 to 1.216 | 0.790 to 1.387 | pass |
| codex launch | 15 | 235.4 | 226.4 | 0.918 | 0.741 to 1.254 | 0.719 to 1.509 | pass |
| codex approve | 15 | 7.2 | 7.0 | 1.000 | 0.897 to 1.181 | 0.812 to 1.206 | pass |
| ledger append | 15 | 6.4 | 6.3 | 0.969 | 0.913 to 1.034 | 0.907 to 1.045 | pass |
| cli version | 15 | 3.1 | 3.1 | 0.986 | 0.950 to 1.066 | 0.888 to 1.092 | pass |
| cli ls | 15 | 3.7 | 3.8 | 1.015 | 0.906 to 1.034 | 0.901 to 1.045 | pass |
| acp suspend | 15 | 465.3 | 462.1 | 0.987 | 0.874 to 1.110 | 0.783 to 1.142 | pass |
| acp move | 15 | 295.9 | 307.6 | 1.040 | 0.885 to 1.079 | 0.838 to 1.120 | pass |
| acp resume | 15 | 847.5 | 838.2 | 1.015 | 0.932 to 1.102 | 0.931 to 1.103 | pass |
| acp pull | 15 | 1561.6 | 1694.1 | 0.997 | 0.957 to 1.075 | 0.956 to 1.109 | pass |
| claude suspend | 15 | 626.7 | 651.0 | 0.983 | 0.923 to 1.167 | 0.914 to 1.198 | pass |
| claude move | 15 | 300.3 | 309.1 | 1.055 | 0.868 to 1.159 | 0.834 to 1.188 | pass |
| claude resume | 15 | 1285.0 | 1315.0 | 1.040 | 0.972 to 1.048 | 0.909 to 1.106 | pass |
| claude pull | 15 | 2233.7 | 2281.6 | 0.953 | 0.902 to 1.086 | 0.885 to 1.122 | pass |
| codex suspend | 15 | 702.9 | 690.2 | 0.974 | 0.889 to 1.048 | 0.882 to 1.136 | pass |
| codex move | 15 | 290.1 | 291.4 | 1.031 | 0.956 to 1.135 | 0.920 to 1.276 | pass |
| codex resume | 15 | 1355.6 | 1446.9 | 1.045 | 0.907 to 1.125 | 0.901 to 1.137 | pass |
| codex pull | 15 | 2292.7 | 2335.8 | 0.997 | 0.979 to 1.074 | 0.923 to 1.076 | pass |
| cold start | 15 | 51.3 | 59.0 | 0.959 | 0.888 to 1.080 | 0.690 to 1.163 | pass |
| restart | 15 | 146.6 | 167.1 | 0.915 | 0.807 to 1.250 | 0.718 to 1.255 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10096.0 | 0.998 | 0.995 to 1.006 | 0.995 to 1.006 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15328.0 | 15472.0 | 1.013 | 1.007 to 1.014 | 1.006 to 1.022 | **fail** |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3232.0 | 3265.0 | 1.000 | 0.977 to 1.020 | 0.975 to 1.020 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5761.0 | 5921.0 | 1.035 | 1.022 to 1.040 | 1.019 to 1.064 | **fail** |
| CLI peak resident size, KiB per counted op | 130 | 5656.0 | 5672.0 | 1.000 | 0.998 to 1.000 | 0.998 to 1.000 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3281.0 | 3281.0 | 0.998 | 0.995 to 1.000 | 0.995 to 1.002 | pass |

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
