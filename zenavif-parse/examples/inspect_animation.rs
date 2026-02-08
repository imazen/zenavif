//! Inspect animated AVIF structure
#![allow(deprecated)]
use zenavif_parse::read_avif;
use std::env;
use std::fs::File;

fn main() {
    env_logger::init();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <avif-file>", args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    let mut f = File::open(path).expect("Failed to open file");

    match read_avif(&mut f) {
        Ok(avif) => {
            println!("File: {}", path);
            println!("Primary item size: {} bytes", avif.primary_item.len());

            if let Some(animation) = &avif.animation {
                println!("\n=== Animation ===");
                println!("Loop count: {}", animation.loop_count);
                println!("Number of frames: {}", animation.frames.len());

                if !animation.frames.is_empty() {
                    println!("\nFirst 5 frames:");
                    for (i, frame) in animation.frames.iter().take(5).enumerate() {
                        println!("  Frame {}: {} bytes, duration {} ms",
                            i, frame.data.len(), frame.duration_ms);
                    }

                    if animation.frames.len() > 5 {
                        println!("  ... ({} more frames)", animation.frames.len() - 5);
                    }

                    // Calculate total duration
                    let total_ms: u32 = animation.frames.iter().map(|f| f.duration_ms).sum();
                    println!("\nTotal duration: {} ms ({:.2} seconds)", total_ms, total_ms as f64 / 1000.0);
                }
            } else {
                println!("\nNo animation (static image)");
            }

            if avif.alpha_item.is_some() {
                println!("\nHas alpha: yes");
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}
