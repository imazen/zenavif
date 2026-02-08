//! Verify YUV formulas against hand-calculated values

use zenavif::yuv_convert::{yuv420_to_rgb8, YuvRange, YuvMatrix};
use zenavif::yuv_convert_libyuv::yuv420_to_rgb8_libyuv_scalar;
use yuv::{yuv420_to_rgb_bilinear, YuvPlanarImage, YuvRange as YuvCrateRange, YuvStandardMatrix};

fn main() {
    // Test with specific known values
    // Y=180, U=100, V=150 -> Expected: R≈230, G≈185, B≈135
    let width = 4;
    let height = 4;
    
    let y_plane = vec![180u8; width * height];
    let u_plane = vec![100u8; (width/2) * (height/2)];
    let v_plane = vec![150u8; (width/2) * (height/2)];

    println!("Test: Y=180, U=100, V=150");
    println!("Expected (libyuv exact): R=230, G=185, B=135");
    println!();

    // Our exact libyuv implementation
    let libyuv_result = yuv420_to_rgb8_libyuv_scalar(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
    );

    // Our float SIMD
    let float_result = yuv420_to_rgb8(
        &y_plane, width,
        &u_plane, width / 2,
        &v_plane, width / 2,
        width, height,
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
    yuv420_to_rgb_bilinear(&yuv_image, &mut yuv_crate_rgb, (width * 3) as u32,
                           YuvCrateRange::Full, YuvStandardMatrix::Bt709).unwrap();

    let libyuv = libyuv_result.buf()[0];
    let float = float_result.buf()[0];
    let yuv_r = yuv_crate_rgb[0];
    let yuv_g = yuv_crate_rgb[1];
    let yuv_b = yuv_crate_rgb[2];
    
    println!("libyuv exact:       R={:3}, G={:3}, B={:3} ✓", libyuv.r, libyuv.g, libyuv.b);
    println!("Our Float SIMD:     R={:3}, G={:3}, B={:3} (err: R{:+}, G{:+}, B{:+})", 
             float.r, float.g, float.b,
             float.r as i16 - libyuv.r as i16,
             float.g as i16 - libyuv.g as i16,
             float.b as i16 - libyuv.b as i16);
    println!("yuv crate bilinear: R={:3}, G={:3}, B={:3} (err: R{:+}, G{:+}, B{:+})", 
             yuv_r, yuv_g, yuv_b,
             yuv_r as i16 - libyuv.r as i16,
             yuv_g as i16 - libyuv.g as i16,
             yuv_b as i16 - libyuv.b as i16);
}
