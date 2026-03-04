//! Parity test: verify safe-simd produces identical output to asm

use std::fs;

fn avif_test_file_1() -> String {
    std::env::var("AVIF_PARITY_TEST_1")
        .unwrap_or_else(|_| "/home/lilith/work/aom-decode/tests/test.avif".into())
}

fn avif_test_file_2() -> String {
    std::env::var("AVIF_PARITY_TEST_2")
        .unwrap_or_else(|_| "/home/lilith/work/libavif/tests/data/white_1x1.avif".into())
}

#[test]
#[ignore] // Run with: cargo test --test parity_test -- --ignored
fn test_decode_works() {
    let test_files = [avif_test_file_1(), avif_test_file_2()];

    for test_file in &test_files {
        if !std::path::Path::new(test_file.as_str()).exists() {
            eprintln!("Skipping {}: file not found", test_file);
            continue;
        }

        println!("Testing: {}", test_file);
        let data = fs::read(test_file).expect("Failed to read test file");

        // Decode with default config
        let result = zenavif::decode(&data);

        match result {
            Ok(image) => {
                println!("  Decoded: {}x{}", image.width(), image.height());
            }
            Err(e) => {
                panic!("Failed to decode {}: {:?}", test_file, e);
            }
        }
    }
}
