### Perf gate: pass

All 62 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 267.2 | 63.6 | 0.287 | 0.237 to 0.321 | 0.197 to 0.342 | pass |
| acp launch | 15 | 32.4 | 21.6 | 0.600 | 0.585 to 0.692 | 0.577 to 0.697 | pass |
| acp approve | 15 | 8.3 | 6.9 | 0.829 | 0.814 to 0.878 | 0.798 to 0.899 | pass |
| claude new | 15 | 247.2 | 56.4 | 0.234 | 0.214 to 0.281 | 0.194 to 0.314 | pass |
| claude launch | 15 | 160.6 | 204.1 | 1.103 | 0.951 to 1.490 | 0.702 to 1.712 | pass |
| claude approve | 15 | 8.7 | 7.6 | 0.831 | 0.694 to 0.907 | 0.603 to 0.924 | pass |
| codex new | 15 | 201.4 | 56.3 | 0.274 | 0.232 to 0.311 | 0.221 to 0.410 | pass |
| codex launch | 15 | 165.8 | 174.4 | 1.113 | 0.843 to 1.289 | 0.609 to 1.917 | pass |
| codex approve | 15 | 8.0 | 6.7 | 0.856 | 0.720 to 0.922 | 0.618 to 1.014 | pass |
| ledger append | 15 | 7.6 | 6.2 | 0.815 | 0.753 to 0.842 | 0.620 to 0.871 | pass |
| cli version | 15 | 3.5 | 3.7 | 0.956 | 0.900 to 1.064 | 0.865 to 1.080 | pass |
| cli ls | 15 | 4.5 | 4.3 | 0.936 | 0.880 to 0.961 | 0.843 to 1.024 | pass |
| acp suspend | 15 | 502.9 | 485.6 | 0.939 | 0.826 to 0.986 | 0.803 to 1.024 | pass |
| acp move | 15 | 333.1 | 232.5 | 0.689 | 0.640 to 0.764 | 0.638 to 0.795 | pass |
| acp resume | 15 | 848.7 | 390.6 | 0.438 | 0.323 to 0.498 | 0.289 to 0.569 | pass |
| acp pull | 15 | 1762.6 | 1087.1 | 0.618 | 0.554 to 0.662 | 0.500 to 0.666 | pass |
| claude suspend | 15 | 681.6 | 482.9 | 0.778 | 0.603 to 0.878 | 0.601 to 0.886 | pass |
| claude move | 15 | 343.0 | 258.6 | 0.752 | 0.727 to 0.825 | 0.594 to 0.869 | pass |
| claude resume | 15 | 1374.1 | 695.9 | 0.476 | 0.448 to 0.579 | 0.416 to 0.584 | pass |
| claude pull | 15 | 2367.0 | 1386.4 | 0.600 | 0.536 to 0.636 | 0.535 to 0.642 | pass |
| codex suspend | 15 | 730.9 | 533.1 | 0.709 | 0.670 to 0.743 | 0.644 to 0.758 | pass |
| codex move | 15 | 324.5 | 261.3 | 0.758 | 0.714 to 0.875 | 0.687 to 0.906 | pass |
| codex resume | 15 | 1393.0 | 655.1 | 0.459 | 0.368 to 0.546 | 0.353 to 0.556 | pass |
| codex pull | 15 | 2387.4 | 1391.5 | 0.576 | 0.556 to 0.610 | 0.506 to 0.638 | pass |
| cold start | 15 | 70.6 | 81.1 | 1.198 | 0.838 to 1.465 | 0.804 to 1.543 | pass |
| restart | 15 | 154.5 | 74.3 | 0.493 | 0.406 to 0.600 | 0.288 to 0.753 | pass |

#### Memory

The family interval is the 95% interval across all 10 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 8640.0 | 0.857 | 0.847 to 0.861 | 0.845 to 0.863 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15472.0 | 13680.0 | 0.886 | 0.879 to 0.893 | 0.866 to 0.902 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3217.0 | 2944.0 | 0.915 | 0.881 to 0.938 | 0.866 to 0.947 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5921.0 | 5745.0 | 0.975 | 0.955 to 0.997 | 0.915 to 1.027 | pass |
| Daemon resident size, daemon a, KiB per counted op | 130 | 13976.0 | 12160.0 | 0.881 | 0.878 to 0.883 | 0.877 to 0.885 | pass |
| Daemon resident size, daemon b, KiB per counted op | 130 | 15792.0 | 13824.0 | 0.875 | 0.874 to 0.876 | 0.874 to 0.876 | pass |
| Daemon physical footprint, daemon a, KiB per counted op | 130 | 5393.0 | 5017.0 | 0.953 | 0.949 to 0.959 | 0.949 to 0.962 | pass |
| Daemon physical footprint, daemon b, KiB per counted op | 130 | 6337.0 | 5969.0 | 0.942 | 0.939 to 0.943 | 0.937 to 0.943 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5664.0 | 5200.0 | 0.915 | 0.909 to 0.919 | 0.908 to 0.920 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3353.0 | 3169.0 | 0.957 | 0.954 to 0.960 | 0.952 to 0.963 | pass |

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
