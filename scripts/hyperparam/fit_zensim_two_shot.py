#!/usr/bin/env python3
"""Two-shot zensim targeting: measure the achievable lattice, then pick and
fit the pass-2 placement rule against it.

The objective this script optimises is NOT "converge inside a tolerance
band".  It is:

    given EXACTLY TWO encodes, minimise |achieved - target|

Input is one or more dense achievable-lattice TSVs from

    zensim_loop_bench lattice <manifest> <speed> <sizes> <score_lo> <score_hi>

which measure the score at EVERY reachable AV1 quantizer index in the band
covering the requested score window.  Encoded output depends on `quality`
only through that integer quantizer, so this table is the complete set of
outcomes any targeting method can produce for that cell -- which makes the
replay below exact rather than interpolated: every pass-1 and pass-2 encode
a rule asks for is looked up, never modelled.

Usage:
    fit_zensim_two_shot.py --train T.tsv [T2.tsv ...] --val V.tsv [...] \
        [--targets 20:90:2.5] [--emit-rust]

Deterministic: no randomness anywhere, so the shipped constants are
reproducible by re-running it.
"""

from __future__ import annotations

import argparse
import bisect
import math
import statistics
import sys
from collections import defaultdict

# ---------------------------------------------------------------------------
# zenavif's quality <-> quantizer curve (mirror of src/encode_plan.rs)
# ---------------------------------------------------------------------------


def quality_to_quantizer_f(quality: float) -> float:
    """Continuous (unrounded) quantizer index for a quality."""
    q = min(max(quality, 1.0), 100.0) / 100.0
    if q >= 0.70:
        x = (1.0 - q) * 1.4
    elif q > 0.10:
        x = 0.42 + (0.70 - q) * 0.85
    else:
        x = 0.93 + (0.10 - q) * 0.78
    return min(x, 1.0) * 255.0


def quality_to_quantizer(quality: float) -> int:
    return int(round(quality_to_quantizer_f(quality)))


# The shipped score -> quality anchor (src/two_pass_zensim.rs).
ANCHOR_SCORE = [20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0,
                60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0]
ANCHOR_QUALITY = [24.546, 27.502, 30.540, 33.217, 35.999, 40.040, 42.495,
                  45.937, 50.294, 56.654, 61.060, 66.338, 74.021, 81.451,
                  92.628]


def _pwl(xs, ys, x):
    """Piecewise-linear interpolation with linear extrapolation, xs ascending."""
    n = len(xs)
    if x <= xs[0]:
        s = (ys[1] - ys[0]) / (xs[1] - xs[0])
        return ys[0] + (x - xs[0]) * s
    if x >= xs[-1]:
        s = (ys[-1] - ys[-2]) / (xs[-1] - xs[-2])
        return ys[-1] + (x - xs[-1]) * s
    i = bisect.bisect_right(xs, x) - 1
    i = min(max(i, 0), n - 2)
    f = (x - xs[i]) / (xs[i + 1] - xs[i])
    return ys[i] + f * (ys[i + 1] - ys[i])


def anchor_quality(t: float) -> float:
    return min(max(_pwl(ANCHOR_SCORE, ANCHOR_QUALITY, t), 1.0), 100.0)


# ---------------------------------------------------------------------------
# lattice loading
# ---------------------------------------------------------------------------


class Cell:
    """One (image, size) cell: the complete achievable-score lattice."""

    __slots__ = ("image", "size", "w", "h", "qis", "score", "dm", "bytes")

    def __init__(self, image, size, w, h):
        self.image, self.size, self.w, self.h = image, size, w, h
        self.qis: list[int] = []
        self.score: dict[int, float] = {}
        self.dm: dict[int, float] = {}
        self.bytes: dict[int, int] = {}

    def key(self):
        return (self.image, self.size)

    def finish(self):
        self.qis = sorted(self.score)

    def has(self, qi: int) -> bool:
        return qi in self.score

    def clamp(self, qi: float) -> int:
        return int(min(max(qi, self.qis[0]), self.qis[-1]))

    def best_error(self, target: float) -> float:
        """The precision CEILING: nearest achievable score to the target."""
        return min(abs(self.score[q] - target) for q in self.qis)

    def best_qi(self, target: float) -> int:
        return min(self.qis, key=lambda q: abs(self.score[q] - target))


def load_lattice(paths) -> dict:
    cells: dict[tuple, Cell] = {}
    for p in paths:
        with open(p) as f:
            head = f.readline().rstrip("\n").split("\t")
            idx = {c: i for i, c in enumerate(head)}
            for line in f:
                r = line.rstrip("\n").split("\t")
                if len(r) < len(head):
                    continue
                image, size = r[idx["image"]], int(r[idx["size"]])
                k = (image, size)
                c = cells.get(k)
                if c is None:
                    c = cells[k] = Cell(image, size, int(r[idx["w"]]), int(r[idx["h"]]))
                qi = int(r[idx["qindex"]])
                c.score[qi] = float(r[idx["zensim"]])
                c.dm[qi] = float(r[idx["dm_mean"]])
                c.bytes[qi] = int(r[idx["bytes"]])
    for c in cells.values():
        c.finish()
    return cells


# ---------------------------------------------------------------------------
# lattice geometry: how fine is the ceiling, really
# ---------------------------------------------------------------------------


def lattice_report(cells, out):
    print("# --- achievable-score lattice geometry ---", file=out)
    print("# Adjacent-quantizer score gaps. The quantizer lattice is the REAL", file=out)
    print("# one: quality only reaches 100 of the 256 quantizers, so an", file=out)
    print("# integer-quality sweep over-states the gap by ~2.6x.", file=out)
    print("group\tn_cells\tn_gaps\tmed_gap\tp90_gap\tfrac_gap>1.0\tmed_ceiling@nearest", file=out)

    def stats(subset, label):
        gaps, ceils = [], []
        for c in subset:
            for a, b in zip(c.qis, c.qis[1:]):
                if b == a + 1:
                    gaps.append(abs(c.score[a] - c.score[b]))
            # ceiling for a uniform grid of targets across the cell's range
            lo, hi = min(c.score.values()), max(c.score.values())
            for i in range(41):
                t = lo + (hi - lo) * i / 40.0
                ceils.append(c.best_error(t))
        if not gaps:
            return
        print(
            f"{label}\t{len(subset)}\t{len(gaps)}\t{statistics.median(gaps):.3f}\t"
            f"{pct(gaps, 90):.3f}\t{sum(g > 1.0 for g in gaps) / len(gaps):.1%}\t"
            f"{statistics.median(ceils):.4f}",
            file=out,
        )

    allc = list(cells.values())
    stats(allc, "ALL")
    for sz in sorted({c.size for c in allc}):
        stats([c for c in allc if c.size == sz], f"size={sz}")
    # by score band -- the gap is strongly score-dependent
    print("# gaps by score band (the lattice is coarsest at LOW scores):", file=out)
    print("band\tn_gaps\tmed_gap\tp90_gap\tfrac_gap>1.0\tfrac_gap>0.5", file=out)
    bands = [(10, 30), (30, 50), (50, 70), (70, 85), (85, 100)]
    for lo, hi in bands:
        g = []
        for c in allc:
            for a, b in zip(c.qis, c.qis[1:]):
                if b == a + 1 and lo <= c.score[a] < hi:
                    g.append(abs(c.score[a] - c.score[b]))
        if g:
            print(
                f"[{lo},{hi})\t{len(g)}\t{statistics.median(g):.3f}\t{pct(g, 90):.3f}\t"
                f"{sum(x > 1.0 for x in g) / len(g):.1%}\t{sum(x > 0.5 for x in g) / len(g):.1%}",
                file=out,
            )


def pct(v, p):
    if not v:
        return float("nan")
    s = sorted(v)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    return s[int(k)] if lo == hi else s[lo] + (s[hi] - s[lo]) * (k - lo)


# ---------------------------------------------------------------------------
# anchor refit, in quantizer space, from the lattice itself
# ---------------------------------------------------------------------------


def fit_quantizer_anchor(cells):
    """Median over cells of the quantizer that first reaches each knot score.

    Same convention as the shipped quality-space anchor (leftmost quality
    reaching the score == the coarsest quantizer reaching it), so the two
    tables describe the same population curve.
    """
    knots = []
    for s in ANCHOR_SCORE:
        per_cell = []
        for c in cells:
            # coarsest (highest) quantizer whose score still reaches s
            reach = [q for q in c.qis if c.score[q] >= s]
            if not reach or max(reach) == c.qis[-1] and c.score[c.qis[-1]] >= s:
                # unreachable at the coarse end -> the cell never drops below s
                pass
            if reach:
                per_cell.append(max(reach))
        if per_cell:
            knots.append(statistics.median(per_cell))
        else:
            knots.append(float("nan"))
    # enforce strict monotone decrease (score up => quantizer down)
    for i in range(1, len(knots)):
        if not math.isnan(knots[i]) and not math.isnan(knots[i - 1]):
            knots[i] = min(knots[i], knots[i - 1] - 0.001)
    return knots


def make_anchor_fns(qknots):
    def a_qi(t):  # score -> quantizer
        return min(max(_pwl(ANCHOR_SCORE, qknots, t), 0.0), 255.0)

    # inverse: quantizer -> score (qknots descend, so reverse for _pwl)
    rq = list(reversed(qknots))
    rs = list(reversed(ANCHOR_SCORE))

    def a_score(qi):
        return _pwl(rq, rs, qi)

    return a_qi, a_score


# derived-from-quality-knots default (what ships before any refit)
DERIVED_QKNOTS = [quality_to_quantizer_f(q) for q in ANCHOR_QUALITY]


# ---------------------------------------------------------------------------
# the rules
# ---------------------------------------------------------------------------


def seed_qi(cell, target, ctx=None):
    """Pass-1 quantizer.

    Matches the shipped `encode_rgb8_zensim_two_shot`, which seeds from the
    QUANTIZER anchor (`anchor_quantizer_for_zensim`), not from the quality
    anchor. Replaying a different seed than the code uses would make every
    downstream number describe a rule nobody ships.
    """
    if ctx is None:
        return quality_to_quantizer(anchor_quality(target))
    return int(round(ctx["a_qi"](target)))


def rule_quality_translate(cell, target, qi1, s1, ctx):
    """The existing loop's global step: translate in QUALITY space."""
    q1 = ctx["qual_of"][qi1]
    shift = anchor_quality(target) - anchor_quality(s1)
    if abs(shift) < 1.0:
        shift = 4.0 if target > s1 else -4.0
    return float(quality_to_quantizer(min(max(q1 + shift, 1.0), 100.0)))


def rule_qi_translate(cell, target, qi1, s1, ctx):
    """Translate in QUANTIZER space along the population curve."""
    a = ctx["a_qi"]
    return qi1 + (a(target) - a(s1))


def rule_qi_ratio(cell, target, qi1, s1, ctx):
    """Multiplicative translate (a translate in log-quantizer space)."""
    a = ctx["a_qi"]
    d = a(s1)
    if d <= 1e-9:
        return qi1 + (a(target) - a(s1))
    return qi1 * a(target) / d


def rule_qi_translate_gain(cell, target, qi1, s1, ctx):
    a, g = ctx["a_qi"], ctx["gain"]
    return qi1 + g * (a(target) - a(s1))


def rule_dm_power(cell, target, qi1, s1, ctx):
    """Predict via the diffmap power law: dm ~ qi^gamma, score = G(dm)."""
    dm1 = cell.dm[qi1]
    g_inv, gamma = ctx["g_inv"], ctx["gamma"]
    want = g_inv(target)
    if dm1 <= 0 or want <= 0:
        return rule_qi_translate(cell, target, qi1, s1, ctx)
    return qi1 * (want / dm1) ** (1.0 / gamma)


def lm_features(target, qi1, s1, dm1, a_qi):
    """Features for the fitted residual model.

    The translate rule predicts `qi1 + step`.  It is exact when this
    image's score-vs-quantizer curve is a pure horizontal translate of the
    population's; the residual is whatever SHAPE difference is left, so the
    features are the two things that can carry shape information out of a
    single measurement:

      step  -- how far pass 2 has to move (a slope error scales with it)
      off   -- how far this image sits from the population at pass 1
               (a proxy for "harder/easier than typical"), and its
               interaction with step, which is what a slope error IS.
    """
    step = a_qi(target) - a_qi(s1)
    off = qi1 - a_qi(s1)
    return [1.0, step, off, step * off, abs(step)]


def fit_lm(cells, targets, a_qi):
    """OLS of the oracle quantizer's residual on lm_features (TRAIN only)."""
    rows, ys = [], []
    for c in cells:
        for t in targets:
            qi1 = seed_qi(c, t, {"a_qi": a_qi})
            if not c.has(qi1):
                continue
            s1 = c.score[qi1]
            x = lm_features(t, qi1, s1, c.dm[qi1], a_qi)
            # target: the residual the plain translate leaves
            rows.append(x)
            ys.append(c.best_qi(t) - (qi1 + x[1]))
    return ols(rows, ys)


def ols(rows, ys):
    """Normal-equation least squares, no numpy. Returns coefficient list."""
    k = len(rows[0])
    ata = [[0.0] * k for _ in range(k)]
    atb = [0.0] * k
    for x, y in zip(rows, ys):
        for i in range(k):
            atb[i] += x[i] * y
            for j in range(k):
                ata[i][j] += x[i] * x[j]
    # tiny ridge so a degenerate column cannot blow the solve up
    for i in range(k):
        ata[i][i] += 1e-6
    # Gaussian elimination with partial pivoting
    m = [row[:] + [atb[i]] for i, row in enumerate(ata)]
    for col in range(k):
        p = max(range(col, k), key=lambda r: abs(m[r][col]))
        m[col], m[p] = m[p], m[col]
        if abs(m[col][col]) < 1e-12:
            continue
        for r in range(k):
            if r == col:
                continue
            f = m[r][col] / m[col][col]
            for cc in range(col, k + 1):
                m[r][cc] -= f * m[col][cc]
    return [m[i][k] / m[i][i] if abs(m[i][i]) > 1e-12 else 0.0 for i in range(k)]


def rule_qi_ratio_hi(cell, target, qi1, s1, ctx):
    """Stretch anchored at the COARSE end of the quantizer range.

    `qi_ratio` anchors the one-parameter stretch at quantizer 0 (the
    near-lossless end, where every image's score converges near 100);
    this one anchors it at 255 instead.  Whichever anchor is closer to
    where the image's curve is actually pinned wins, so both are worth
    measuring rather than assumed.
    """
    a = ctx["a_qi"]
    den = 255.0 - a(s1)
    if den <= 1e-9:
        return rule_qi_translate(cell, target, qi1, s1, ctx)
    return 255.0 - (255.0 - qi1) * (255.0 - a(target)) / den


def rule_blend(cell, target, qi1, s1, ctx):
    """Mean of the translate and the two stretches."""
    return (
        rule_qi_translate(cell, target, qi1, s1, ctx)
        + rule_qi_ratio(cell, target, qi1, s1, ctx)
        + rule_qi_ratio_hi(cell, target, qi1, s1, ctx)
    ) / 3.0


def rule_qi_translate_lm(cell, target, qi1, s1, ctx):
    a, coef = ctx["a_qi"], ctx["lm"]
    x = lm_features(target, qi1, s1, cell.dm[qi1], a)
    return qi1 + x[1] + sum(c * v for c, v in zip(coef, x))


RULES = {
    "quality_translate": rule_quality_translate,
    "qi_translate": rule_qi_translate,
    "qi_ratio": rule_qi_ratio,
    "qi_translate_gain": rule_qi_translate_gain,
    "dm_power": rule_dm_power,
    "qi_translate_lm": rule_qi_translate_lm,
    "qi_ratio_hi": rule_qi_ratio_hi,
    "blend": rule_blend,
}


def translate_family_ceiling(cells, targets, a_qi, out):
    """How good can ANY pure-translate rule be, with a perfect offset?

    For each cell, choose the single quantizer offset that minimises the
    median |score error| the translate rule would leave across all
    targets, and report what is left.  That residual is the *shape*
    difference between the image's score-vs-quantizer curve and the
    population's -- the part no one-measurement translate model can
    remove, however well its population curve or its gain is fitted.

    Comparing the achieved error against this, rather than against the
    lattice ceiling, is what says whether more modelling effort is worth
    anything.
    """
    per_cell = []
    for c in cells:
        best = None
        for off in range(-40, 41):
            errs = []
            for t in targets:
                qi = c.clamp(int(round(a_qi(t) + off)))
                if c.has(qi):
                    errs.append(abs(c.score[qi] - t))
            if errs:
                m = statistics.median(errs)
                if best is None or m < best[0]:
                    best = (m, off, errs)
        if best:
            per_cell.append(best)
    if not per_cell:
        return
    meds = [b[0] for b in per_cell]
    allerr = [e for b in per_cell for e in b[2]]
    print("# --- the translate family's own ceiling (perfect per-image offset) ---", file=out)
    print(f"#   per-cell median |err|: median {statistics.median(meds):.4f}  "
          f"p90 {pct(meds, 90):.4f}  max {max(meds):.4f}", file=out)
    print(f"#   pooled |err|: median {statistics.median(allerr):.4f}  "
          f"p90 {pct(allerr, 90):.4f}  p99 {pct(allerr, 99):.4f}", file=out)
    print("#   (a two-shot rule cannot beat this without modelling curve SHAPE,", file=out)
    print("#    not just offset -- and one measurement carries no shape information)", file=out)


def round_policy(x, policy):
    if policy == "nearest":
        return int(round(x))
    if policy == "at_least":  # score >= target  =>  quantizer <= predicted
        return int(math.floor(x))
    if policy == "at_most":
        return int(math.ceil(x))
    raise ValueError(policy)


def replay(cells, targets, rule, ctx, policy="nearest"):
    """Exact 2-encode replay. Returns list of per-(cell,target) records."""
    fn = RULES[rule]
    out = []
    for c in cells:
        for t in targets:
            qi1 = seed_qi(c, t, ctx)
            if not c.has(qi1):
                continue  # seed outside the measured band -> cannot replay
            s1 = c.score[qi1]
            qi2 = c.clamp(round_policy(fn(c, t, qi1, s1, ctx), policy))
            if not c.has(qi2):
                continue
            s2 = c.score[qi2]
            out.append(
                {
                    "cell": c.key(),
                    "size": c.size,
                    "target": t,
                    "qi1": qi1,
                    "s1": s1,
                    "qi2": qi2,
                    "s2": s2,
                    "err": abs(s2 - t),
                    "signed": s2 - t,
                    "err_best_of_2": min(abs(s2 - t), abs(s1 - t)),
                    "encodes": 1 if qi2 == qi1 else 2,
                    "ceiling": c.best_error(t),
                    "best_qi": c.best_qi(t),
                    "bytes": c.bytes[qi2],
                }
            )
    return out


def summarise(recs, label, out, ceiling=True):
    if not recs:
        print(f"{label}\t(no records)", file=out)
        return
    e = [r["err"] for r in recs]
    b = [r["err_best_of_2"] for r in recs]
    hit = sum(r["qi2"] == r["best_qi"] for r in recs) / len(recs)
    within1 = sum(r["qi2"] in (r["best_qi"] - 1, r["best_qi"], r["best_qi"] + 1) for r in recs) / len(recs)
    ceil_med = statistics.median([r["ceiling"] for r in recs])
    pred_med = statistics.median([max(0.0, r["err"] - r["ceiling"]) for r in recs])
    enc = statistics.mean([r["encodes"] for r in recs])
    over = sum(r["signed"] > 0 for r in recs) / len(recs)
    print(
        f"{label}\t{len(recs)}\t{statistics.median(e):.4f}\t{pct(e, 90):.4f}\t"
        f"{pct(e, 99):.4f}\t{max(e):.4f}\t{hit:.1%}\t{within1:.1%}\t"
        f"{ceil_med:.4f}\t{pred_med:.4f}\t{statistics.median(b):.4f}\t{enc:.3f}\t{over:.1%}",
        file=out,
    )


HDR = ("rule\tn\tmed_err\tp90_err\tp99_err\tmax_err\tnearest_hit\twithin1_lat\t"
       "med_LATTICE\tmed_PREDICTION\tmed_err_best2\tmean_enc\tfrac_over")


# ---------------------------------------------------------------------------
# score <-> diffmap relation (for the dm_power rule)
# ---------------------------------------------------------------------------


def fit_score_of_dm(cells):
    """Global monotone map ln(dm_mean) -> score, plus its inverse.

    Fitted as the median score in each ln(dm) bin over all TRAIN cells.
    Returns (g, g_inv, spread) where spread is the median absolute
    deviation of score within a bin -- the honest measure of how much
    information dm_mean alone carries about the score.
    """
    pts = []
    for c in cells:
        for q in c.qis:
            if c.dm[q] > 0:
                pts.append((math.log(c.dm[q]), c.score[q]))
    pts.sort()
    if not pts:
        return None, None, float("nan")
    nb = 40
    step = max(1, len(pts) // nb)
    xs, ys, dev = [], [], []
    for i in range(0, len(pts), step):
        chunk = pts[i:i + step]
        if len(chunk) < 5:
            continue
        xs.append(statistics.median([p[0] for p in chunk]))
        m = statistics.median([p[1] for p in chunk])
        ys.append(m)
        dev.append(statistics.median([abs(p[1] - m) for p in chunk]))
    # ys must descend with ln(dm) (more error = lower score)
    for i in range(1, len(ys)):
        ys[i] = min(ys[i], ys[i - 1] - 1e-6)

    def g(lndm):
        return _pwl(xs, ys, lndm)

    rys, rxs = list(reversed(ys)), list(reversed(xs))

    def g_inv(score):
        return math.exp(_pwl(rys, rxs, score))

    return g, g_inv, statistics.median(dev)


def fit_gamma(cells):
    """Median d ln(dm) / d ln(qi) over adjacent lattice points."""
    sl = []
    for c in cells:
        for a, b in zip(c.qis, c.qis[1:]):
            if b == a + 1 and c.dm[a] > 0 and c.dm[b] > 0 and a > 0:
                dl = math.log(c.dm[b]) - math.log(c.dm[a])
                dq = math.log(b) - math.log(a)
                if dq > 0:
                    sl.append(dl / dq)
    return statistics.median(sl) if sl else 1.79


# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--train", nargs="+", required=True)
    ap.add_argument("--val", nargs="+", required=True)
    ap.add_argument("--targets", default="20:90:2.5")
    ap.add_argument("--emit-rust", action="store_true")
    a = ap.parse_args()

    lo, hi, st = (float(x) for x in a.targets.split(":"))
    targets = []
    t = lo
    while t <= hi + 1e-9:
        targets.append(round(t, 4))
        t += st

    train = load_lattice(a.train)
    val = load_lattice(a.val)
    out = sys.stdout

    print(f"# train cells {len(train)}  val cells {len(val)}  targets {len(targets)} "
          f"({lo}..{hi} step {st})", file=out)
    overlap = set(k[0] for k in train) & set(k[0] for k in val)
    print(f"# source overlap train/val: {len(overlap)} (must be 0)", file=out)
    print(file=out)
    lattice_report({**train, **val}, out)
    print(file=out)

    tr, va = list(train.values()), list(val.values())
    qual_of = {qi: None for qi in range(256)}
    # quality that addresses each quantizer (inverse of the curve)
    for qi in range(256):
        x = qi / 255.0
        if x <= 0.42:
            q = 1.0 - x / 1.4
        elif x <= 0.93:
            q = 0.70 - (x - 0.42) / 0.85
        else:
            q = 0.10 - (x - 0.93) / 0.78
        qual_of[qi] = min(max(q * 100.0, 1.0), 100.0)

    # --- contexts ----------------------------------------------------------
    derived_qi, derived_sc = make_anchor_fns(DERIVED_QKNOTS)
    refit_knots = fit_quantizer_anchor(tr)
    refit_qi, refit_sc = make_anchor_fns(refit_knots)
    g, g_inv, spread = fit_score_of_dm(tr)
    gamma = fit_gamma(tr)

    print(f"# fitted gamma (d ln dm / d ln qi, TRAIN median): {gamma:.4f}", file=out)
    print(f"# score-from-ln(dm) bin spread (median abs dev, TRAIN): {spread:.3f} zensim points", file=out)
    print("# derived qknots (from the shipped quality anchor):", file=out)
    print("#   " + ", ".join(f"{v:.3f}" for v in DERIVED_QKNOTS), file=out)
    print("# refit  qknots (from the TRAIN lattice, quantizer space):", file=out)
    print("#   " + ", ".join(f"{v:.3f}" for v in refit_knots), file=out)
    print(file=out)

    base = {"qual_of": qual_of, "g_inv": g_inv, "gamma": gamma, "gain": 1.0}
    ctx_derived = dict(base, a_qi=derived_qi, a_score=derived_sc)
    ctx_refit = dict(base, a_qi=refit_qi, a_score=refit_sc)
    lm = fit_lm(tr, targets, refit_qi)
    ctx_refit["lm"] = lm
    ctx_derived["lm"] = lm
    print("# fitted residual model (TRAIN), coefficients on "
          "[1, step, off, step*off, |step|]:", file=out)
    print("#   " + ", ".join(f"{c:+.4f}" for c in lm), file=out)

    # --- gain sweep on TRAIN ----------------------------------------------
    print("# --- pass-2 gain sweep (qi_translate_gain), TRAIN ---", file=out)
    print("gain\tmed_err\tp90_err\tnearest_hit", file=out)
    best_gain, best_med = 1.0, float("inf")
    gg = 0.60
    while gg <= 1.401:
        c = dict(ctx_refit, gain=gg)
        r = replay(tr, targets, "qi_translate_gain", c)
        m = statistics.median([x["err"] for x in r])
        h = sum(x["qi2"] == x["best_qi"] for x in r) / len(r)
        print(f"{gg:.2f}\t{m:.4f}\t{pct([x['err'] for x in r], 90):.4f}\t{h:.1%}", file=out)
        if m < best_med:
            best_med, best_gain = m, gg
        gg += 0.05
    print(f"# best TRAIN gain {best_gain:.2f} (median |err| {best_med:.4f})", file=out)
    print(file=out)

    # --- rule comparison ---------------------------------------------------
    for split, cellset in (("TRAIN", tr), ("VAL", va)):
        print(f"# --- 2-encode error distribution, {split} (policy=nearest) ---", file=out)
        print(HDR, file=out)
        summarise(replay(cellset, targets, "quality_translate", ctx_derived),
                  "quality_translate(shipped loop step)", out)
        summarise(replay(cellset, targets, "qi_translate", ctx_derived),
                  "qi_translate(derived knots)", out)
        summarise(replay(cellset, targets, "qi_translate", ctx_refit),
                  "qi_translate(refit knots)", out)
        summarise(replay(cellset, targets, "qi_ratio", ctx_refit), "qi_ratio(refit)", out)
        summarise(replay(cellset, targets, "qi_translate_gain",
                         dict(ctx_refit, gain=best_gain)),
                  f"qi_translate_gain(g={best_gain:.2f},refit)", out)
        summarise(replay(cellset, targets, "dm_power", ctx_refit), "dm_power", out)
        summarise(replay(cellset, targets, "qi_translate_lm", ctx_refit),
                  "qi_translate_lm(TRAIN-fitted residual)", out)
        # pass-1-only reference: the 1-encode open loop
        recs = replay(cellset, targets, "qi_translate", ctx_refit)
        p1 = [dict(r, err=abs(r["s1"] - r["target"]), qi2=r["qi1"], encodes=1,
                   err_best_of_2=abs(r["s1"] - r["target"]),
                   signed=r["s1"] - r["target"]) for r in recs]
        summarise(p1, "pass1 only (1 encode)", out)
        # the ceiling itself, as a rule
        ceil = [dict(r, err=r["ceiling"], qi2=r["best_qi"],
                     err_best_of_2=r["ceiling"], signed=0.0) for r in recs]
        summarise(ceil, "ORACLE (nearest lattice point)", out)
        print(file=out)

    # --- paired rule comparison: is the gap real, or is it noise? ---------
    # Two medians differing by 0.09 on 433 combinations says nothing on its
    # own. Pairing on identical (cell, target) removes the cell mix, and the
    # sign test says whether the direction is consistent -- which is what
    # decides between shipping the simpler rule and the marginally better one.
    print("# --- paired rule comparisons on VAL (identical cell+target) ---", file=out)
    print("a_vs_b\tn\tmean_d\tmed_d\ta_better\ttie\tb_better\tsign_test_p", file=out)

    def paired(rule_a, ctx_a, rule_b, ctx_b, label):
        ra = {(r["cell"], r["target"]): r["err"] for r in replay(va, targets, rule_a, ctx_a)}
        rb = {(r["cell"], r["target"]): r["err"] for r in replay(va, targets, rule_b, ctx_b)}
        keys = sorted(set(ra) & set(rb), key=lambda k: (k[0], k[1]))
        if not keys:
            return
        d = [ra[k] - rb[k] for k in keys]
        wins = sum(x < -1e-9 for x in d)
        loss = sum(x > 1e-9 for x in d)
        # two-sided sign test over the non-tied pairs
        n = wins + loss
        if n:
            k = min(wins, loss)
            p = min(1.0, 2.0 * sum(math.comb(n, i) for i in range(k + 1)) / (2.0 ** n))
        else:
            p = 1.0
        print(f"{label}\t{len(keys)}\t{statistics.mean(d):+.4f}\t{statistics.median(d):+.4f}\t"
              f"{wins / len(keys):.1%}\t{(len(keys) - n) / len(keys):.1%}\t"
              f"{loss / len(keys):.1%}\t{p:.4f}", file=out)

    paired("qi_translate", ctx_refit, "quality_translate", ctx_derived,
           "qi_translate_vs_quality_translate")
    paired("qi_ratio", ctx_refit, "qi_translate", ctx_refit, "qi_ratio_vs_qi_translate")
    paired("qi_translate_gain", dict(ctx_refit, gain=best_gain), "qi_translate", ctx_refit,
           f"gain{best_gain:.2f}_vs_qi_translate")
    paired("qi_translate", ctx_refit, "qi_translate", ctx_derived,
           "refit_knots_vs_derived_knots")
    print("#   negative mean_d = the FIRST rule is closer to target", file=out)
    print("#   sign_test_p is two-sided over non-tied pairs; treat p > 0.05 as", file=out)
    print("#   'not distinguishable here' and prefer the simpler rule.", file=out)
    print(file=out)

    # --- policy comparison on the chosen rule ------------------------------
    print("# --- lattice policy, VAL, qi_translate(refit) ---", file=out)
    print(HDR, file=out)
    for pol in ("nearest", "at_least", "at_most"):
        r = replay(va, targets, "qi_translate", ctx_refit, policy=pol)
        summarise(r, f"policy={pol}", out)
        under = sum(x["signed"] < 0 for x in r) / len(r)
        print(f"#   {pol}: undershoot {under:.1%}  median signed "
              f"{statistics.median([x['signed'] for x in r]):+.4f}  "
              f"median bytes {statistics.median([x['bytes'] for x in r]):.0f}", file=out)
    print(file=out)

    # --- by size and by target band ---------------------------------------
    chosen = replay(va, targets, "qi_translate", ctx_refit)
    print("# --- chosen rule on VAL, by size ---", file=out)
    print(HDR, file=out)
    for sz in sorted({r["size"] for r in chosen}):
        summarise([r for r in chosen if r["size"] == sz], f"size={sz}", out)
    print("# --- chosen rule on VAL, by target band (equal density low/high) ---", file=out)
    print(HDR, file=out)
    for lo_b, hi_b in ((20, 40), (40, 60), (60, 80), (80, 91)):
        summarise([r for r in chosen if lo_b <= r["target"] < hi_b],
                  f"t[{lo_b},{hi_b})", out)
    print(file=out)

    if a.emit_rust:
        print("// paste into src/two_pass_zensim.rs", file=out)
        print("const ANCHOR_QUANTIZER: [f32; 15] = [", file=out)
        print("    " + ", ".join(f"{v:.3f}" for v in refit_knots) + ",", file=out)
        print("];", file=out)


if __name__ == "__main__":
    main()
