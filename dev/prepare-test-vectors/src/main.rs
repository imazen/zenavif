//! Fetch original test inputs into codec-corpus; preserve existing local vectors.
#![forbid(unsafe_code)]
use sha2::{Digest as _, Sha256};
use std::path::Path;
fn copy_missing(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let path = entry?.path();
        if path.is_dir() {
            copy_missing(&path, dst)?;
        } else if path.extension().is_some_and(|e| e == "avif") {
            let dest = dst.join(path.file_name().unwrap());
            if !dest.exists() {
                std::fs::copy(&path, dest)?;
            }
        }
    }
    Ok(())
}
fn main() {
    let out = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("vector directory required"),
    );
    let corpus = codec_corpus::Corpus::new().expect("codec-corpus cache");
    let source = corpus
        .github_repo("AOMediaCodec/libavif", "tests/data", "v1.4.2")
        .expect("fetch pinned libavif original fixtures");
    let name = "abc_color_irot_alpha_irot.avif";
    let bytes = std::fs::read(source.join(name)).expect("original alpha/rotation fixture");
    assert_eq!(
        Sha256::digest(&bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>(),
        "b371cc88244a873131e4d10ff9363d71ce4f41cf333bd4a491b38d970d9abd3b",
        "original fixture identity"
    );
    copy_missing(&source, &out).expect("copy missing vectors without replacing existing inputs");
    assert_eq!(std::fs::read(out.join(name)).unwrap(), bytes);
}
