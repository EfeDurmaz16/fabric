### New paths, candidate alone

No reference build has these paths, so there is no ratio. Medians in ms with the 95% bootstrap interval of the median.

| op | runs | median | 95% interval | min | max |
|---|---:|---:|---|---:|---:|
| facade submit | 20 | 8.2 | 7.0 to 10.4 | 6.2 | 52.7 |
| placement decision | 20 | 76.9 | 74.6 to 79.5 | 69.1 | 129.7 |
| watch delivery | 20 | 12.3 | 10.2 to 15.6 | 7.2 | 48.9 |
| facade continue | 20 | 13.5 | 11.1 to 17.6 | 9.1 | 72.9 |

Eligible candidates of each placement decision, of the two nodes, by run: 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2 2

Daemon sizes in KiB, median over the runs:

- daemon a rss_kib: 11264
- daemon a footprint_kib: 5008
- daemon b rss_kib: 9104
- daemon b footprint_kib: 4112

### Counted run

Every `fabric` process over one whole run of the four ops.

| counter | value |
|---|---:|
| cli_cpu_us | 56108 |
| cli_maxrss_kib | 4800 |
| cli_processes | 10 |
| cpu_us | 64623 |
| frees | 16140 |
| fullfsync | 5 |
| malloc_bytes | 5164652 |
| mallocs | 18318 |
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
| daemon_rss_kib.a | 11072 |
| daemon_footprint_kib.a | 5040 |
| daemon_rss_kib.b | 8848 |
| daemon_footprint_kib.b | 4256 |
