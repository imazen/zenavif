//! AVIF decoder implementation using rav1d-safe managed API
//!
//! This module provides a 100% safe implementation using the managed API.
//! No unsafe code required!
//!
//! # Layout
//!
//! [`ManagedAvifDecoder`] owns the parser and the AV1 decoder; its inherent
//! methods are grouped by concern across the submodules below (rustdoc merges
//! them back into one page):
//!
//! | module | what lives there |
//! |---|---|
//! | `decoder` | the struct, construction, `decode_frame`, `decode` / `decode_full` |
//! | `metadata` | `ImageInfo` derivation, `probe_info`, gain map, container queries |
//! | `frame_convert` | decoded `Frame` → `PixelBuffer` driver (crop, depth, alpha) |
//! | `plane_convert` | the stateless YUV-plane → pixel-buffer kernels |
//! | `grid` | tiled (grid) decode and canvas stitching |
//! | `sink` | row-streaming output (`decode_to_sink`) |
//! | `animation` | `decode_animation` and [`AnimationDecoder`] |
//! | `cicp_map` | rav1d ↔ zenavif ↔ `yuv` crate enum translation |
//! | `aom` | the `aom-backend`-gated still/grid decode path |

#![deny(unsafe_code)]

mod animation;
#[cfg(feature = "aom-backend")]
mod aom;
mod cicp_map;
mod decoder;
mod frame_convert;
mod grid;
mod metadata;
mod plane_convert;
mod sink;

pub use animation::AnimationDecoder;
pub use decoder::ManagedAvifDecoder;
