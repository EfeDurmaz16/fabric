### Perf gate: pass

The first run failed none, so CI ran both builds again; the confirmation failed none. A check fails only when it fails in both runs: none.

### First run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 270.6 | 240.8 | 0.904 | 0.868 to 1.076 | 0.799 to 1.166 | pass |
| acp launch | 15 | 29.5 | 31.3 | 1.028 | 0.972 to 1.104 | 0.968 to 1.173 | pass |
| acp approve | 15 | 6.6 | 6.7 | 0.984 | 0.967 to 0.990 | 0.909 to 1.003 | pass |
| claude new | 15 | 265.8 | 222.4 | 0.915 | 0.644 to 1.095 | 0.556 to 1.373 | pass |
| claude launch | 15 | 188.0 | 213.0 | 1.095 | 0.943 to 1.331 | 0.752 to 1.436 | pass |
| claude approve | 15 | 8.1 | 8.8 | 0.994 | 0.939 to 1.235 | 0.871 to 1.403 | pass |
| codex new | 15 | 237.6 | 272.7 | 1.110 | 0.926 to 1.396 | 0.849 to 1.412 | pass |
| codex launch | 15 | 220.3 | 224.6 | 0.969 | 0.673 to 1.089 | 0.648 to 1.658 | pass |
| codex approve | 15 | 8.3 | 8.2 | 1.008 | 0.946 to 1.049 | 0.910 to 1.547 | pass |
| ledger append | 15 | 7.3 | 7.3 | 0.987 | 0.964 to 1.032 | 0.913 to 1.136 | pass |
| cli version | 15 | 3.8 | 3.5 | 0.968 | 0.948 to 1.021 | 0.901 to 1.049 | pass |
| cli ls | 15 | 4.6 | 4.4 | 1.033 | 0.993 to 1.185 | 0.903 to 1.300 | pass |
| acp suspend | 15 | 530.9 | 510.1 | 1.028 | 0.963 to 1.104 | 0.871 to 1.128 | pass |
| acp move | 15 | 384.1 | 344.8 | 1.030 | 0.902 to 1.088 | 0.848 to 1.230 | pass |
| acp resume | 15 | 916.0 | 959.5 | 1.045 | 0.901 to 1.130 | 0.892 to 1.181 | pass |
| acp pull | 15 | 1772.1 | 1795.5 | 1.052 | 0.999 to 1.108 | 0.923 to 1.121 | pass |
| claude suspend | 15 | 628.8 | 670.2 | 0.999 | 0.914 to 1.148 | 0.857 to 1.296 | pass |
| claude move | 15 | 326.4 | 303.2 | 0.930 | 0.879 to 1.112 | 0.861 to 1.151 | pass |
| claude resume | 15 | 1374.1 | 1496.1 | 1.074 | 0.981 to 1.176 | 0.947 to 1.209 | pass |
| claude pull | 15 | 2402.4 | 2496.3 | 0.993 | 0.966 to 1.031 | 0.894 to 1.099 | pass |
| codex suspend | 15 | 698.8 | 734.4 | 0.971 | 0.916 to 1.092 | 0.897 to 1.135 | pass |
| codex move | 15 | 337.5 | 336.6 | 1.014 | 0.991 to 1.124 | 0.916 to 1.126 | pass |
| codex resume | 15 | 1258.8 | 1346.1 | 0.997 | 0.859 to 1.120 | 0.850 to 1.135 | pass |
| codex pull | 15 | 2427.1 | 2416.9 | 1.005 | 0.949 to 1.057 | 0.916 to 1.062 | pass |
| cold start | 15 | 90.4 | 75.6 | 1.004 | 0.709 to 1.109 | 0.488 to 1.625 | pass |
| restart | 15 | 149.8 | 147.9 | 1.061 | 0.714 to 1.255 | 0.689 to 1.266 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10112.0 | 10128.0 | 1.002 | 0.998 to 1.008 | 0.997 to 1.014 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15376.0 | 15488.0 | 1.000 | 0.983 to 1.006 | 0.983 to 1.006 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3232.0 | 3249.0 | 1.005 | 1.005 to 1.027 | 1.000 to 1.031 | pass; its 95% interval alone is above 1 |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5809.0 | 5921.0 | 1.003 | 0.960 to 1.016 | 0.959 to 1.017 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5632.0 | 5656.0 | 1.000 | 0.997 to 1.000 | 0.997 to 1.000 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3297.0 | 3265.0 | 1.000 | 0.995 to 1.005 | 0.995 to 1.005 | pass |

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

### Confirmation run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 272.3 | 254.4 | 0.895 | 0.717 to 1.016 | 0.589 to 1.183 | pass |
| acp launch | 15 | 50.0 | 51.9 | 0.986 | 0.921 to 1.027 | 0.878 to 1.070 | pass |
| acp approve | 15 | 10.6 | 9.6 | 0.908 | 0.842 to 1.053 | 0.650 to 1.079 | pass |
| claude new | 15 | 265.9 | 238.1 | 1.012 | 0.713 to 1.072 | 0.551 to 1.147 | pass |
| claude launch | 15 | 130.9 | 250.8 | 1.322 | 0.954 to 2.139 | 0.823 to 2.299 | pass |
| claude approve | 15 | 12.2 | 11.5 | 0.975 | 0.865 to 1.097 | 0.806 to 1.178 | pass |
| codex new | 15 | 261.2 | 267.7 | 1.008 | 0.948 to 1.223 | 0.758 to 1.266 | pass |
| codex launch | 15 | 251.5 | 216.3 | 0.973 | 0.675 to 1.096 | 0.483 to 1.233 | pass |
| codex approve | 15 | 10.7 | 11.8 | 1.032 | 0.830 to 1.123 | 0.681 to 1.186 | pass |
| ledger append | 15 | 11.3 | 11.6 | 0.957 | 0.932 to 1.111 | 0.856 to 1.271 | pass |
| cli version | 15 | 4.6 | 5.1 | 0.984 | 0.908 to 1.163 | 0.893 to 1.331 | pass |
| cli ls | 15 | 6.8 | 5.6 | 0.941 | 0.741 to 0.962 | 0.720 to 1.020 | pass |
| acp suspend | 15 | 672.4 | 661.4 | 0.975 | 0.925 to 1.046 | 0.890 to 1.062 | pass |
| acp move | 15 | 432.7 | 446.1 | 1.028 | 0.896 to 1.116 | 0.878 to 1.135 | pass |
| acp resume | 15 | 1153.6 | 1206.8 | 1.038 | 0.955 to 1.146 | 0.901 to 1.153 | pass |
| acp pull | 15 | 2293.5 | 2213.3 | 0.969 | 0.909 to 1.035 | 0.904 to 1.055 | pass |
| claude suspend | 15 | 863.1 | 826.3 | 0.988 | 0.860 to 1.071 | 0.853 to 1.215 | pass |
| claude move | 15 | 445.7 | 438.8 | 1.047 | 0.942 to 1.107 | 0.895 to 1.144 | pass |
| claude resume | 15 | 1739.9 | 1777.5 | 0.988 | 0.899 to 1.068 | 0.890 to 1.151 | pass |
| claude pull | 15 | 2692.7 | 2866.3 | 1.039 | 0.979 to 1.110 | 0.882 to 1.115 | pass |
| codex suspend | 15 | 907.2 | 927.1 | 0.995 | 0.853 to 1.081 | 0.852 to 1.130 | pass |
| codex move | 15 | 462.4 | 447.1 | 1.004 | 0.949 to 1.037 | 0.882 to 1.075 | pass |
| codex resume | 15 | 1834.2 | 1713.5 | 0.948 | 0.903 to 1.082 | 0.843 to 1.158 | pass |
| codex pull | 15 | 3024.3 | 3067.4 | 0.963 | 0.933 to 1.038 | 0.918 to 1.062 | pass |
| cold start | 15 | 87.0 | 96.8 | 1.249 | 0.831 to 1.376 | 0.801 to 1.646 | pass |
| restart | 15 | 190.4 | 202.1 | 1.035 | 0.867 to 1.179 | 0.817 to 1.239 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10112.0 | 1.000 | 0.995 to 1.005 | 0.992 to 1.008 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15456.0 | 15504.0 | 0.997 | 0.994 to 1.004 | 0.985 to 1.009 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3248.0 | 3232.0 | 1.000 | 0.980 to 1.014 | 0.980 to 1.015 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5889.0 | 5969.0 | 0.995 | 0.988 to 1.018 | 0.967 to 1.023 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5640.0 | 5656.0 | 1.000 | 1.000 to 1.003 | 0.998 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3265.0 | 3297.0 | 1.000 | 0.995 to 1.005 | 0.995 to 1.005 | pass |

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
