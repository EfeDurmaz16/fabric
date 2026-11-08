### Perf gate: pass

The first run failed none, so CI ran both builds again; the confirmation failed ledger append. A check fails only when it fails in both runs: none.

### First run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 266.7 | 262.2 | 1.070 | 0.870 to 1.227 | 0.682 to 1.369 | pass |
| acp launch | 15 | 43.7 | 39.1 | 0.994 | 0.965 to 1.073 | 0.847 to 1.078 | pass |
| acp approve | 15 | 8.1 | 7.8 | 0.996 | 0.934 to 1.086 | 0.838 to 1.147 | pass |
| claude new | 15 | 175.7 | 231.2 | 1.157 | 0.876 to 1.515 | 0.796 to 1.725 | pass |
| claude launch | 15 | 156.0 | 199.4 | 1.262 | 0.858 to 1.410 | 0.765 to 1.510 | pass |
| claude approve | 15 | 8.6 | 9.7 | 0.954 | 0.864 to 1.227 | 0.861 to 1.298 | pass |
| codex new | 15 | 248.4 | 244.1 | 1.073 | 0.615 to 1.306 | 0.615 to 1.471 | pass |
| codex launch | 15 | 214.1 | 177.8 | 0.970 | 0.661 to 1.086 | 0.514 to 1.447 | pass |
| codex approve | 15 | 9.3 | 9.2 | 1.061 | 0.884 to 1.125 | 0.873 to 1.148 | pass |
| ledger append | 15 | 9.6 | 8.1 | 0.942 | 0.820 to 1.020 | 0.744 to 1.050 | pass |
| cli version | 15 | 3.7 | 3.6 | 0.984 | 0.924 to 1.248 | 0.896 to 1.274 | pass |
| cli ls | 15 | 5.4 | 4.7 | 1.031 | 0.934 to 1.159 | 0.812 to 1.176 | pass |
| acp suspend | 15 | 589.8 | 589.2 | 1.054 | 0.926 to 1.078 | 0.920 to 1.106 | pass |
| acp move | 15 | 348.7 | 337.5 | 0.999 | 0.948 to 1.086 | 0.897 to 1.125 | pass |
| acp resume | 15 | 1070.8 | 912.4 | 0.959 | 0.846 to 0.993 | 0.818 to 1.020 | pass |
| acp pull | 15 | 2010.1 | 1911.4 | 1.003 | 0.952 to 1.083 | 0.856 to 1.094 | pass |
| claude suspend | 15 | 777.8 | 730.1 | 1.052 | 0.911 to 1.174 | 0.855 to 1.224 | pass |
| claude move | 15 | 376.4 | 349.5 | 0.967 | 0.938 to 1.037 | 0.917 to 1.040 | pass |
| claude resume | 15 | 1439.2 | 1483.6 | 1.018 | 0.992 to 1.106 | 0.992 to 1.149 | pass |
| claude pull | 15 | 2514.8 | 2606.3 | 0.974 | 0.950 to 1.075 | 0.898 to 1.120 | pass |
| codex suspend | 15 | 794.7 | 807.2 | 0.996 | 0.955 to 1.067 | 0.941 to 1.102 | pass |
| codex move | 15 | 364.0 | 339.2 | 0.979 | 0.873 to 0.996 | 0.813 to 1.002 | pass |
| codex resume | 15 | 1496.3 | 1565.0 | 1.087 | 0.852 to 1.124 | 0.806 to 1.212 | pass |
| codex pull | 15 | 2671.5 | 2595.2 | 0.969 | 0.935 to 1.078 | 0.917 to 1.093 | pass |
| cold start | 15 | 73.5 | 87.2 | 1.100 | 0.971 to 1.287 | 0.789 to 1.419 | pass |
| restart | 15 | 207.8 | 180.3 | 1.057 | 0.984 to 1.167 | 0.855 to 1.186 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10064.0 | 10080.0 | 1.000 | 0.995 to 1.002 | 0.994 to 1.002 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15456.0 | 15376.0 | 0.992 | 0.991 to 0.995 | 0.991 to 0.996 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3216.0 | 3216.0 | 1.000 | 0.991 to 1.000 | 0.985 to 1.000 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5889.0 | 5809.0 | 0.984 | 0.978 to 0.987 | 0.976 to 0.989 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5656.0 | 5680.0 | 1.000 | 1.000 to 1.002 | 1.000 to 1.003 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3337.0 | 1.000 | 0.998 to 1.005 | 0.995 to 1.005 | pass |

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

1 of 58 checks break the performance rule (CONTRIBUTING.md): ledger append.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 268.8 | 220.2 | 0.856 | 0.684 to 1.052 | 0.634 to 1.062 | pass |
| acp launch | 15 | 29.5 | 28.9 | 1.026 | 0.953 to 1.049 | 0.831 to 1.093 | pass |
| acp approve | 15 | 6.5 | 7.0 | 1.047 | 1.002 to 1.084 | 0.959 to 1.110 | pass; its 95% interval alone is above 1 |
| claude new | 15 | 215.1 | 248.4 | 1.064 | 0.919 to 1.290 | 0.843 to 1.580 | pass |
| claude launch | 15 | 229.9 | 206.7 | 0.828 | 0.670 to 1.207 | 0.568 to 1.714 | pass |
| claude approve | 15 | 8.2 | 7.5 | 0.963 | 0.796 to 1.101 | 0.783 to 1.244 | pass |
| codex new | 15 | 248.8 | 229.6 | 0.958 | 0.843 to 1.088 | 0.656 to 1.109 | pass |
| codex launch | 15 | 244.3 | 238.2 | 0.946 | 0.833 to 1.275 | 0.744 to 1.387 | pass |
| codex approve | 15 | 7.4 | 8.3 | 1.028 | 0.915 to 1.172 | 0.859 to 1.316 | pass |
| ledger append | 15 | 6.8 | 7.5 | 1.034 | 1.022 to 1.083 | 1.016 to 1.112 | **fail** |
| cli version | 15 | 3.1 | 3.3 | 1.019 | 0.970 to 1.126 | 0.929 to 1.146 | pass |
| cli ls | 15 | 4.3 | 3.7 | 0.985 | 0.888 to 1.018 | 0.841 to 1.031 | pass |
| acp suspend | 15 | 460.7 | 482.1 | 1.034 | 0.920 to 1.129 | 0.886 to 1.205 | pass |
| acp move | 15 | 304.2 | 301.9 | 1.032 | 0.939 to 1.056 | 0.805 to 1.182 | pass |
| acp resume | 15 | 854.8 | 834.1 | 0.961 | 0.858 to 1.184 | 0.772 to 1.267 | pass |
| acp pull | 15 | 1569.4 | 1626.5 | 1.019 | 0.996 to 1.061 | 0.934 to 1.112 | pass |
| claude suspend | 15 | 663.0 | 609.9 | 0.929 | 0.799 to 1.076 | 0.780 to 1.132 | pass |
| claude move | 15 | 315.1 | 306.9 | 0.985 | 0.940 to 1.061 | 0.917 to 1.088 | pass |
| claude resume | 15 | 1281.2 | 1291.8 | 1.008 | 0.876 to 1.068 | 0.797 to 1.083 | pass |
| claude pull | 15 | 2332.5 | 2242.2 | 0.978 | 0.938 to 1.028 | 0.921 to 1.064 | pass |
| codex suspend | 15 | 667.5 | 697.8 | 1.027 | 0.948 to 1.163 | 0.947 to 1.196 | pass |
| codex move | 15 | 298.5 | 303.7 | 0.999 | 0.966 to 1.254 | 0.937 to 1.270 | pass |
| codex resume | 15 | 1419.2 | 1299.4 | 0.970 | 0.906 to 1.110 | 0.800 to 1.179 | pass |
| codex pull | 15 | 2270.3 | 2361.8 | 0.985 | 0.921 to 1.047 | 0.897 to 1.065 | pass |
| cold start | 15 | 87.5 | 85.5 | 0.997 | 0.547 to 1.329 | 0.486 to 2.469 | pass |
| restart | 15 | 176.8 | 165.0 | 0.881 | 0.820 to 1.174 | 0.774 to 1.210 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10144.0 | 10064.0 | 0.995 | 0.992 to 1.002 | 0.990 to 1.005 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15488.0 | 15536.0 | 1.002 | 0.996 to 1.004 | 0.993 to 1.004 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3248.0 | 3200.0 | 0.996 | 0.985 to 1.015 | 0.971 to 1.023 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5921.0 | 5969.0 | 1.005 | 0.991 to 1.013 | 0.981 to 1.013 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5608.0 | 5624.0 | 1.000 | 0.997 to 1.000 | 0.997 to 1.002 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3313.0 | 3313.0 | 0.998 | 0.995 to 1.000 | 0.994 to 1.004 | pass |

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
