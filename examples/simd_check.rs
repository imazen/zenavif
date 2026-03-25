//! Check if SIMD is actually being used

use archmage::prelude::*;

fn main() {
    if let Some(token) = Desktop64::summon() {
        println!("AVX2/FMA (Desktop64) IS available and being used");
        println!("  Token: {:?}", std::any::type_name_of_val(&token));
    } else {
        println!("AVX2/FMA NOT available - using scalar fallback");
    }

    // Also check what CPU features are available
    #[cfg(target_arch = "x86_64")]
    {
        println!("\nCPU Features:");
        println!("  SSE4.1: {}", is_x86_feature_detected!("sse4.1"));
        println!("  AVX: {}", is_x86_feature_detected!("avx"));
        println!("  AVX2: {}", is_x86_feature_detected!("avx2"));
        println!("  FMA: {}", is_x86_feature_detected!("fma"));
    }

    #[cfg(target_arch = "aarch64")]
    {
        println!("\nCPU Features:");
        println!("  NEON: always available on aarch64");
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        println!("\nCPU Features: no SIMD detection for this architecture");
    }
}
