//! Check if SIMD is actually being used

use archmage::prelude::*;

fn main() {
    if let Some(token) = Desktop64::summon() {
        println!("✓ AVX2/FMA (Desktop64) IS available and being used");
        println!("  Token: {:?}", std::any::type_name_of_val(&token));
    } else {
        println!("✗ AVX2/FMA NOT available - using scalar fallback");
    }

    // Also check what CPU features are available
    println!("\nCPU Features:");
    println!("  SSE4.1: {}", is_x86_feature_detected!("sse4.1"));
    println!("  AVX: {}", is_x86_feature_detected!("avx"));
    println!("  AVX2: {}", is_x86_feature_detected!("avx2"));
    println!("  FMA: {}", is_x86_feature_detected!("fma"));
}
