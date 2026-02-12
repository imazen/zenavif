//! Debug YUV conversion accuracy

use archmage::prelude::*;
use zenavif::yuv_convert::{YuvMatrix, YuvRange, yuv420_to_rgb8};
use zenavif::yuv_convert_fast::yuv420_to_rgb8_fast;

fn main() {
    // Simple test case: gray pixel (Y=128, U=128, V=128) should give ~gray RGB
    let width = 4;
    let height = 4;

    let y_plane = vec![128u8; width * height];
    let u_plane = vec![128u8; (width / 2) * (height / 2)];
    let v_plane = vec![128u8; (width / 2) * (height / 2)];

    let float_result = yuv420_to_rgb8(
        &y_plane,
        width,
        &u_plane,
        width / 2,
        &v_plane,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    let fast_result = if let Some(token) = Desktop64::summon() {
        yuv420_to_rgb8_fast(
            token,
            &y_plane,
            width,
            &u_plane,
            width / 2,
            &v_plane,
            width / 2,
            width,
            height,
        )
    } else {
        panic!("AVX2 not available");
    };

    println!("Test: Y=128, U=128, V=128 (should be ~gray)");
    println!();
    println!("Float SIMD result (first 4 pixels):");
    for i in 0..4 {
        let p = float_result.buf()[i];
        println!("  Pixel {}: R={}, G={}, B={}", i, p.r, p.g, p.b);
    }
    println!();
    println!("Fast Integer result (first 4 pixels):");
    for i in 0..4 {
        let p = fast_result.buf()[i];
        println!("  Pixel {}: R={}, G={}, B={}", i, p.r, p.g, p.b);
    }

    // Test with varying values
    println!("\n--- Test with Y=180, U=100, V=150 ---");
    let y_plane2 = vec![180u8; width * height];
    let u_plane2 = vec![100u8; (width / 2) * (height / 2)];
    let v_plane2 = vec![150u8; (width / 2) * (height / 2)];

    let float_result2 = yuv420_to_rgb8(
        &y_plane2,
        width,
        &u_plane2,
        width / 2,
        &v_plane2,
        width / 2,
        width,
        height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    let fast_result2 = if let Some(token) = Desktop64::summon() {
        yuv420_to_rgb8_fast(
            token,
            &y_plane2,
            width,
            &u_plane2,
            width / 2,
            &v_plane2,
            width / 2,
            width,
            height,
        )
    } else {
        panic!("AVX2 not available");
    };

    println!("Float SIMD:");
    let p = float_result2.buf()[0];
    println!("  Pixel 0: R={}, G={}, B={}", p.r, p.g, p.b);

    println!("Fast Integer:");
    let p = fast_result2.buf()[0];
    println!("  Pixel 0: R={}, G={}, B={}", p.r, p.g, p.b);
}
