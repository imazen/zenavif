//! Chroma upsampling iterators for YUV plane processing
//!
//! These iterators combine Y, U, V planes into YUV pixels, handling
//! chroma subsampling via nearest-neighbor interpolation.

use yuv::YUV;

/// Iterator that combines equal-sized planes of Y, U, V into YUV pixels (4:4:4)
pub fn yuv_444<'a, T: Copy + 'a, YRowsIter, URowsIter, VRowsIter>(
    y: YRowsIter,
    u: URowsIter,
    v: VRowsIter,
) -> impl Iterator<Item = YUV<T>> + 'a
where
    YRowsIter: Iterator<Item = &'a [T]> + 'a,
    URowsIter: Iterator<Item = &'a [T]> + 'a,
    VRowsIter: Iterator<Item = &'a [T]> + 'a,
{
    y.zip(u.zip(v)).flat_map(|(y, (u, v))| {
        y.iter()
            .copied()
            .zip(u.iter().copied().zip(v.iter().copied()))
            .map(|(y, (u, v))| YUV { y, u, v })
    })
}

/// Iterator that combines planes of Y, U, V into YUV pixels, where U and V have half width (4:2:2)
///
/// Uses nearest-neighbor scaling.
pub fn yuv_422<'a, T: Copy + 'a, YRowsIter, URowsIter, VRowsIter>(
    y: YRowsIter,
    u: URowsIter,
    v: VRowsIter,
) -> impl Iterator<Item = YUV<T>> + 'a
where
    YRowsIter: Iterator<Item = &'a [T]> + 'a,
    URowsIter: Iterator<Item = &'a [T]> + 'a,
    VRowsIter: Iterator<Item = &'a [T]> + 'a,
{
    y.zip(u.zip(v)).flat_map(|(y, (u, v))| {
        let u = u
            .iter()
            .copied()
            .flat_map(|u_px| std::iter::repeat_n(u_px, 2));
        let v = v
            .iter()
            .copied()
            .flat_map(|v_px| std::iter::repeat_n(v_px, 2));
        y.iter()
            .copied()
            .zip(u.zip(v))
            .map(|(y, (u, v))| YUV { y, u, v })
    })
}

/// Iterator that combines planes of Y, U, V into YUV pixels, where U and V have half width and half height (4:2:0)
///
/// Uses nearest-neighbor scaling.
pub fn yuv_420<'a, T: Copy + 'a, YRowsIter, URowsIter, VRowsIter>(
    y: YRowsIter,
    u: URowsIter,
    v: VRowsIter,
) -> impl Iterator<Item = YUV<T>> + 'a
where
    YRowsIter: Iterator<Item = &'a [T]> + 'a,
    URowsIter: Iterator<Item = &'a [T]> + 'a,
    VRowsIter: Iterator<Item = &'a [T]> + 'a,
{
    let u = u.flat_map(|u_row| std::iter::repeat_n(u_row, 2));
    let v = v.flat_map(|v_row| std::iter::repeat_n(v_row, 2));
    y.zip(u.zip(v)).flat_map(|(y, (u, v))| {
        let u = u
            .iter()
            .copied()
            .flat_map(|u_px| std::iter::repeat_n(u_px, 2));
        let v = v
            .iter()
            .copied()
            .flat_map(|v_px| std::iter::repeat_n(v_px, 2));
        y.iter()
            .copied()
            .zip(u.zip(v))
            .map(|(y, (u, v))| YUV { y, u, v })
    })
}
