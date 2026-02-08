//! Exact libyuv YUV to RGB conversion
//!
//! This module replicates libyuv's exact integer math for pixel-perfect matching.
//! Constants and formulas from libyuv row_common.cc (BT.709)

use imgref::ImgVec;
use rgb::RGB8;

/// BT.709 constants from libyuv
const YG: i32 = 18997;   // 1.164 * 64 * 256 * 256 / 257
const YGB: i32 = -1160;  // 1.164 * 64 * -16 + 64 / 2
const UB: i32 = -128;    // -2.112 * 64
const UG: i32 = 14;      // 0.213 * 64
const VG: i32 = 34;      // 0.533 * 64
const VR: i32 = -115;    // -1.793 * 64

/// Precomputed bias values
const BB: i32 = UB * 128 + YGB;  // -17544
const BG: i32 = UG * 128 + VG * 128 + YGB;  // 4984
const BR: i32 = VR * 128 + YGB;  // -15880

/// Convert single YUV pixel to RGB using exact libyuv formula
#[inline(always)]
fn yuv_pixel(y: u8, u: u8, v: u8) -> RGB8 {
    // libyuv YuvPixel formula:
    // y1 = (y * 0x0101 * YG) >> 16
    // b = (-(u * UB) + y1 + BB) >> 6
    // g = (-(u * UG + v * VG) + y1 + BG) >> 6
    // r = (-(v * VR) + y1 + BR) >> 6
    
    let y1 = ((y as u32) * 0x0101 * (YG as u32)) >> 16;
    let y1 = y1 as i32;
    
    let b_raw = (-((u as i32) * UB) + y1 + BB) >> 6;
    let g_raw = (-((u as i32) * UG + (v as i32) * VG) + y1 + BG) >> 6;
    let r_raw = (-((v as i32) * VR) + y1 + BR) >> 6;
    
    RGB8 {
        r: r_raw.clamp(0, 255) as u8,
        g: g_raw.clamp(0, 255) as u8,
        b: b_raw.clamp(0, 255) as u8,
    }
}

/// Convert YUV420 to RGB8 using exact libyuv math (scalar version)
pub fn yuv420_to_rgb8_libyuv_scalar(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: usize,
    height: usize,
) -> ImgVec<RGB8> {
    let mut out = vec![RGB8::default(); width * height];
    
    for y in 0..height {
        let chroma_y = y / 2;
        for x in 0..width {
            let chroma_x = x / 2;
            
            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[chroma_y * u_stride + chroma_x];
            let v_val = v_plane[chroma_y * v_stride + chroma_x];
            
            out[y * width + x] = yuv_pixel(y_val, u_val, v_val);
        }
    }
    
    ImgVec::new(out, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_yuv_pixel() {
        // Test with Y=180, U=100, V=150
        // Expected: R=230, G=185, B=135 (from manual calculation)
        let rgb = yuv_pixel(180, 100, 150);
        assert_eq!(rgb.r, 230);
        assert_eq!(rgb.g, 185);
        assert_eq!(rgb.b, 135);
    }
    
    #[test]
    fn test_yuv420_conversion() {
        // Simple 4x4 test
        let width = 4;
        let height = 4;
        
        let y_plane = vec![180u8; width * height];
        let u_plane = vec![100u8; (width/2) * (height/2)];
        let v_plane = vec![150u8; (width/2) * (height/2)];
        
        let result = yuv420_to_rgb8_libyuv_scalar(
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
        );
        
        // All pixels should be R=230, G=185, B=135
        for pixel in result.buf() {
            assert_eq!(pixel.r, 230);
            assert_eq!(pixel.g, 185);
            assert_eq!(pixel.b, 135);
        }
    }
}
