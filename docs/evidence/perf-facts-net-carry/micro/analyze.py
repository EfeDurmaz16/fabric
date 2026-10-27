import sys, random, statistics
rows = [l.split() for l in open(sys.argv[1])]
data = {}
for r in rows:
    pair, build = int(r[0]), r[1]
    vals = dict(zip(r[3::2], map(float, r[4::2])))
    data.setdefault(pair, {})[build] = vals
metrics = sys.argv[2].split(",")
random.seed(7)
for m in metrics:
    ratios = [d["head"][m] / d["base"][m] for d in data.values() if "head" in d and "base" in d]
    med = statistics.median(ratios)
    boots = sorted(statistics.median(random.choices(ratios, k=len(ratios))) for _ in range(10000))
    lo, hi = boots[249], boots[9749]
    base = statistics.median(d["base"][m] for d in data.values())
    head = statistics.median(d["head"][m] for d in data.values())
    print(f"{m}: pairs {len(ratios)} base {base*1000:.1f} ms head {head*1000:.1f} ms ratio {med:.3f} [{lo:.3f}-{hi:.3f}]")
