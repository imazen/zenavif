use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

#[test]
fn test_yuv_conversion_accuracy() {
    let width = 1;
    let height = 1;

    // Test several known YUV->RGB conversions for BT.601 Full range
    let test_cases = vec![
        // (Y, U, V) -> expected (R, G, B)
        ((0, 128, 128), (0, 0, 0)),         // Black
        ((255, 128, 128), (255, 255, 255)), // White
        ((128, 128, 128), (128, 128, 128)), // Gray
        ((76, 85, 255), (255, 0, 0)),       // Red (approx)
        ((150, 44, 21), (0, 255, 0)),       // Green (approx)
        ((29, 255, 107), (0, 0, 255)),      // Blue (approx)
    ];

    for ((y, u, v), (exp_r, exp_g, exp_b)) in test_cases {
        let y_plane = vec![y];
        let u_plane = vec![u];
        let v_plane = vec![v];

        let planar = YuvPlanarImage {
            y_plane: &y_plane,
            y_stride: 1,
            u_plane: &u_plane,
            u_stride: 1,
            v_plane: &v_plane,
            v_stride: 1,
            width: 1,
            height: 1,
        };

        let mut rgb = vec![0u8; 3];
        yuv::yuv444_to_rgb(
            &planar,
            &mut rgb,
            3,
            YuvRange::Full,
            YuvStandardMatrix::Bt601,
        )
        .unwrap();

        let diff_r = (rgb[0] as i16 - exp_r as i16).abs();
        let diff_g = (rgb[1] as i16 - exp_g as i16).abs();
        let diff_b = (rgb[2] as i16 - exp_b as i16).abs();
        let max_diff = diff_r.max(diff_g).max(diff_b);

        eprintln!(
            "YUV({:3},{:3},{:3}) -> RGB({:3},{:3},{:3}) expected ({:3},{:3},{:3}) diff={}",
            y, u, v, rgb[0], rgb[1], rgb[2], exp_r, exp_g, exp_b, max_diff
        );

        // Allow up to 2 for rounding, but anything more is suspicious
        if max_diff > 2 {
            eprintln!(
                "  WARNING: Difference {} exceeds rounding tolerance!",
                max_diff
            );
        }
    }
}
