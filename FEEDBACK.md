# Feedback Log

## 2026-02-15
- User: "by zen* codecs, please?? also make every struct nonexhaustive, and every enum too" — wants all public types in zencodec-types-api marked `#[non_exhaustive]` for forward compatibility.
- User requested deep research on AVIF/AV1 transfer functions, bit depth, what values are stored in the bitstream, what libavif/image-rs output for u16, PNG 16-bit behavior, and performance implications of linearization.
- User confirmed "go ahead with the codec changes" to implement unified u16 convention (0–65535 gamma-encoded) across all codecs.
- User requested in-depth research on 8 codec abstraction layers: image-rs, Pillow, stb_image, ImageMagick, FFmpeg, GDK-Pixbuf, Skia, libheif. Focus on config/execution separation, pixel formats, streaming, animation, metadata, errors, threading, tradeoffs.
- User requested deep architectural research on libvips: loader/saver hierarchy, streaming/demand-driven model, VipsImage, VipsRegion, sequential access, format options, metadata, memory management, threading, error handling, pipeline composition, format negotiation.
- User requested comprehensive research on resource cost estimation and resource limits across 7 image codec libraries: libavif, libjxl, libvips, Skia, FFmpeg, image-rs, WIC. Focus on pre-decode cost fields, settable limits, enforcement timing, encode-side estimation, memory multiplier patterns.

## 2026-02-14
- User requested implementation of frame-by-frame AnimationDecoder and 16-bit animation encoding support.
- User requested prep of zenrav1e + zenavif-serialize for crates.io. Both were already published (v0.1.0 each) by the time the session started.

## 2026-02-13
- User requested comprehensive research on libavif's full feature set for both decoding and encoding, pixel output formats, and v1.1+ new features.

## 2026-02-12
- User requested comprehensive research on animated AVIF format specification, timing model, decoder API patterns, avif-parse/zenavif-parse capabilities, and HEIF sequence relationship.
