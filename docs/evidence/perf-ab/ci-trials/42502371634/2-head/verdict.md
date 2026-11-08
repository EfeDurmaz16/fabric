### Perf gate: pass

All 62 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 261.3 | 56.4 | 0.203 | 0.175 to 0.258 | 0.172 to 0.285 | pass |
| acp launch | 15 | 32.2 | 23.1 | 0.684 | 0.642 to 0.739 | 0.618 to 0.811 | pass |
| acp approve | 15 | 7.7 | 6.5 | 0.872 | 0.759 to 1.052 | 0.755 to 1.091 | pass |
| claude new | 15 | 206.6 | 45.1 | 0.245 | 0.178 to 0.340 | 0.161 to 0.343 | pass |
| claude launch | 15 | 215.5 | 173.8 | 0.900 | 0.682 to 1.185 | 0.667 to 1.299 | pass |
| claude approve | 15 | 10.2 | 7.7 | 0.784 | 0.689 to 0.877 | 0.615 to 1.313 | pass |
| codex new | 15 | 250.9 | 47.7 | 0.199 | 0.164 to 0.308 | 0.156 to 0.392 | pass |
| codex launch | 15 | 182.1 | 204.3 | 1.109 | 0.804 to 1.380 | 0.765 to 1.541 | pass |
| codex approve | 15 | 8.3 | 7.6 | 0.847 | 0.740 to 1.005 | 0.712 to 1.057 | pass |
| ledger append | 15 | 8.3 | 6.9 | 0.885 | 0.811 to 0.913 | 0.758 to 0.949 | pass |
| cli version | 15 | 3.3 | 3.4 | 0.962 | 0.888 to 1.042 | 0.869 to 1.100 | pass |
| cli ls | 15 | 4.2 | 4.2 | 0.957 | 0.944 to 0.975 | 0.920 to 1.029 | pass |
| acp suspend | 15 | 463.0 | 437.9 | 0.966 | 0.850 to 1.052 | 0.733 to 1.130 | pass |
| acp move | 15 | 311.2 | 225.6 | 0.755 | 0.633 to 0.820 | 0.576 to 0.869 | pass |
| acp resume | 15 | 978.7 | 341.4 | 0.357 | 0.300 to 0.455 | 0.293 to 0.543 | pass |
| acp pull | 15 | 1766.0 | 983.0 | 0.590 | 0.551 to 0.665 | 0.539 to 0.676 | pass |
| claude suspend | 15 | 612.0 | 463.1 | 0.730 | 0.662 to 0.775 | 0.626 to 0.828 | pass |
| claude move | 15 | 316.4 | 251.6 | 0.773 | 0.641 to 0.870 | 0.555 to 0.908 | pass |
| claude resume | 15 | 1362.7 | 713.3 | 0.488 | 0.453 to 0.597 | 0.425 to 0.673 | pass |
| claude pull | 15 | 2274.0 | 1310.4 | 0.534 | 0.519 to 0.609 | 0.512 to 0.680 | pass |
| codex suspend | 15 | 692.1 | 534.3 | 0.708 | 0.692 to 0.778 | 0.612 to 0.814 | pass |
| codex move | 15 | 317.2 | 257.7 | 0.826 | 0.745 to 0.932 | 0.620 to 0.967 | pass |
| codex resume | 15 | 1315.5 | 627.1 | 0.475 | 0.389 to 0.552 | 0.388 to 0.596 | pass |
| codex pull | 15 | 2286.4 | 1427.2 | 0.628 | 0.577 to 0.667 | 0.539 to 0.669 | pass |
| cold start | 15 | 52.7 | 81.0 | 1.392 | 0.592 to 2.399 | 0.554 to 3.127 | pass |
| restart | 15 | 165.7 | 82.8 | 0.523 | 0.298 to 0.899 | 0.271 to 0.981 | pass |

#### Memory

The family interval is the 95% interval across all 10 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10048.0 | 8592.0 | 0.852 | 0.850 to 0.860 | 0.847 to 0.861 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15376.0 | 13424.0 | 0.877 | 0.875 to 0.885 | 0.875 to 0.885 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3217.0 | 2880.0 | 0.894 | 0.888 to 0.929 | 0.882 to 0.932 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5809.0 | 5457.0 | 0.951 | 0.946 to 0.971 | 0.939 to 0.981 | pass |
| Daemon resident size, daemon a, KiB per counted op | 130 | 13816.0 | 12248.0 | 0.887 | 0.885 to 0.891 | 0.884 to 0.893 | pass |
| Daemon resident size, daemon b, KiB per counted op | 130 | 15600.0 | 13920.0 | 0.894 | 0.892 to 0.894 | 0.892 to 0.894 | pass |
| Daemon physical footprint, daemon a, KiB per counted op | 130 | 5233.0 | 5097.0 | 0.971 | 0.968 to 0.973 | 0.968 to 0.974 | pass |
| Daemon physical footprint, daemon b, KiB per counted op | 130 | 6161.0 | 6065.0 | 0.987 | 0.982 to 0.990 | 0.982 to 0.990 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5696.0 | 5176.0 | 0.913 | 0.907 to 0.917 | 0.905 to 0.919 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3345.0 | 3161.0 | 0.956 | 0.951 to 0.960 | 0.949 to 0.964 | pass |

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
