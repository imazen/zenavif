use std::fs;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: inspect_planes <avif-path>");
    let data = fs::read(&path).unwrap();

    // Parse AVIF container
    let parse_config = zenavif_parse::DecodeConfig::default().lenient(true);
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        data.clone(),
        &parse_config,
        &enough::Unstoppable,
    )
    .unwrap();

    let primary_data = parser.primary_data().unwrap();
    eprintln!("Primary data: {} bytes", primary_data.len());

    // Decode with rav1d
    let settings = rav1d_safe::src::managed::Settings {
        threads: 1,
        ..Default::default()
    };
    let mut decoder = rav1d_safe::src::managed::Decoder::with_settings(settings).unwrap();
    let frame = match decoder.decode(&primary_data) {
        Ok(Some(f)) => f,
        Ok(None) => {
            let frames = decoder.flush().unwrap();
            frames.into_iter().last().unwrap()
        }
        Err(e) => panic!("decode error: {e:?}"),
    };

    println!(
        "Frame: {}x{} @ {}bpc, layout={:?}",
        frame.width(),
        frame.height(),
        frame.bit_depth(),
        frame.pixel_layout()
    );
    let color = frame.color_info();
    println!(
        "Color: primaries={:?} transfer={:?} matrix={:?} range={:?}",
        color.primaries,
        color.transfer_characteristics,
        color.matrix_coefficients,
        color.color_range
    );

    match frame.planes() {
        rav1d_safe::src::managed::Planes::Depth8(planes) => {
            let y = planes.y();
            println!(
                "Y plane: {}x{} stride={}",
                y.width(),
                y.height(),
                y.stride()
            );
            let y_data = y.as_slice();
            println!("Y first 10 values: {:?}", &y_data[..10.min(y_data.len())]);
            // Show Y mean
            let y_mean: f64 = y_data
                .iter()
                .take(y.width() * y.height())
                .map(|&v| v as f64)
                .sum::<f64>()
                / (y.width() * y.height()) as f64;
            println!("Y mean: {y_mean:.1}");

            if let Some(u) = planes.u() {
                println!(
                    "U plane: {}x{} stride={}",
                    u.width(),
                    u.height(),
                    u.stride()
                );
                let u_data = u.as_slice();
                println!("U first 10 values: {:?}", &u_data[..10.min(u_data.len())]);
                let u_mean: f64 = u_data
                    .iter()
                    .take(u.width() * u.height())
                    .map(|&v| v as f64)
                    .sum::<f64>()
                    / (u.width() * u.height()) as f64;
                println!("U mean: {u_mean:.1}");
            } else {
                println!("U plane: MISSING");
            }

            if let Some(v) = planes.v() {
                println!(
                    "V plane: {}x{} stride={}",
                    v.width(),
                    v.height(),
                    v.stride()
                );
                let v_data = v.as_slice();
                println!("V first 10 values: {:?}", &v_data[..10.min(v_data.len())]);
                let v_mean: f64 = v_data
                    .iter()
                    .take(v.width() * v.height())
                    .map(|&v| v as f64)
                    .sum::<f64>()
                    / (v.width() * v.height()) as f64;
                println!("V mean: {v_mean:.1}");
            } else {
                println!("V plane: MISSING");
            }
        }
        rav1d_safe::src::managed::Planes::Depth16(planes) => {
            let y = planes.y();
            println!(
                "Y plane (16bit): {}x{} stride={}",
                y.width(),
                y.height(),
                y.stride()
            );
        }
    }
}
