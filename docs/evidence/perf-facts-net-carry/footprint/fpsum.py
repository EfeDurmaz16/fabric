import sys, collections, statistics
for path in sys.argv[1:]:
    rows = [l.rstrip('\n').split('\t') for l in open(path)][1:]
    timed = collections.defaultdict(dict)
    counted = collections.defaultdict(dict)
    for phase, run, op, side, first, m, v in rows:
        if phase == 'timed' and op in ('daemon a', 'daemon b'):
            timed[(op, run)][side] = int(v)
        if phase == 'counted' and m in ('daemon_rss_kib.a', 'daemon_rss_kib.b'):
            counted[(m[-1], run, op)][side] = int(v)
    print(path.split('/')[-2])
    for d in ('daemon a', 'daemon b'):
        diffs = [v['candidate'] - v['anchor'] for (o, r), v in sorted(timed.items(), key=lambda x: int(x[0][1])) if o == d and len(v) == 2]
        ratios = [v['candidate'] / v['anchor'] for (o, r), v in timed.items() if o == d and len(v) == 2]
        print(f"  timed {d}: diffs KiB {diffs} median ratio {statistics.median(ratios):.4f}")
    for n in ('a', 'b'):
        diffs = [v['candidate'] - v['anchor'] for (nn, r, o), v in counted.items() if nn == n and len(v) == 2]
        print(f"  counted daemon {n}: median diff KiB {statistics.median(diffs)} over {len(diffs)} ops")
