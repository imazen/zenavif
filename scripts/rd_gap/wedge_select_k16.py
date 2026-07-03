#!/usr/bin/env python3
"""WEDGE-FINDER origin selector: K=16 content-diverse TRAIN-split origins from
imazen-26 via k-means on zenanalyze features (centroid-nearest per cluster).

Same machinery as the train26 K=24 selection (adapted from
zenmetrics/scripts/sweep/knobablation_firstcut_select.py; canonical LSD
origin_split rule {0,2,4,6,8}=train via origin_split.py): k-means over the
full+native rows of the canonical imazen-26 features parquet, standardized,
random_state=0, centroid-nearest member per cluster. Reports overlap with the
train26 K=24 picks (coincident origins mean fewer fresh materializations and
direct continuity anchors against the qmdist-2026-07-03 sweeps).

Output: picks JSON at <outdir>/picks_k16.json (default the wedge corpus dir).
"""
import sys, json, os

sys.path.insert(0, "/home/lilith/work/zen/zenmetrics/scripts/picker")
import origin_split
import numpy as np
import pyarrow.parquet as pq
import pyarrow.compute as pc
from sklearn.preprocessing import StandardScaler
from sklearn.cluster import KMeans

K = 16
FEAT = "/mnt/v/output/imazen-26-features/imazen26_features_2026-06-23.parquet"
OUTDIR = sys.argv[1] if len(sys.argv) > 1 else "/mnt/v/output/rd-gap-wedge-2026-07-03"
TRAIN26_MANIFEST = "/mnt/v/output/rd-gap-train26-2026-07-02/_MANIFEST.json"

t = pq.read_table(FEAT)
names = t.schema.names
feat_cols = names[9:]  # after the 9 meta cols
m = pc.and_(pc.equal(t["crop_label"], "full"), pc.equal(t["size_class"], "native"))
ft = t.filter(m)
paths = ft["image_path"].to_pylist()
classes = ft["content_class"].to_pylist()
ws = ft["width"].to_pylist()
hs = ft["height"].to_pylist()
keep = [i for i, p in enumerate(paths) if origin_split.split_of(p) == "train"]
print(f"full+native rows: {ft.num_rows}; canonical-train origins: {len(keep)}", file=sys.stderr)

X = np.column_stack([np.asarray(ft[c].to_pylist(), dtype=float) for c in feat_cols])
X = X[keep]
paths = [paths[i] for i in keep]
classes = [classes[i] for i in keep]
mx = [max(ws[i], hs[i]) for i in keep]

colmed = np.nanmedian(X, axis=0)
good = ~np.isnan(colmed)
X = X[:, good]
fcols = [feat_cols[i] for i in range(len(feat_cols)) if good[i]]
colmed = colmed[good]
inds = np.where(np.isnan(X))
X[inds] = np.take(colmed, inds[1])
var = X.var(axis=0)
nz = var > 1e-12
X = X[:, nz]
fcols = [fcols[i] for i in range(len(fcols)) if nz[i]]
print(f"features used for kmeans: {X.shape[1]}", file=sys.stderr)

Xs = StandardScaler().fit_transform(X)
km = KMeans(n_clusters=K, random_state=0, n_init=10).fit(Xs)
picks = []
for c in range(K):
    members = np.where(km.labels_ == c)[0]
    d = np.linalg.norm(Xs[members] - km.cluster_centers_[c], axis=1)
    best = members[np.argmin(d)]
    picks.append(
        {
            "cluster": c,
            "cluster_size": int(len(members)),
            "image_path": paths[best],
            "content_class": classes[best],
            "native_longedge": int(mx[best]),
            "origin_id": origin_split.origin_id(paths[best]),
        }
    )

picks.sort(key=lambda r: r["cluster"])
os.makedirs(OUTDIR, exist_ok=True)
out = os.path.join(OUTDIR, "picks_k16.json")
with open(out, "w") as f:
    json.dump(
        {
            "k": K,
            "feature_parquet": FEAT,
            "n_kmeans_features": len(fcols),
            "selector": "adapted from zenmetrics/scripts/sweep/knobablation_firstcut_select.py (StandardScaler + KMeans random_state=0 n_init=10, centroid-nearest, TRAIN LSD origins)",
            "picks": picks,
        },
        f,
        indent=2,
    )
print(f"wrote {out}")

print(f'{"cl":>2} {"size":>5} {"longE":>6} {"class":38s} path')
for r in picks:
    print(
        f'{r["cluster"]:>2} {r["cluster_size"]:>5} {r["native_longedge"]:>6} '
        f'{r["content_class"]:38s} {r["image_path"].split("/")[-1][:64]}'
    )
print("\nn distinct classes among picks:", len(set(r["content_class"] for r in picks)))

# Overlap with the train26 K=24 picks (continuity anchors)
try:
    t26 = {p["image_path"] for p in json.load(open(TRAIN26_MANIFEST))["picks"]}
    ov = [r["image_path"].split("/")[-1] for r in picks if r["image_path"] in t26]
    print(f"\noverlap with train26 K=24: {len(ov)}/16")
    for o in ov:
        print(f"  {o[:80]}")
except Exception as e:  # noqa: BLE001
    print(f"train26 overlap check skipped: {e}", file=sys.stderr)
