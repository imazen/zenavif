//! Debug yuv crate bilinear output

use yuv::{YuvPlanarImage, YuvRange as YuvCrateRange, YuvStandardMatrix, yuv420_to_rgb_bilinear};
use zenavif::yuv_convert::{YuvMatrix, YuvRange, yuv420_to_rgb8};

fn main() {
    // Simple test: uniform gray (Y=128, U=128, V=128)
    let width = 8;
    let height = 8;

    let y_plane = vec![128u8; width * height];
    let u_plane = vec![128u8; (width / 2) * (height / 2)];
    let v_plane = vec![128u8; (width / 2) * (height / 2)];

    // Our float SIMD
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

    // yuv crate bilinear
    let yuv_image = YuvPlanarImage {
        y_plane: &y_plane,
        y_stride: width as u32,
        u_plane: &u_plane,
        u_stride: (width / 2) as u32,
        v_plane: &v_plane,
        v_stride: (width / 2) as u32,
        width: width as u32,
        height: height as u32,
    };
    let mut yuv_crate_rgb = vec![0u8; width * height * 3];
    yuv420_to_rgb_bilinear(
        &yuv_image,
        &mut yuv_crate_rgb,
        (width * 3) as u32,
        YuvCrateRange::Full,
        YuvStandardMatrix::Bt709,
    )
    .unwrap();

    println!("Test: Y=128, U=128, V=128 (should be gray ~130)");
    println!();
    println!("Our Float SIMD (first 4 pixels):");
    for i in 0..4 {
        let p = float_result.buf()[i];
        println!("  Pixel {}: R={:3}, G={:3}, B={:3}", i, p.r, p.g, p.b);
    }

    println!();
    println!("yuv crate bilinear (first 4 pixels):");
    for i in 0..4 {
        let r = yuv_crate_rgb[i * 3];
        let g = yuv_crate_rgb[i * 3 + 1];
        let b = yuv_crate_rgb[i * 3 + 2];
        println!("  Pixel {}: R={:3}, G={:3}, B={:3}", i, r, g, b);
    }
}
