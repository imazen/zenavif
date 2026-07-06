#!/usr/bin/env python3
"""Pattern-2 separability scan for the monotonicity program.

Pattern 2 = the s6+ bundle HURTS: a slower s6/s7/s8 tier is Pareto-dominated by
the faster s4 (@q80). Labels each image bundle-hurts vs not (from labels.tsv),
joins the FULL zenanalyze feature set, and for every feature reports how cleanly
a single threshold separates the two classes (perfect-split count). With only ~24
origins this is a DIAGNOSTIC, not a fit — a feature that perfectly separates 6
from 18 here still needs a dense held-out sweep before it can gate. No clean
single-feature split ⇒ the dense multi-feature fit is required (chunk 5).

Usage: scan_pattern2_features.py LABELS_TSV FEATURES_TSV
"""
import sys
from collections import defaultdict

TIME_MARGIN, BYTES_MARGIN, SSIM2_MARGIN = 0.80, 0.01, 0.20


def hdr_line(f):
    for ln in f:
        s = ln.rstrip("\n")
        if s and not s.startswith("#"):
            return s
    return ""


def main():
    labels_path, feats_path = sys.argv[1], sys.argv[2]
    # load q80 tiers per image
    rows = defaultdict(dict)
    with open(labels_path) as f:
        h = hdr_line(f).split("\t")
        ix = {c: i for i, c in enumerate(h)}
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) < len(h) or p[ix["q"]] != "80" or p[ix["ssim2"]] == "NA":
                continue
            rows[p[ix["img"]]][int(p[ix["speed"]])] = (
                int(p[ix["bytes"]]), float(p[ix["ssim2"]]), float(p[ix["enc_ms"]]))

    def hurts(t):  # s6/7/8 dominated by faster s4?
        if 4 not in t:
            return False
        ab, aq, am = t[4]
        for s in (6, 7, 8):
            if s not in t:
                continue
            bb, bq, bm = t[s]
            if am < TIME_MARGIN * bm and ab <= bb and aq >= bq and (
                    (bb - ab) > BYTES_MARGIN * bb or (aq - bq) > SSIM2_MARGIN):
                return True
        return False

    label = {img: hurts(t) for img, t in rows.items()}
    hurt = {img for img, v in label.items() if v}
    print(f"bundle-hurts: {len(hurt)}/{len(label)}  {sorted(i.split('_')[0] for i in hurt)}")

    # load full features
    feats, fnames = {}, []
    with open(feats_path) as f:
        h = hdr_line(f).split("\t")
        vn = h.index("variant_name") if "variant_name" in h else 0
        fcols = [(i, c) for i, c in enumerate(h) if c.startswith("feat_")]
        fnames = [c for _, c in fcols]
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) <= vn:
                continue
            feats[p[vn]] = {c: (float(p[i]) if p[i] not in ("", "NA") else float("nan"))
                            for i, c in fcols}

    # for each feature, best single-threshold separation (minimize misclassified)
    scored = []
    for c in fnames:
        pos = [feats[img[:-4]][c] for img in hurt if img[:-4] in feats]
        neg = [feats[img[:-4]][c] for img in label if not label[img] and img[:-4] in feats]
        pos = [v for v in pos if v == v]
        neg = [v for v in neg if v == v]
        if len(pos) < 3 or len(neg) < 3:
            continue
        vals = sorted(set(pos + neg))
        best_err, best_t, best_dir = 1e9, None, None
        for i in range(len(vals) - 1):
            t = (vals[i] + vals[i + 1]) / 2
            # dir '<' : hurts if feature < t
            err_lt = sum(v >= t for v in pos) + sum(v < t for v in neg)
            err_gt = sum(v < t for v in pos) + sum(v >= t for v in neg)
            if err_lt < best_err:
                best_err, best_t, best_dir = err_lt, t, "<"
            if err_gt < best_err:
                best_err, best_t, best_dir = err_gt, t, ">"
        scored.append((best_err, c, best_dir, best_t, min(pos), max(pos), min(neg), max(neg)))

    scored.sort()
    print(f"\ntop single-feature separators (misclassified of {len(hurt)}+{len(label)-len(hurt)}):")
    for err, c, d, t, pmn, pmx, nmn, nmx in scored[:12]:
        print(f"  err={err:2d}  {c:<38} hurts if {d}{t:.3f}  (hurt {pmn:.2f}..{pmx:.2f} / rest {nmn:.2f}..{nmx:.2f})")


if __name__ == "__main__":
    main()
