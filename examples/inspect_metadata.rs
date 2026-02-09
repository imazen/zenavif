use std::fs;

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect_metadata <avif-path>");
    let data = fs::read(&path).unwrap();

    let parse_config = zenavif_parse::DecodeConfig::default().lenient(true);
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        data,
        &parse_config,
        &enough::Unstoppable,
    )
    .unwrap();

    println!("Color info: {:?}", parser.color_info());
    println!("Has alpha: {:?}", parser.alpha_data().is_some());
    println!("Premultiplied alpha: {:?}", parser.premultiplied_alpha());
    println!("Grid config: {:?}", parser.grid_config());
    println!("Tile count: {:?}", parser.grid_tile_count());

    // Check AVIF boxes
    if let Some(ci) = parser.color_info() {
        match ci {
            zenavif_parse::ColorInformation::Nclx {
                color_primaries,
                transfer_characteristics,
                matrix_coefficients,
                full_range,
            } => {
                println!(
                    "NCLX: cp={} tc={} mc={} full_range={}",
                    *color_primaries as u16,
                    *transfer_characteristics as u16,
                    *matrix_coefficients as u16,
                    full_range
                );
            }
            zenavif_parse::ColorInformation::IccProfile(data) => {
                println!("ICC profile: {} bytes", data.len());
            }
        }
    } else {
        println!("No color_info (no colr box)");
    }
}
