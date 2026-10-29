import sys, random, statistics, collections
# pooled ratio per op kind across shapes: timed phase, metric wall time
path = sys.argv[1]
rows = [l.rstrip('\n').split('\t') for l in open(path)][1:]
metrics = collections.Counter(r[5] for r in rows if r[0] == 'timed')
metric = [m for m in metrics][0] if len(metrics) == 1 else None
t = collections.defaultdict(dict)
for phase, run, op, side, first, m, v in rows:
    if phase != 'timed': continue
    t[(op, run)][side] = float(v)
kinds = collections.defaultdict(list)
for (op, run), d in t.items():
    if 'anchor' in d and 'candidate' in d:
        kind = op.split(' ', 1)[1] if op.split(' ')[0] in ('acp', 'claude', 'codex') else op
        kinds[kind].append(d['candidate'] / d['anchor'])
random.seed(7)
print('metrics', dict(metrics))
for kind, ratios in sorted(kinds.items()):
    med = statistics.median(ratios)
    boots = sorted(statistics.median(random.choices(ratios, k=len(ratios))) for _ in range(10000))
    print(f"{kind}: pairs {len(ratios)} ratio {med:.3f} [{boots[249]:.3f}-{boots[9749]:.3f}]")
