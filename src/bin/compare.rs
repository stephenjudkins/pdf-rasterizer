use eyre::{Result, WrapErr, eyre};
use image::{ImageBuffer, Rgba, RgbaImage};
use std::fs;
use std::{env, process::ExitCode};

use lopdf::Document;
use pdfium_render::prelude::*;
use rasterizer::offscreen::pdf_to_rgba_image;

const PAGE: u32 = 1;
const DEFAULT_SCALE: f32 = 2.0;

async fn compare_pdf_renderers(pdf_path: &str) -> Result<()> {
    let bytes =
        fs::read(pdf_path).wrap_err_with(|| eyre!("Failed to read PDF file: {}", pdf_path))?;

    // Render with our rasterizer
    let doc = Document::load_mem(&bytes).wrap_err("Failed to parse PDF document")?;
    let our_image = pdf_to_rgba_image(&doc, PAGE, DEFAULT_SCALE).await?;
    our_image
        .save("actual.png")
        .wrap_err("Failed to save actual.png")?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
            .wrap_err("Failed to bind to pdfium library")?,
    );

    let document = pdfium
        .load_pdf_from_byte_slice(&bytes, None)
        .wrap_err("Failed to load PDF with pdfium")?;

    let page = document
        .pages()
        .get(0)
        .wrap_err("Failed to get first page from pdfium document")?;

    let width = (page.width().value * DEFAULT_SCALE) as u32;
    let height = (page.height().value * DEFAULT_SCALE) as u32;

    let render_config = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_target_height(height as i32)
        .set_maximum_width(width as i32)
        .set_maximum_height(height as i32)
        .set_path_smoothing(false)
        .set_image_smoothing(false)
        .set_text_smoothing(false)
        .set_format(PdfBitmapFormat::BGRx)
        .disable_native_text_rendering(true);

    let pdfium_image = page
        .render_with_config(&render_config)
        .wrap_err("Failed to render page with pdfium")?
        .as_image()
        .to_rgba8();

    pdfium_image
        .save("expected.png")
        .wrap_err("Failed to save expected.png")?;
    compare_images(&our_image, &pdfium_image)?;
    Ok(())
}

fn compare_images(actual_img: &RgbaImage, expected_img: &RgbaImage) -> Result<()> {
    let (actual_width, actual_height) = actual_img.dimensions();
    let (expected_width, expected_height) = expected_img.dimensions();

    if actual_width != expected_width || actual_height != expected_height {
        return Ok(());
    }

    let mut diff_img: RgbaImage = ImageBuffer::new(actual_width, actual_height);
    let mut total_diff = 0u64;
    let mut max_diff = 0u8;

    for (x, y, actual_pixel) in actual_img.enumerate_pixels() {
        let expected_pixel = expected_img.get_pixel(x, y);

        let r_diff = (actual_pixel[0] as i16 - expected_pixel[0] as i16).abs() as u8;
        let g_diff = (actual_pixel[1] as i16 - expected_pixel[1] as i16).abs() as u8;
        let b_diff = (actual_pixel[2] as i16 - expected_pixel[2] as i16).abs() as u8;

        let pixel_diff = r_diff.max(g_diff).max(b_diff);
        max_diff = max_diff.max(pixel_diff);
        total_diff += pixel_diff as u64;

        // Scale difference for visibility (multiply by 3 to make differences more apparent)
        let scaled_diff = (pixel_diff as u16 * 3).min(255) as u8;

        diff_img.put_pixel(x, y, Rgba([scaled_diff, scaled_diff, scaled_diff, 255]));
    }

    diff_img
        .save("difference.png")
        .wrap_err("Failed to save difference.png")?;

    Ok(())
}

fn main() -> Result<ExitCode> {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        2 => {
            let pdf_path = &args[1];
            pollster::block_on(compare_pdf_renderers(pdf_path))?;
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("Usage: {} <pdf_file>", args[0]);
            Ok(ExitCode::FAILURE)
        }
    }
}
