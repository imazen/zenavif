//! zencodec trait implementations for zenavif.
//!
//! Provides [`AvifEncoderConfig`] and [`AvifDecoderConfig`] types that implement
//! the trait hierarchy from zencodec, wrapping the native zenavif API.
//!
//! # Trait mapping
//!
//! | zencodec | zenavif adapter |
//! |----------------|-----------------|
//! | `EncoderConfig` | [`AvifEncoderConfig`] |
//! | `EncodeJob` | [`AvifEncodeJob`] |
//! | `Encoder` | [`AvifEncoder`] |
//! | `DecoderConfig` | [`AvifDecoderConfig`] |
//! | `DecodeJob<'a>` | [`AvifDecodeJob`] |
//! | `Decode` | [`AvifDecoder`] |
//! | `AnimationFrameEncoder` | [`AvifAnimationFrameEncoder`] |
//! | `AnimationFrameDecoder` | [`AvifAnimationFrameDecoder`] |
//!
//! # Layout
//!
//! Each adapter type owns a submodule. The shared decode-side helpers
//! (`orientation`, `color`, `info`, `gain_map`, `negotiate`) and the
//! thread-count policy (`threads`) sit beside them.

// ── Pattern B envelope: the original At<Error> method bodies live in private
// inherent `*_inner` helpers. Each zencodec trait method in these submodules is
// a thin `self.*_inner(..).map_err(zencodec::CodecError::of)` boundary that
// re-wraps the located native error into the shared `At<CodecError>` envelope
// (Pattern B), preserving the whereat trace. The inherent helpers keep the
// verbatim logic.

mod anim_decoder;
#[cfg(feature = "encode")]
mod anim_encoder;
mod color;
mod decode_config;
mod decode_job;
mod decoder;
#[cfg(feature = "encode")]
mod encode_config;
#[cfg(feature = "encode")]
mod encode_job;
#[cfg(feature = "encode")]
mod encoder;
mod gain_map;
mod info;
mod negotiate;
mod orientation;
mod streaming;
mod threads;

pub use anim_decoder::AvifAnimationFrameDecoder;
#[cfg(feature = "encode")]
pub use anim_encoder::AvifAnimationFrameEncoder;
pub use decode_config::AvifDecoderConfig;
pub use decode_job::AvifDecodeJob;
pub use decoder::AvifDecoder;
#[cfg(feature = "encode")]
pub use encode_config::AvifEncoderConfig;
#[cfg(feature = "encode")]
pub use encode_job::AvifEncodeJob;
#[cfg(feature = "encode")]
pub use encoder::AvifEncoder;
