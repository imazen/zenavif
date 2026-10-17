//! Print the structural inventory of AVIF files: every box, item, item
//! extent, animation sample and property, with what the zencodec decode does
//! with it.
//!
//! ```text
//! cargo run --example inventory -- FILE...
//! ```

use zencodec::decode::{DecodeJob, DecoderConfig};

fn main() {
    let mut failed = false;
    for path in std::env::args().skip(1) {
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{path}: {e}");
                failed = true;
                continue;
            }
        };
        match zenavif::AvifDecoderConfig::new().job().inventory(&data) {
            Ok(inv) => {
                println!("== {path}");
                print!("{inv}");
                if let Err(e) = inv.validate() {
                    println!("INVALID: {e}");
                    failed = true;
                }
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
