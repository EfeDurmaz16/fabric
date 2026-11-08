### New paths, candidate alone

No reference build has these paths, so there is no ratio. Medians in ms with the 95% bootstrap interval of the median.

| op | runs | median | 95% interval | min | max |
|---|---:|---:|---|---:|---:|
| facade submit | 20 | 7.7 | 7.5 to 8.1 | 6.5 | 9.5 |
| placement decision | 20 | 78.1 | 76.0 to 78.6 | 66.6 | 83.3 |
| watch delivery | 20 | 12.9 | 12.4 to 13.4 | 8.1 | 15.8 |
| facade continue | 20 | 14.8 | 13.8 to 15.0 | 9.5 | 38.5 |

Eligible candidates of each placement decision, of the two nodes, by run: 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2

Daemon sizes in KiB, median over the runs:

- daemon a rss_kib: 12160
- daemon a footprint_kib: 4912
- daemon b rss_kib: 10272
- daemon b footprint_kib: 4048

### Counted run

Every `fabric` process over one whole run of the four ops.

| counter | value |
|---|---:|
| cli_cpu_us | 69920 |
| cli_maxrss_kib | 4816 |
| cli_processes | 10 |
| cpu_us | 79493 |
| frees | 16133 |
| fullfsync | 5 |
| malloc_bytes | 5169169 |
| mallocs | 18363 |
| opens | 3 |
| read_bytes.ipc | 80 |
| read_bytes.state | 61 |
| read_calls.ipc | 8 |
| read_calls.state | 2 |
| spawn:claude | 2 |
| spawn:codex | 2 |
| spawns | 4 |
| syncs.journal | 5 |
| write_bytes.ipc | 5612 |
| write_bytes.journal | 177160 |
| write_bytes.state | 163 |
| write_calls.ipc | 15 |
| write_calls.journal | 86 |
| write_calls.state | 1 |
| daemon_rss_kib.a | 12144 |
| daemon_footprint_kib.a | 4976 |
| daemon_rss_kib.b | 10384 |
| daemon_footprint_kib.b | 4256 |
