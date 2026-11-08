### Perf gate: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 247.8 | 71.1 | 0.291 | 0.234 to 0.348 | 0.228 to 0.430 | pass |
| acp launch | 15 | 43.2 | 31.5 | 0.674 | 0.644 to 0.757 | 0.613 to 0.763 | pass |
| acp approve | 15 | 9.4 | 8.6 | 0.991 | 0.885 to 1.150 | 0.841 to 1.156 | pass |
| claude new | 15 | 242.5 | 65.2 | 0.306 | 0.242 to 0.325 | 0.229 to 0.370 | pass |
| claude launch | 15 | 184.8 | 205.3 | 1.023 | 0.833 to 1.182 | 0.638 to 1.557 | pass |
| claude approve | 15 | 11.1 | 8.0 | 0.710 | 0.611 to 0.917 | 0.583 to 1.025 | pass |
| codex new | 15 | 236.8 | 63.8 | 0.269 | 0.241 to 0.336 | 0.231 to 0.416 | pass |
| codex launch | 15 | 210.3 | 220.1 | 0.954 | 0.831 to 1.330 | 0.809 to 1.569 | pass |
| codex approve | 15 | 9.1 | 7.2 | 0.791 | 0.728 to 0.821 | 0.687 to 0.858 | pass |
| ledger append | 15 | 10.0 | 7.8 | 0.826 | 0.746 to 0.908 | 0.715 to 1.099 | pass |
| cli version | 15 | 4.0 | 3.7 | 0.950 | 0.858 to 1.045 | 0.767 to 1.211 | pass |
| cli ls | 15 | 4.7 | 4.5 | 0.943 | 0.859 to 0.988 | 0.762 to 1.094 | pass |
| acp suspend | 15 | 536.3 | 516.6 | 0.980 | 0.820 to 1.119 | 0.731 to 1.131 | pass |
| acp move | 15 | 357.7 | 241.8 | 0.704 | 0.639 to 0.775 | 0.623 to 0.778 | pass |
| acp resume | 15 | 1074.9 | 403.9 | 0.380 | 0.299 to 0.445 | 0.297 to 0.463 | pass |
| acp pull | 15 | 2158.8 | 1207.1 | 0.538 | 0.508 to 0.664 | 0.496 to 0.712 | pass |
| claude suspend | 15 | 707.9 | 551.6 | 0.713 | 0.659 to 0.754 | 0.631 to 0.838 | pass |
| claude move | 15 | 373.3 | 267.5 | 0.704 | 0.597 to 0.731 | 0.566 to 0.739 | pass |
| claude resume | 15 | 1515.5 | 741.4 | 0.457 | 0.429 to 0.511 | 0.392 to 0.668 | pass |
| claude pull | 15 | 2725.4 | 1539.2 | 0.579 | 0.516 to 0.675 | 0.509 to 0.693 | pass |
| codex suspend | 15 | 810.0 | 623.6 | 0.719 | 0.675 to 0.805 | 0.666 to 0.873 | pass |
| codex move | 15 | 367.1 | 247.4 | 0.653 | 0.621 to 0.706 | 0.603 to 0.755 | pass |
| codex resume | 15 | 1519.4 | 739.3 | 0.485 | 0.406 to 0.524 | 0.403 to 0.593 | pass |
| codex pull | 15 | 2770.1 | 1549.4 | 0.542 | 0.510 to 0.675 | 0.469 to 0.699 | pass |
| cold start | 15 | 104.8 | 80.1 | 0.887 | 0.605 to 1.044 | 0.541 to 1.056 | pass |
| restart | 15 | 175.7 | 99.0 | 0.587 | 0.324 to 0.745 | 0.301 to 0.800 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 8592.0 | 0.855 | 0.852 to 0.858 | 0.852 to 0.858 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15472.0 | 13520.0 | 0.869 | 0.865 to 0.877 | 0.862 to 0.879 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3216.0 | 2913.0 | 0.910 | 0.892 to 0.921 | 0.890 to 0.930 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5921.0 | 5520.0 | 0.924 | 0.903 to 0.945 | 0.896 to 0.945 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5632.0 | 5176.0 | 0.915 | 0.909 to 0.920 | 0.908 to 0.922 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3153.0 | 0.959 | 0.955 to 0.964 | 0.954 to 0.965 | pass |

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
