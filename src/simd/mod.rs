//! SIMD implementations for AV1 decode operations
//!
//! This module contains safe SIMD implementations using archmage tokens.

#![allow(dead_code)]
#![allow(unused_imports)]

mod avg;

pub use avg::*;

mod unpremul;
#[cfg(feature = "_dev")]
pub use unpremul::unpremultiply8_dispatch;
#[cfg(not(feature = "_dev"))]
pub(crate) use unpremul::unpremultiply8_dispatch;
