> **UPDATE 2026-08-06:** the two `.tsv.zst` files are now COMMITTED beside
> this pointer (110 KB / 106 KB), on the user's call — they exceed the 30 KB
> rule even compressed, and that waiver was theirs to give, not mine. Read
> them with `zstd -dc <file>.tsv.zst`. The block-storage copy and its
> sha256s below remain valid as a second location.

# Dense achievable-score lattice, 2026-08-06 — NOT committed (pointer)

The two raw lattice TSVs are **not in git**: at 112 KB and 108 KB
zstd-compressed they exceed the repo's 30 KB commit rule, and that rule is
not mine to waive. They are not compressible below it either — stripped to
the bare minimum (short image ids, no `w`/`h`/`q`/`enc_ms`, no `dm_mean`)
the train file still zstds to 54 KB, because ~7 bytes per row of score and
byte-count digits is genuine entropy, not formatting.

That is fine here, because they are **deterministically regenerable** and
about to be superseded (the constants get re-fitted at the final arms
encoder anyway). Everything a reader needs to reach a conclusion — the
lattice geometry, the rule comparison, the paired tests — is in the
committed `zensim_two_shot_fit_2026-08-06.txt`.

## Where they are

    /Users/lilith/work/zen/_bench-data/zenavif-two-shot-2026-08-06/
        zensim_two_shot_lattice_train_2026-08-06.tsv.zst
        zensim_two_shot_lattice_val_2026-08-06.tsv.zst
        provenance_pinned.txt

sha256:

    eb073d4b26cc76a5d5a9408cb5d7f1959ae374271f7a4244997b8615112e474b
        zensim_two_shot_lattice_train_2026-08-06.tsv.zst   (35 cells, 3,935 rows)
    9a20a29b8ba8e24887e6971c7c60027845cb676786b17e0a4477186eee55fdfc
        zensim_two_shot_lattice_val_2026-08-06.tsv.zst     (34 cells, 3,916 rows)

Read with `zstd -dc <file> | less`. Columns:
`image size w h q qindex bytes zensim dm_mean enc_ms`.

**This is a local path on one macOS box, not a backup.** If these matter to
you, regenerate them rather than hoping they survived.

## How to regenerate

    scripts/hyperparam/run_zensim_two_shot.sh <outdir>

which rebuilds, records provenance, sweeps both splits, fits, and runs the
A/B. To reproduce *these exact bytes* you also need the encoder state they
were taken at (`provenance_pinned.txt`, copied alongside them):

    ravif    619d81adcaa5dd5546d55bf669560e9eeb74d080, src sha256
             361e222c74eb655a6bfb2d907e46222fab80c3947874b73a75b7203d452926b1
    zenrav1e 4d1ed620703d7eb94bb24467b44c5b498896af62, clean, src sha256
             aa81fbf7210246d162d3b8952ba93393cba07dbef9448f6c7527ff3edc488bc7
    binary   sha256 b030f1ba07cd8a4f3a0b804d55452aa80bebfa678aa0cb9c2f660667f47a5687
    zensim   imazen/zensim main @ e4b875b5 (0.3.0), profile codec_target() == B

619d81a predates ravif c69050a, which arms four of the five release-gated
speed rows. These tables therefore describe the **pre-arm** encoder; see
`zensim_two_shot_2026-08-06.tsv.meta` for how much that was measured to
matter (median zero at these configs, tail to 2.8 zensim on one source).

## Coverage

69 of 72 planned cells. 12 sources (6 TRAIN + 6 held-out VAL, disjoint by
source) × long edges 64 / 256 / 1024, every reachable quantizer in the band
covering zensim 12..96 (~200–240 quantizers per cell).

Three cells are missing — one TRAIN, two VAL — because the last two shards
were stopped after box contention pushed per-cell time from ~90 s to over
15 minutes. Cells are written whole, so no partial cell is present
(validated: 0 malformed rows). Losing 3 of 72 cells moves nothing here; it
is recorded because a silently short corpus is how a corpus becomes a lie.
