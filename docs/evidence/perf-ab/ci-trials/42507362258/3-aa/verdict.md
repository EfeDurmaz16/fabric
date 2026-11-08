### Perf gate: pass

The first run failed none, so CI ran both builds again; the confirmation failed none. A check fails only when it fails in both runs: none.

### First run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 257.9 | 231.6 | 0.926 | 0.737 to 1.094 | 0.706 to 1.249 | pass |
| acp launch | 15 | 29.4 | 31.2 | 1.033 | 0.957 to 1.104 | 0.923 to 1.179 | pass |
| acp approve | 15 | 6.1 | 6.5 | 1.034 | 0.973 to 1.050 | 0.943 to 1.120 | pass |
| claude new | 15 | 243.6 | 257.3 | 1.041 | 0.878 to 1.212 | 0.779 to 1.830 | pass |
| claude launch | 15 | 210.5 | 167.5 | 0.920 | 0.637 to 1.000 | 0.514 to 1.169 | pass |
| claude approve | 15 | 8.2 | 7.8 | 0.978 | 0.947 to 1.066 | 0.595 to 1.084 | pass |
| codex new | 15 | 257.6 | 259.0 | 1.011 | 0.873 to 1.082 | 0.855 to 1.164 | pass |
| codex launch | 15 | 197.9 | 243.8 | 1.066 | 0.931 to 1.142 | 0.864 to 1.317 | pass |
| codex approve | 15 | 9.7 | 10.2 | 0.967 | 0.906 to 1.059 | 0.731 to 1.246 | pass |
| ledger append | 15 | 8.9 | 8.5 | 0.955 | 0.806 to 1.032 | 0.791 to 1.215 | pass |
| cli version | 15 | 3.9 | 3.5 | 0.955 | 0.896 to 1.083 | 0.842 to 1.106 | pass |
| cli ls | 15 | 4.6 | 4.5 | 1.000 | 0.974 to 1.084 | 0.945 to 1.239 | pass |
| acp suspend | 15 | 478.3 | 524.5 | 0.978 | 0.925 to 1.139 | 0.865 to 1.193 | pass |
| acp move | 15 | 310.3 | 344.0 | 1.036 | 0.940 to 1.175 | 0.920 to 1.189 | pass |
| acp resume | 15 | 984.6 | 969.4 | 1.038 | 0.923 to 1.124 | 0.892 to 1.147 | pass |
| acp pull | 15 | 1716.2 | 1805.7 | 0.959 | 0.923 to 1.039 | 0.896 to 1.067 | pass |
| claude suspend | 15 | 653.1 | 695.5 | 1.022 | 0.978 to 1.144 | 0.970 to 1.250 | pass |
| claude move | 15 | 307.1 | 343.8 | 1.087 | 0.948 to 1.130 | 0.946 to 1.247 | pass |
| claude resume | 15 | 1410.4 | 1359.0 | 0.972 | 0.912 to 1.096 | 0.905 to 1.152 | pass |
| claude pull | 15 | 2266.7 | 2382.8 | 1.015 | 0.979 to 1.115 | 0.913 to 1.118 | pass |
| codex suspend | 15 | 743.6 | 723.4 | 1.017 | 0.922 to 1.063 | 0.787 to 1.102 | pass |
| codex move | 15 | 290.9 | 309.9 | 1.054 | 0.942 to 1.200 | 0.885 to 1.258 | pass |
| codex resume | 15 | 1381.6 | 1372.0 | 1.019 | 0.929 to 1.096 | 0.920 to 1.154 | pass |
| codex pull | 15 | 2430.5 | 2466.1 | 0.980 | 0.917 to 1.071 | 0.904 to 1.082 | pass |
| cold start | 15 | 65.4 | 82.5 | 1.120 | 0.941 to 1.904 | 0.775 to 2.091 | pass |
| restart | 15 | 180.5 | 162.4 | 0.978 | 0.816 to 1.208 | 0.743 to 1.406 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 10080.0 | 0.998 | 0.997 to 1.006 | 0.995 to 1.008 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15568.0 | 15408.0 | 0.994 | 0.990 to 1.003 | 0.981 to 1.003 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3233.0 | 3249.0 | 0.995 | 0.990 to 1.025 | 0.990 to 1.026 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 6017.0 | 5857.0 | 0.985 | 0.976 to 1.008 | 0.948 to 1.008 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5680.0 | 5632.0 | 1.000 | 0.998 to 1.000 | 0.997 to 1.002 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3297.0 | 3273.0 | 1.000 | 0.995 to 1.005 | 0.995 to 1.005 | pass |

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
| acp new | 15 | 270.7 | 179.7 | 0.737 | 0.673 to 0.894 | 0.587 to 0.967 | pass |
| acp launch | 15 | 37.1 | 32.2 | 0.973 | 0.861 to 1.004 | 0.810 to 1.080 | pass |
| acp approve | 15 | 7.0 | 7.2 | 0.986 | 0.939 to 1.015 | 0.911 to 1.031 | pass |
| claude new | 15 | 219.5 | 244.5 | 1.088 | 0.924 to 1.747 | 0.803 to 1.856 | pass |
| claude launch | 15 | 160.2 | 201.0 | 1.122 | 0.887 to 1.643 | 0.833 to 2.076 | pass |
| claude approve | 15 | 8.0 | 7.3 | 0.983 | 0.932 to 1.135 | 0.923 to 1.474 | pass |
| codex new | 15 | 257.9 | 235.8 | 0.959 | 0.878 to 1.060 | 0.714 to 1.067 | pass |
| codex launch | 15 | 215.9 | 212.9 | 1.081 | 0.796 to 1.351 | 0.701 to 1.378 | pass |
| codex approve | 15 | 7.5 | 7.9 | 1.032 | 0.913 to 1.101 | 0.776 to 1.263 | pass |
| ledger append | 15 | 7.2 | 6.9 | 0.985 | 0.922 to 1.063 | 0.906 to 1.068 | pass |
| cli version | 15 | 3.1 | 3.3 | 1.032 | 0.945 to 1.054 | 0.893 to 1.109 | pass |
| cli ls | 15 | 4.3 | 4.1 | 0.983 | 0.944 to 1.052 | 0.844 to 1.071 | pass |
| acp suspend | 15 | 509.4 | 500.5 | 1.043 | 0.979 to 1.099 | 0.957 to 1.151 | pass |
| acp move | 15 | 309.4 | 306.4 | 0.982 | 0.924 to 1.106 | 0.887 to 1.221 | pass |
| acp resume | 15 | 942.6 | 928.7 | 0.992 | 0.968 to 1.163 | 0.932 to 1.198 | pass |
| acp pull | 15 | 1816.3 | 1690.0 | 1.016 | 0.936 to 1.047 | 0.886 to 1.071 | pass |
| claude suspend | 15 | 634.9 | 711.9 | 1.076 | 0.873 to 1.184 | 0.853 to 1.231 | pass |
| claude move | 15 | 307.7 | 320.8 | 1.027 | 0.953 to 1.073 | 0.917 to 1.149 | pass |
| claude resume | 15 | 1418.8 | 1405.2 | 1.002 | 0.928 to 1.095 | 0.910 to 1.142 | pass |
| claude pull | 15 | 2370.2 | 2260.0 | 1.002 | 0.918 to 1.062 | 0.906 to 1.073 | pass |
| codex suspend | 15 | 761.5 | 731.6 | 0.966 | 0.902 to 1.076 | 0.772 to 1.086 | pass |
| codex move | 15 | 330.5 | 316.5 | 0.995 | 0.898 to 1.063 | 0.856 to 1.073 | pass |
| codex resume | 15 | 1501.0 | 1368.6 | 1.022 | 0.876 to 1.046 | 0.849 to 1.147 | pass |
| codex pull | 15 | 2429.7 | 2429.9 | 0.976 | 0.912 to 1.044 | 0.906 to 1.065 | pass |
| cold start | 15 | 70.2 | 55.2 | 1.011 | 0.499 to 1.189 | 0.473 to 1.680 | pass |
| restart | 15 | 147.5 | 142.2 | 1.075 | 0.872 to 1.130 | 0.837 to 1.145 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10112.0 | 10112.0 | 1.000 | 0.997 to 1.002 | 0.994 to 1.003 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15568.0 | 15536.0 | 1.006 | 1.001 to 1.013 | 0.998 to 1.013 | pass; its 95% interval alone is above 1 |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3216.0 | 3232.0 | 1.000 | 0.990 to 1.014 | 0.985 to 1.015 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 6001.0 | 6001.0 | 1.019 | 1.008 to 1.041 | 1.000 to 1.041 | pass; its 95% interval alone is above 1 |
| CLI peak resident size, KiB per counted op | 130 | 5632.0 | 5640.0 | 1.000 | 1.000 to 1.002 | 0.998 to 1.002 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3281.0 | 3281.0 | 1.000 | 1.000 to 1.005 | 0.995 to 1.005 | pass |

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
