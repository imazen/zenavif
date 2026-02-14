use zenavif_serialize::Aviffy;
use std::fs;
use std::path::Path;

fn main() {
    let path = std::env::args_os().nth(1).expect("Please specify path to an AVIF file to optimize");

    let avif_file = fs::read(&path).expect("Can't load input image");

    let parser = zenavif_parse::AvifParser::from_owned(avif_file).unwrap();
    let meta = parser.primary_metadata().expect("Can't read AV1 metadata");

    let primary = parser.primary_data().expect("Can't read primary data");
    let alpha = parser.alpha_data().and_then(|r| r.ok());

    let out = Aviffy::new()
        .set_seq_profile(meta.seq_profile)
        .set_chroma_subsampling(meta.chroma_subsampling)
        .set_monochrome(meta.monochrome)
        .to_vec(
            primary.as_ref(),
            alpha.as_deref(),
            meta.max_frame_width.get(),
            meta.max_frame_height.get(),
            meta.bit_depth,
        );

    let new_path = Path::new(&path).with_extension("rewrite.avif");
    fs::write(&new_path, out).expect("Can't write new file");
    eprintln!("Written {}", new_path.display());
}
