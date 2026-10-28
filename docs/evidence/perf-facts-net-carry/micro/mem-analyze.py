import sys, random, statistics, collections
# lines: <pair> <side> <bench> BENCH all <s> before <b> after <b> peak <b>
data = collections.defaultdict(dict)
for line in open(sys.argv[1]):
    p = line.split()
    if len(p) < 5 or p[3] != 'BENCH':
        continue
    vals = dict(zip(p[4::2], map(float, p[5::2])))
    vals['grown'] = vals['after'] - vals['before']
    data[(p[2], int(p[0]))][p[1]] = vals
random.seed(7)
for bench in sorted({b for b, _ in data}):
    for m in ('all', 'after', 'peak', 'grown'):
        pairs = [d for (b, _), d in data.items() if b == bench and len(d) == 2]
        ratios = [d['head'][m] / d['base'][m] for d in pairs]
        med = statistics.median(ratios)
        boots = sorted(statistics.median(random.choices(ratios, k=len(ratios))) for _ in range(10000))
        base = statistics.median(d['base'][m] for d in pairs)
        head = statistics.median(d['head'][m] for d in pairs)
        unit = 's' if m == 'all' else 'KiB'
        scale = 1 if m == 'all' else 1 / 1024
        print(f"{bench} {m}: pairs {len(ratios)} base {base*scale:.3f} {unit} head {head*scale:.3f} {unit} ratio {med:.3f} [{boots[249]:.3f}-{boots[9749]:.3f}]")
