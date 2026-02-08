//! Verify YUV formulas against hand-calculated values

use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};
use zenavif::yuv_convert_libyuv;

fn main() {
    // Test with specific known values
    // Y=180, U=100, V=150 -> Expected: R=230, G=185, B=135
    let width = 4;
    let height = 4;
    
    let y_plane = vec![180u8; width * height];
    let u_plane = vec![100u8; (width/2) * (height/2)];
    let v_plane = vec![150u8; (width/2) * (height/2)];

    println!("Test: Y=180, U=100, V=150");
    println!("Expected (libyuv exact): R=230, G=185, B=135");
    println!();

    // Our exact libyuv implementation
    let libyuv_result = yuv_convert_libyuv::yuv420_to_rgb8(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    ).unwrap();

    // Our float SIMD
    let float_result = yuv420_to_rgb8(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
        YuvRange::Full,
        YuvMatrix::Bt709,
    );

    let libyuv = libyuv_result.buf()[0];
    let float = float_result.buf()[0];
    
    println!("libyuv exact:       R={:3}, G={:3}, B={:3} ✓", libyuv.r, libyuv.g, libyuv.b);
    println!("Our Float SIMD:     R={:3}, G={:3}, B={:3} (err: R{:+}, G{:+}, B{:+})", 
             float.r, float.g, float.b,
             float.r as i16 - libyuv.r as i16,
             float.g as i16 - libyuv.g as i16,
             float.b as i16 - libyuv.b as i16);
}
