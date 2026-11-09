### Perf gate: pass

All 62 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 257.0 | 61.4 | 0.273 | 0.238 to 0.308 | 0.218 to 0.355 | pass |
| acp launch | 15 | 38.5 | 25.3 | 0.638 | 0.609 to 0.721 | 0.548 to 0.734 | pass |
| acp approve | 15 | 8.6 | 7.2 | 0.849 | 0.782 to 0.908 | 0.773 to 0.917 | pass |
| claude new | 15 | 272.7 | 56.8 | 0.210 | 0.200 to 0.277 | 0.197 to 0.300 | pass |
| claude launch | 15 | 199.4 | 197.5 | 0.954 | 0.743 to 1.214 | 0.590 to 1.351 | pass |
| claude approve | 15 | 8.5 | 7.5 | 0.877 | 0.820 to 1.200 | 0.584 to 1.210 | pass |
| codex new | 15 | 281.8 | 55.4 | 0.204 | 0.183 to 0.246 | 0.178 to 0.254 | pass |
| codex launch | 15 | 207.7 | 250.8 | 1.095 | 0.902 to 1.471 | 0.742 to 1.919 | pass |
| codex approve | 15 | 9.2 | 7.2 | 0.825 | 0.755 to 0.973 | 0.723 to 1.030 | pass |
| ledger append | 15 | 8.8 | 7.6 | 0.850 | 0.819 to 0.881 | 0.818 to 0.948 | pass |
| cli version | 15 | 3.8 | 3.6 | 0.977 | 0.935 to 1.001 | 0.917 to 1.020 | pass |
| cli ls | 15 | 4.7 | 4.5 | 0.953 | 0.917 to 0.989 | 0.880 to 1.016 | pass |
| acp suspend | 15 | 504.8 | 485.1 | 0.926 | 0.880 to 0.975 | 0.808 to 1.095 | pass |
| acp move | 15 | 327.0 | 231.0 | 0.690 | 0.634 to 0.802 | 0.630 to 0.856 | pass |
| acp resume | 15 | 1006.4 | 363.4 | 0.361 | 0.311 to 0.453 | 0.252 to 0.498 | pass |
| acp pull | 15 | 1793.9 | 1078.6 | 0.623 | 0.547 to 0.711 | 0.519 to 0.714 | pass |
| claude suspend | 15 | 668.2 | 475.7 | 0.732 | 0.695 to 0.783 | 0.651 to 0.784 | pass |
| claude move | 15 | 340.2 | 249.8 | 0.769 | 0.708 to 0.844 | 0.653 to 0.859 | pass |
| claude resume | 15 | 1465.2 | 668.2 | 0.466 | 0.416 to 0.492 | 0.370 to 0.518 | pass |
| claude pull | 15 | 2515.5 | 1391.5 | 0.572 | 0.527 to 0.616 | 0.507 to 0.640 | pass |
| codex suspend | 15 | 796.3 | 572.2 | 0.709 | 0.647 to 0.797 | 0.641 to 0.851 | pass |
| codex move | 15 | 341.4 | 270.1 | 0.777 | 0.645 to 0.809 | 0.625 to 0.842 | pass |
| codex resume | 15 | 1520.6 | 725.6 | 0.467 | 0.403 to 0.532 | 0.390 to 0.544 | pass |
| codex pull | 15 | 2579.9 | 1464.5 | 0.562 | 0.540 to 0.648 | 0.483 to 0.650 | pass |
| cold start | 15 | 81.9 | 93.9 | 0.834 | 0.567 to 1.910 | 0.520 to 2.138 | pass |
| restart | 15 | 176.6 | 84.9 | 0.501 | 0.298 to 0.552 | 0.289 to 0.615 | pass |

#### Memory

The family interval is the 95% interval across all 10 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10128.0 | 8624.0 | 0.852 | 0.847 to 0.856 | 0.843 to 0.856 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15472.0 | 13568.0 | 0.877 | 0.862 to 0.885 | 0.860 to 0.889 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3280.0 | 2896.0 | 0.890 | 0.874 to 0.899 | 0.853 to 0.901 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5921.0 | 5617.0 | 0.949 | 0.907 to 0.974 | 0.903 to 0.994 | pass |
| Daemon resident size, daemon a, KiB per counted op | 130 | 13968.0 | 12376.0 | 0.884 | 0.882 to 0.891 | 0.881 to 0.892 | pass |
| Daemon resident size, daemon b, KiB per counted op | 130 | 15600.0 | 13904.0 | 0.891 | 0.890 to 0.892 | 0.890 to 0.892 | pass |
| Daemon physical footprint, daemon a, KiB per counted op | 130 | 5425.0 | 5201.0 | 0.962 | 0.959 to 0.964 | 0.954 to 0.968 | pass |
| Daemon physical footprint, daemon b, KiB per counted op | 130 | 6177.0 | 6033.0 | 0.979 | 0.972 to 0.979 | 0.972 to 0.979 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5648.0 | 5152.0 | 0.917 | 0.912 to 0.922 | 0.907 to 0.924 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3321.0 | 3153.0 | 0.959 | 0.954 to 0.964 | 0.953 to 0.966 | pass |

#### Counters

Medians over the counted runs, anchor -> candidate. An op fails when the candidate starts more processes or makes more full or barrier flushes than the anchor.

| op | process starts | full and barrier flushes | verdict |
|---|---:|---:|---|
| acp new | 3 -> 1 | 8 -> 7 | pass |
| acp launch | 4 -> 2 | 15 -> 10 | pass |
| acp approve | 0 -> 0 | 3 -> 2 | pass |
| claude new | 3 -> 1 | 8 -> 7 | pass |
| claude launch | 4 -> 2 | 15 -> 13 | pass |
| claude approve | 0 -> 0 | 3 -> 2 | pass |
| codex new | 3 -> 1 | 8 -> 7 | pass |
| codex launch | 4 -> 2 | 17 -> 15 | pass |
| codex approve | 0 -> 0 | 3 -> 2 | pass |
| ledger append | 0 -> 0 | 3 -> 2 | pass |
| cli version | 0 -> 0 | 0 -> 0 | pass |
| cli ls | 0 -> 0 | 0 -> 0 | pass |
| acp suspend | 35 -> 34 | 39 -> 30 | pass |
| acp move | 42 -> 8 | 15 -> 11 | pass |
| acp resume | 36 -> 14 | 24 -> 22 | pass |
| acp pull | 115 -> 56 | 81 -> 65 | pass |
| claude suspend | 35 -> 34 | 41 -> 30 | pass |
| claude move | 42 -> 11 | 15 -> 11 | pass |
| claude resume | 38 -> 16 | 24 -> 22 | pass |
| claude pull | 118 -> 61 | 83 -> 65 | pass |
| codex suspend | 37 -> 36 | 40 -> 28 | pass |
| codex move | 42 -> 11 | 15 -> 11 | pass |
| codex resume | 39 -> 17 | 24 -> 22 | pass |
| codex pull | 121 -> 64 | 82 -> 63 | pass |
| cold start | 0 -> 0 | 8 -> 8 | pass |
| restart | 0 -> 0 | 44 -> 9 | pass |
