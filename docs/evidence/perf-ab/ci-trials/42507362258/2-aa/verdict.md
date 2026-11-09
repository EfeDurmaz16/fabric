### Perf gate: pass

The first run failed acp pull, so CI ran both builds again; the confirmation failed Daemon physical footprint, daemon b, KiB after each timed run. A check fails only when it fails in both runs: none.

### First run: fail

1 of 58 checks break the performance rule (CONTRIBUTING.md): acp pull.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 257.1 | 237.2 | 0.957 | 0.921 to 1.025 | 0.842 to 1.155 | pass |
| acp launch | 15 | 25.3 | 25.7 | 0.989 | 0.961 to 1.021 | 0.895 to 1.059 | pass |
| acp approve | 15 | 6.0 | 6.2 | 1.000 | 0.970 to 1.030 | 0.937 to 1.099 | pass |
| claude new | 15 | 223.0 | 246.3 | 1.111 | 1.034 to 1.447 | 0.958 to 1.452 | pass; its 95% interval alone is above 1 |
| claude launch | 15 | 212.9 | 230.9 | 1.097 | 0.935 to 1.171 | 0.576 to 1.233 | pass |
| claude approve | 15 | 7.6 | 8.0 | 0.972 | 0.802 to 1.211 | 0.682 to 1.462 | pass |
| codex new | 15 | 232.9 | 249.8 | 0.968 | 0.803 to 1.021 | 0.761 to 1.639 | pass |
| codex launch | 15 | 226.1 | 242.5 | 1.202 | 0.940 to 1.425 | 0.527 to 1.630 | pass |
| codex approve | 15 | 6.8 | 7.3 | 0.984 | 0.939 to 1.432 | 0.777 to 1.563 | pass |
| ledger append | 15 | 6.4 | 6.3 | 0.961 | 0.930 to 1.031 | 0.910 to 1.033 | pass |
| cli version | 15 | 2.9 | 3.0 | 1.030 | 0.967 to 1.087 | 0.966 to 1.094 | pass |
| cli ls | 15 | 3.9 | 3.7 | 0.945 | 0.916 to 1.000 | 0.906 to 1.009 | pass |
| acp suspend | 15 | 463.9 | 461.8 | 1.025 | 0.962 to 1.045 | 0.908 to 1.278 | pass |
| acp move | 15 | 305.6 | 286.5 | 0.953 | 0.929 to 0.993 | 0.870 to 1.014 | pass |
| acp resume | 15 | 860.7 | 921.7 | 0.984 | 0.961 to 1.087 | 0.891 to 1.103 | pass |
| acp pull | 15 | 1622.1 | 1601.2 | 1.037 | 1.031 to 1.089 | 1.004 to 1.106 | **fail** |
| claude suspend | 15 | 628.4 | 652.2 | 1.010 | 0.896 to 1.119 | 0.852 to 1.155 | pass |
| claude move | 15 | 312.3 | 295.8 | 1.031 | 0.859 to 1.322 | 0.825 to 1.354 | pass |
| claude resume | 15 | 1354.2 | 1242.6 | 0.990 | 0.903 to 1.064 | 0.869 to 1.113 | pass |
| claude pull | 15 | 2235.3 | 2223.9 | 0.987 | 0.948 to 1.080 | 0.906 to 1.107 | pass |
| codex suspend | 15 | 684.9 | 693.7 | 1.015 | 0.951 to 1.069 | 0.870 to 1.134 | pass |
| codex move | 15 | 279.4 | 283.1 | 1.035 | 0.891 to 1.081 | 0.876 to 1.200 | pass |
| codex resume | 15 | 1319.5 | 1251.8 | 0.969 | 0.868 to 1.115 | 0.867 to 1.163 | pass |
| codex pull | 15 | 2358.5 | 2319.4 | 1.003 | 0.956 to 1.028 | 0.921 to 1.030 | pass |
| cold start | 15 | 59.1 | 75.5 | 1.267 | 0.729 to 1.582 | 0.722 to 1.731 | pass |
| restart | 15 | 140.1 | 157.4 | 1.095 | 0.980 to 1.153 | 0.788 to 1.482 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10112.0 | 0.998 | 0.994 to 1.002 | 0.992 to 1.002 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15392.0 | 15488.0 | 1.006 | 0.998 to 1.011 | 0.998 to 1.012 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3265.0 | 3232.0 | 0.990 | 0.981 to 1.000 | 0.977 to 1.005 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5841.0 | 5905.0 | 1.011 | 0.993 to 1.029 | 0.993 to 1.034 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5656.0 | 5632.0 | 1.000 | 0.997 to 1.000 | 0.996 to 1.002 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3321.0 | 0.998 | 0.994 to 1.000 | 0.991 to 1.005 | pass |

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

1 of 58 checks break the performance rule (CONTRIBUTING.md): Daemon physical footprint, daemon b, KiB after each timed run.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 240.0 | 238.2 | 1.019 | 0.728 to 1.223 | 0.695 to 1.595 | pass |
| acp launch | 15 | 28.3 | 25.9 | 1.014 | 0.914 to 1.025 | 0.914 to 1.049 | pass |
| acp approve | 15 | 6.1 | 6.2 | 1.048 | 0.932 to 1.132 | 0.930 to 1.132 | pass |
| claude new | 15 | 251.4 | 236.9 | 0.991 | 0.651 to 1.082 | 0.621 to 1.165 | pass |
| claude launch | 15 | 174.3 | 209.9 | 1.279 | 1.002 to 2.014 | 0.607 to 2.135 | pass; its 95% interval alone is above 1 |
| claude approve | 15 | 7.1 | 6.9 | 0.995 | 0.665 to 1.123 | 0.661 to 1.158 | pass |
| codex new | 15 | 240.7 | 243.9 | 0.954 | 0.769 to 1.085 | 0.691 to 1.099 | pass |
| codex launch | 15 | 199.6 | 253.5 | 1.069 | 0.821 to 1.423 | 0.668 to 1.667 | pass |
| codex approve | 15 | 7.0 | 6.7 | 0.996 | 0.883 to 1.074 | 0.781 to 1.094 | pass |
| ledger append | 15 | 6.8 | 6.7 | 0.975 | 0.944 to 1.068 | 0.926 to 1.071 | pass |
| cli version | 15 | 3.0 | 3.0 | 0.976 | 0.942 to 1.074 | 0.926 to 1.093 | pass |
| cli ls | 15 | 3.7 | 3.8 | 1.005 | 0.981 to 1.052 | 0.969 to 1.072 | pass |
| acp suspend | 15 | 457.9 | 471.4 | 1.011 | 0.963 to 1.057 | 0.897 to 1.126 | pass |
| acp move | 15 | 289.5 | 312.1 | 1.022 | 0.955 to 1.133 | 0.913 to 1.169 | pass |
| acp resume | 15 | 920.4 | 852.6 | 1.004 | 0.968 to 1.090 | 0.918 to 1.098 | pass |
| acp pull | 15 | 1585.3 | 1590.9 | 0.988 | 0.930 to 1.063 | 0.913 to 1.063 | pass |
| claude suspend | 15 | 620.2 | 607.0 | 1.008 | 0.934 to 1.053 | 0.833 to 1.075 | pass |
| claude move | 15 | 297.4 | 300.7 | 0.959 | 0.844 to 1.011 | 0.796 to 1.054 | pass |
| claude resume | 15 | 1319.3 | 1310.8 | 1.021 | 0.904 to 1.086 | 0.839 to 1.105 | pass |
| claude pull | 15 | 2228.7 | 2145.4 | 0.987 | 0.922 to 1.008 | 0.905 to 1.029 | pass |
| codex suspend | 15 | 657.0 | 691.5 | 1.032 | 0.849 to 1.094 | 0.817 to 1.165 | pass |
| codex move | 15 | 307.2 | 286.5 | 0.961 | 0.881 to 1.017 | 0.832 to 1.030 | pass |
| codex resume | 15 | 1318.6 | 1360.3 | 1.027 | 0.959 to 1.161 | 0.906 to 1.176 | pass |
| codex pull | 15 | 2342.4 | 2307.2 | 0.999 | 0.948 to 1.030 | 0.927 to 1.038 | pass |
| cold start | 15 | 72.3 | 67.7 | 0.872 | 0.590 to 1.175 | 0.585 to 1.718 | pass |
| restart | 15 | 147.7 | 131.2 | 0.926 | 0.829 to 0.991 | 0.806 to 1.131 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10096.0 | 1.002 | 0.992 to 1.003 | 0.991 to 1.005 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15440.0 | 15504.0 | 1.003 | 1.000 to 1.004 | 0.999 to 1.004 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3232.0 | 3216.0 | 1.000 | 0.970 to 1.014 | 0.969 to 1.020 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5872.0 | 5953.0 | 1.013 | 1.003 to 1.015 | 1.003 to 1.015 | **fail** |
| CLI peak resident size, KiB per counted op | 130 | 5600.0 | 5656.0 | 1.000 | 0.999 to 1.002 | 0.998 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3281.0 | 3345.0 | 1.000 | 0.996 to 1.005 | 0.995 to 1.005 | pass |

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
