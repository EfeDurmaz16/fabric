### Perf gate: pass

The first run failed none, so CI ran both builds again; the confirmation failed none. A check fails only when it fails in both runs: none.

### First run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 207.2 | 56.9 | 0.293 | 0.258 to 0.375 | 0.206 to 0.414 | pass |
| acp launch | 15 | 35.2 | 22.3 | 0.682 | 0.603 to 0.706 | 0.593 to 0.707 | pass |
| acp approve | 15 | 7.7 | 6.6 | 0.838 | 0.775 to 0.883 | 0.773 to 0.945 | pass |
| claude new | 15 | 247.8 | 55.0 | 0.242 | 0.210 to 0.272 | 0.201 to 0.297 | pass |
| claude launch | 15 | 177.1 | 213.2 | 1.184 | 0.769 to 1.475 | 0.506 to 1.679 | pass |
| claude approve | 15 | 8.7 | 7.1 | 0.836 | 0.549 to 0.935 | 0.492 to 0.959 | pass |
| codex new | 15 | 236.7 | 53.3 | 0.245 | 0.212 to 0.407 | 0.208 to 0.435 | pass |
| codex launch | 15 | 243.3 | 253.5 | 1.036 | 0.711 to 1.403 | 0.633 to 1.769 | pass |
| codex approve | 15 | 8.9 | 7.0 | 0.906 | 0.704 to 1.019 | 0.693 to 1.078 | pass |
| ledger append | 15 | 8.0 | 6.9 | 0.850 | 0.818 to 0.875 | 0.799 to 0.880 | pass |
| cli version | 15 | 3.4 | 3.3 | 1.007 | 0.842 to 1.056 | 0.599 to 1.100 | pass |
| cli ls | 15 | 4.3 | 4.2 | 0.950 | 0.899 to 1.005 | 0.894 to 1.066 | pass |
| acp suspend | 15 | 490.4 | 470.0 | 0.939 | 0.905 to 0.997 | 0.899 to 1.022 | pass |
| acp move | 15 | 315.0 | 244.5 | 0.751 | 0.674 to 0.802 | 0.658 to 0.830 | pass |
| acp resume | 15 | 912.5 | 380.1 | 0.398 | 0.368 to 0.446 | 0.341 to 0.569 | pass |
| acp pull | 15 | 1747.2 | 1037.2 | 0.602 | 0.543 to 0.652 | 0.538 to 0.698 | pass |
| claude suspend | 15 | 698.1 | 480.5 | 0.721 | 0.638 to 0.753 | 0.638 to 0.776 | pass |
| claude move | 15 | 340.2 | 234.7 | 0.742 | 0.634 to 0.828 | 0.617 to 0.833 | pass |
| claude resume | 15 | 1307.7 | 642.9 | 0.460 | 0.427 to 0.561 | 0.395 to 0.572 | pass |
| claude pull | 15 | 2367.3 | 1391.0 | 0.569 | 0.552 to 0.607 | 0.510 to 0.634 | pass |
| codex suspend | 15 | 695.4 | 491.0 | 0.668 | 0.618 to 0.797 | 0.606 to 0.818 | pass |
| codex move | 15 | 327.9 | 223.2 | 0.641 | 0.574 to 0.767 | 0.547 to 0.849 | pass |
| codex resume | 15 | 1445.4 | 702.9 | 0.457 | 0.443 to 0.544 | 0.381 to 0.631 | pass |
| codex pull | 15 | 2448.6 | 1338.9 | 0.542 | 0.528 to 0.630 | 0.520 to 0.665 | pass |
| cold start | 15 | 112.0 | 65.5 | 0.609 | 0.506 to 0.756 | 0.417 to 0.881 | pass |
| restart | 15 | 180.4 | 94.2 | 0.488 | 0.381 to 0.756 | 0.220 to 0.766 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10096.0 | 8624.0 | 0.854 | 0.849 to 0.857 | 0.844 to 0.857 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15312.0 | 13712.0 | 0.901 | 0.894 to 0.911 | 0.892 to 0.911 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3232.0 | 2896.0 | 0.896 | 0.877 to 0.917 | 0.869 to 0.924 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5744.0 | 5761.0 | 1.020 | 1.003 to 1.044 | 0.985 to 1.044 | pass; its 95% interval alone is above 1 |
| CLI peak resident size, KiB per counted op | 130 | 5680.0 | 5208.0 | 0.919 | 0.914 to 0.925 | 0.911 to 0.926 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3297.0 | 3153.0 | 0.964 | 0.959 to 0.967 | 0.957 to 0.968 | pass |

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

### Confirmation run: pass

All 58 checks hold against the anchor.

#### Times

Medians in ms. The family interval is the 95% interval across all 26 ops at once (Bonferroni); an op fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| acp new | 15 | 180.4 | 44.6 | 0.255 | 0.224 to 0.310 | 0.174 to 0.322 | pass |
| acp launch | 15 | 23.5 | 15.3 | 0.661 | 0.640 to 0.672 | 0.633 to 0.684 | pass |
| acp approve | 15 | 6.0 | 4.7 | 0.845 | 0.787 to 0.870 | 0.762 to 0.879 | pass |
| claude new | 15 | 254.5 | 39.3 | 0.190 | 0.152 to 0.233 | 0.148 to 0.245 | pass |
| claude launch | 15 | 169.3 | 176.8 | 1.045 | 0.739 to 1.670 | 0.730 to 1.805 | pass |
| claude approve | 15 | 8.8 | 6.8 | 0.794 | 0.509 to 0.923 | 0.479 to 1.348 | pass |
| codex new | 15 | 212.0 | 40.7 | 0.202 | 0.151 to 0.256 | 0.145 to 0.276 | pass |
| codex launch | 15 | 235.4 | 213.3 | 0.975 | 0.551 to 1.249 | 0.548 to 1.518 | pass |
| codex approve | 15 | 7.6 | 5.3 | 0.794 | 0.553 to 0.970 | 0.522 to 0.985 | pass |
| ledger append | 15 | 5.6 | 4.9 | 0.858 | 0.835 to 0.895 | 0.829 to 0.909 | pass |
| cli version | 15 | 2.8 | 2.7 | 0.962 | 0.908 to 1.059 | 0.886 to 1.081 | pass |
| cli ls | 15 | 3.3 | 3.2 | 0.954 | 0.931 to 0.987 | 0.924 to 1.007 | pass |
| acp suspend | 15 | 453.0 | 431.3 | 0.969 | 0.828 to 1.005 | 0.822 to 1.137 | pass |
| acp move | 15 | 270.2 | 237.6 | 0.772 | 0.751 to 0.937 | 0.723 to 1.090 | pass |
| acp resume | 15 | 838.6 | 318.3 | 0.382 | 0.322 to 0.442 | 0.300 to 0.446 | pass |
| acp pull | 15 | 1537.5 | 987.2 | 0.623 | 0.561 to 0.669 | 0.557 to 0.700 | pass |
| claude suspend | 15 | 515.8 | 409.7 | 0.718 | 0.616 to 0.866 | 0.608 to 1.061 | pass |
| claude move | 15 | 279.6 | 234.8 | 0.841 | 0.770 to 0.947 | 0.692 to 1.016 | pass |
| claude resume | 15 | 1234.1 | 644.9 | 0.543 | 0.486 to 0.606 | 0.459 to 0.637 | pass |
| claude pull | 15 | 2262.3 | 1298.4 | 0.582 | 0.568 to 0.589 | 0.542 to 0.654 | pass |
| codex suspend | 15 | 639.6 | 456.7 | 0.700 | 0.634 to 0.747 | 0.574 to 0.774 | pass |
| codex move | 15 | 294.5 | 243.6 | 0.859 | 0.639 to 0.952 | 0.556 to 0.955 | pass |
| codex resume | 15 | 1323.2 | 601.0 | 0.441 | 0.405 to 0.489 | 0.368 to 0.599 | pass |
| codex pull | 15 | 2246.2 | 1337.9 | 0.575 | 0.568 to 0.641 | 0.559 to 0.667 | pass |
| cold start | 15 | 51.9 | 80.6 | 0.898 | 0.629 to 1.622 | 0.549 to 2.254 | pass |
| restart | 15 | 128.7 | 61.6 | 0.531 | 0.371 to 0.634 | 0.355 to 0.748 | pass |

#### Memory

The family interval is the 95% interval across all 6 rows at once; a row fails when it lies above 1.

| what | pairs | anchor | candidate | ratio | 95% interval | family interval | verdict |
|---|---:|---:|---:|---:|---|---|---|
| Daemon resident size, daemon a, KiB after each timed run | 15 | 10080.0 | 8624.0 | 0.856 | 0.850 to 0.858 | 0.849 to 0.859 | pass |
| Daemon resident size, daemon b, KiB after each timed run | 15 | 15408.0 | 13616.0 | 0.881 | 0.878 to 0.884 | 0.877 to 0.888 | pass |
| Daemon physical footprint, daemon a, KiB after each timed run | 15 | 3200.0 | 2912.0 | 0.900 | 0.888 to 0.920 | 0.877 to 0.924 | pass |
| Daemon physical footprint, daemon b, KiB after each timed run | 15 | 5824.0 | 5632.0 | 0.960 | 0.953 to 0.974 | 0.942 to 0.994 | pass |
| CLI peak resident size, KiB per counted op | 130 | 5672.0 | 5200.0 | 0.917 | 0.914 to 0.922 | 0.912 to 0.924 | pass |
| CLI peak physical footprint, KiB per counted op | 130 | 3329.0 | 3137.0 | 0.959 | 0.955 to 0.963 | 0.954 to 0.964 | pass |

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
