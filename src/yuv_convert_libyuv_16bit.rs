//! 16-bit YUV to RGB conversion (for 10/12-bit content and HDR)

use imgref::ImgVec;
use rgb::RGB16;
use crate::yuv_convert::{YuvRange, YuvMatrix};

/// YUV conversion constants for 16-bit
#[allow(dead_code)]
struct YuvConstants16 {
    yg: i32,
    ygb: i32,
    ub: i32,
    ug: i32,
    vg: i32,
    vr: i32,
    bb: i32,
    bg: i32,
    br: i32,
}

impl YuvConstants16 {
    /// BT.709 Full Range (same as 8-bit)
    const BT709_FULL: Self = Self {
        yg: 18997,
        ygb: -1160,
        ub: -128,
        ug: 14,
        vg: 34,
        vr: -115,
        bb: -17544,
        bg: 4984,
        br: -15880,
    };
    
    /// BT.2020 Full Range (for HDR content)
    const BT2020_FULL: Self = Self {
        yg: 18997,   // 1.164 * 64 * 256 * 256 / 257
        ygb: -1160,  // 1.164 * 64 * -16 + 64 / 2
        ub: -144,    // -2.251 * 64 (approximate)
        ug: 16,      // 0.256 * 64 (approximate)
        vg: 56,      // 0.875 * 64 (approximate)
        vr: -112,    // -1.750 * 64 (approximate)
        bb: -144 * 128 + (-1160),  // -19592
        bg: 16 * 128 + 56 * 128 + (-1160),  // 8056
        br: -112 * 128 + (-1160),  // -15496
    };
}

fn get_constants_16(matrix: YuvMatrix, range: YuvRange) -> Option<&'static YuvConstants16> {
    match (matrix, range) {
        (YuvMatrix::Bt709, YuvRange::Full) => Some(&YuvConstants16::BT709_FULL),
        (YuvMatrix::Bt2020, YuvRange::Full) => Some(&YuvConstants16::BT2020_FULL),
        _ => None,
    }
}

/// Convert single 16-bit YUV pixel to RGB16
/// 
/// Input: 10-bit or 12-bit YUV values (0-1023 or 0-4095)
/// Output: 16-bit RGB (0-65535)
#[inline(always)]
fn yuv_pixel_16(y: u16, u: u16, v: u16, bit_depth: u32, c: &YuvConstants16) -> RGB16 {
    // Scale down to 8-bit range for formula (libyuv approach)
    let shift = if bit_depth > 8 { bit_depth - 8 } else { 0 };
    let y8 = (y >> shift) as u8;
    let u8 = (u >> shift).min(255) as u8;
    let v8 = (v >> shift).min(255) as u8;
    
    // Apply libyuv formula
    let y1 = ((y8 as u32) * 0x0101 * (c.yg as u32)) >> 16;
    let y1 = y1 as i32;
    
    let b_raw = (-((u8 as i32) * c.ub) + y1 + c.bb) >> 6;
    let g_raw = (-((u8 as i32) * c.ug + (v8 as i32) * c.vg) + y1 + c.bg) >> 6;
    let r_raw = (-((v8 as i32) * c.vr) + y1 + c.br) >> 6;
    
    // Clamp to 8-bit, then scale to 16-bit
    let r8 = r_raw.clamp(0, 255) as u16;
    let g8 = g_raw.clamp(0, 255) as u16;
    let b8 = b_raw.clamp(0, 255) as u16;
    
    // Scale 8-bit -> 16-bit (multiply by 257 for perfect mapping)
    RGB16 {
        r: r8 * 257,
        g: g8 * 257,
        b: b8 * 257,
    }
}

/// Convert YUV420 16-bit to RGB16 (for 10/12-bit content)
pub fn yuv420_to_rgb16(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    height: usize,
    bit_depth: u32,
    range: YuvRange,
    matrix: YuvMatrix,
) -> Option<ImgVec<RGB16>> {
    let c = get_constants_16(matrix, range)?;
    let mut out = vec![RGB16::default(); width * height];
    
    for y in 0..height {
        let chroma_y = y / 2;
        for x in 0..width {
            let chroma_x = x / 2;
            
            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[chroma_y * u_stride + chroma_x];
            let v_val = v_plane[chroma_y * v_stride + chroma_x];
            
            out[y * width + x] = yuv_pixel_16(y_val, u_val, v_val, bit_depth, c);
        }
    }
    
    Some(ImgVec::new(out, width, height))
}

/// Convert YUV422 16-bit to RGB16
pub fn yuv422_to_rgb16(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    height: usize,
    bit_depth: u32,
    range: YuvRange,
    matrix: YuvMatrix,
) -> Option<ImgVec<RGB16>> {
    let c = get_constants_16(matrix, range)?;
    let mut out = vec![RGB16::default(); width * height];
    
    for y in 0..height {
        for x in 0..width {
            let chroma_x = x / 2;
            
            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[y * u_stride + chroma_x];
            let v_val = v_plane[y * v_stride + chroma_x];
            
            out[y * width + x] = yuv_pixel_16(y_val, u_val, v_val, bit_depth, c);
        }
    }
    
    Some(ImgVec::new(out, width, height))
}

/// Convert YUV444 16-bit to RGB16
pub fn yuv444_to_rgb16(
    y_plane: &[u16],
    y_stride: usize,
    u_plane: &[u16],
    u_stride: usize,
    v_plane: &[u16],
    v_stride: usize,
    width: usize,
    height: usize,
    bit_depth: u32,
    range: YuvRange,
    matrix: YuvMatrix,
) -> Option<ImgVec<RGB16>> {
    let c = get_constants_16(matrix, range)?;
    let mut out = vec![RGB16::default(); width * height];
    
    for y in 0..height {
        for x in 0..width {
            let y_val = y_plane[y * y_stride + x];
            let u_val = u_plane[y * u_stride + x];
            let v_val = v_plane[y * v_stride + x];
            
            out[y * width + x] = yuv_pixel_16(y_val, u_val, v_val, bit_depth, c);
        }
    }
    
    Some(ImgVec::new(out, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_10bit_conversion() {
        // Test with 10-bit values
        let width = 4;
        let height = 4;
        
        // 10-bit: Y=512 (mid), U=512 (mid), V=512 (mid) -> should give gray
        let y_plane = vec![512u16; width * height];
        let u_plane = vec![512u16; (width/2) * (height/2)];
        let v_plane = vec![512u16; (width/2) * (height/2)];
        
        let result = yuv420_to_rgb16(
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
            10,  // 10-bit
            YuvRange::Full,
            YuvMatrix::Bt709,
        ).unwrap();
        
        // Should produce some gray value
        let pixel = result.buf()[0];
        assert!(pixel.r > 0 && pixel.r < 65535);
        assert!(pixel.g > 0 && pixel.g < 65535);
        assert!(pixel.b > 0 && pixel.b < 65535);
    }
    
    #[test]
    fn test_bt2020_supported() {
        let width = 4;
        let height = 4;
        
        let y_plane = vec![512u16; width * height];
        let u_plane = vec![512u16; (width/2) * (height/2)];
        let v_plane = vec![512u16; (width/2) * (height/2)];
        
        let result = yuv420_to_rgb16(
            &y_plane, width,
            &u_plane, width / 2,
            &v_plane, width / 2,
            width, height,
            10,
            YuvRange::Full,
            YuvMatrix::Bt2020,
        );
        
        assert!(result.is_some(), "BT.2020 should be supported for HDR");
    }
}
