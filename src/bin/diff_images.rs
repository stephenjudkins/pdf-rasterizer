use eyre::{Result, WrapErr};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::{env, process::ExitCode};

fn compare_images(actual_path: &str, expected_path: &str, diff_path: &str) -> Result<()> {
    println!("Loading images...");
    let actual_img = image::open(actual_path)
        .wrap_err_with(|| format!("Failed to open {}", actual_path))?
        .to_rgba8();
    let expected_img = image::open(expected_path)
        .wrap_err_with(|| format!("Failed to open {}", expected_path))?
        .to_rgba8();

    let (actual_width, actual_height) = actual_img.dimensions();
    let (expected_width, expected_height) = expected_img.dimensions();

    if actual_width != expected_width || actual_height != expected_height {
        println!("Image dimensions differ:");
        println!("  {}: {}x{}", actual_path, actual_width, actual_height);
        println!(
            "  {}: {}x{}",
            expected_path, expected_width, expected_height
        );
        return Ok(());
    }

    println!("Both images are {}x{}", actual_width, actual_height);

    let mut diff_img: RgbaImage = ImageBuffer::new(actual_width, actual_height);
    let mut total_diff = 0u64;
    let mut max_diff = 0u8;
    let mut different_pixels = 0u32;

    for (x, y, actual_pixel) in actual_img.enumerate_pixels() {
        let expected_pixel = expected_img.get_pixel(x, y);

        let r_diff = (actual_pixel[0] as i16 - expected_pixel[0] as i16).abs() as u8;
        let g_diff = (actual_pixel[1] as i16 - expected_pixel[1] as i16).abs() as u8;
        let b_diff = (actual_pixel[2] as i16 - expected_pixel[2] as i16).abs() as u8;

        let pixel_diff = r_diff.max(g_diff).max(b_diff);
        max_diff = max_diff.max(pixel_diff);
        total_diff += pixel_diff as u64;

        if pixel_diff > 0 {
            different_pixels += 1;
        }

        // Scale difference for visibility (multiply by 5 to make differences more apparent)
        let scaled_diff = (pixel_diff as u16 * 5).min(255) as u8;

        diff_img.put_pixel(x, y, Rgba([scaled_diff, scaled_diff, scaled_diff, 255]));
    }

    diff_img
        .save(diff_path)
        .wrap_err_with(|| format!("Failed to save {}", diff_path))?;

    let total_pixels = actual_width * actual_height;
    let avg_diff = total_diff as f64 / total_pixels as f64;
    let percent_different = (different_pixels as f64 / total_pixels as f64) * 100.0;

    println!("\nImage comparison results:");
    println!("  Dimensions: {}x{}", actual_width, actual_height);
    println!("  Total pixels: {}", total_pixels);
    println!(
        "  Different pixels: {} ({:.2}%)",
        different_pixels, percent_different
    );
    println!("  Average per-pixel difference: {:.2}", avg_diff);
    println!("  Maximum per-pixel difference: {}", max_diff);
    println!("  Difference image saved as {}", diff_path);

    if different_pixels == 0 {
        println!("\n✅ Images are identical!");
    } else if percent_different < 1.0 {
        println!("\n⚠️  Images have minor differences");
    } else {
        println!("\n❌ Images have significant differences");
    }

    Ok(())
}

fn main() -> Result<ExitCode> {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        3 => {
            let actual_path = &args[1];
            let expected_path = &args[2];
            compare_images(actual_path, expected_path, "difference.png")?;
            Ok(ExitCode::SUCCESS)
        }
        4 => {
            let actual_path = &args[1];
            let expected_path = &args[2];
            let diff_path = &args[3];
            compare_images(actual_path, expected_path, diff_path)?;
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!(
                "Usage: {} <actual.png> <expected.png> [difference.png]",
                args[0]
            );
            eprintln!("If difference output is not specified, defaults to 'difference.png'");
            Ok(ExitCode::FAILURE)
        }
    }
}
