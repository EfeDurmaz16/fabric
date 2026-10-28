"""Median of the pairs' candidate/anchor ratios with a 95% percentile
bootstrap interval (10,000 resamples, fixed seed), per op."""
import csv, random, statistics, sys
from collections import defaultdict
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
pairs = defaultdict(dict)
for r in rows:
    pairs[(r["op"], int(r["pair"]))][r["side"]] = int(r["us"])
byop = defaultdict(list)
for (op, _), sides in sorted(pairs.items()):
    byop[op].append((sides["anchor"], sides["candidate"]))
rng = random.Random(20261027)
print("| op | pairs | anchor ms | candidate ms | ratio | 95% interval |")
print("|---|---:|---:|---:|---:|---|")
for op, ps in byop.items():
    ratios = [c / a for a, c in ps]
    boot = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(10000))
    lo, hi = boot[249], boot[9749]
    a = statistics.median(p[0] for p in ps) / 1000
    c = statistics.median(p[1] for p in ps) / 1000
    print(f"| {op} | {len(ps)} | {a:.2f} | {c:.2f} | {statistics.median(ratios):.3f} | {lo:.3f} to {hi:.3f} |")
