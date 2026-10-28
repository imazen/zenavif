# SVT full-resolution stills

The `zenav1-svt` backend accepts 8-bit 4:4:4 stills with full-range BT.601
YCbCr or identity RGB (G/B/R planes). Existing 8/10-bit 4:2:0 remains available.
The color stream and its av1C entry use profile 1 for 4:4:4. A separate alpha
item remains profile-0 monochrome; its samples and signaling do not inherit
color subsampling.

For source-lossless RGB or RGBA, select `EncodeColorModel::Rgb`,
`EncodeChromaSubsampling::Yuv444`, and `with_lossless(true)`. This selects QP 0
for color and alpha. All channels, including RGB under alpha zero, are kept.
The generic fidelity adapter retains the same mode even without encode-imazen.
QP 0 over converted or subsampled YCbCr is not described as source-lossless.

The supported still extension excludes high-precision input, 10-bit 4:4:4,
limited range, premultiplied alpha, film grain or parity policy on 4:4:4,
non-default lossless coding tools, and 4:4:4 animation. These combinations
return errors. The quality dial continues to select QP >= 1.

The SVT pin d0f6edc0 contains the still facade and an identity sequence-header
repair from [SVT PR 28](https://github.com/imazen/zenav1-svt/pull/28). Before the
repair, four new identity cells were rejected by aomdec; the required
separate-UV-delta-Q field was missing after the identity branch.

Validation on 2026-10-28:

- `cargo test --locked -p zenavif --features zenav1-svt --test svt_444`
  exercises native/trait lossless without the zenravif extension feature.
- `cargo test --locked -p zenavif --features zenav1-svt,encode-imazen
  --test svt_444 --test svt_rs_backend --test backend_router` preserves the
  4:2:0 checks and replaces old 4:4:4 refusals with format/pixel checks.
- `just test-svt-444-oracle <directory>` requires libavif, aomdec and dav1d.
  Six RGB/RGBA files at 1x1, 7x9 and 65x67 preserve all source channels in
  libavif PNG output; aomdec and dav1d exactly preserve the G/B/R and alpha
  planes. The tool fails if an oracle is missing; it has no skip branch.
- Scoped strict lint passes for the changed library, tests and oracle example.

This is a still/RGB8 extension. It does not qualify 10-bit 4:4:4, source-lossless
16-bit input, animation, or arbitrary tool combinations.
