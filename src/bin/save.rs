use eyre::{Result, WrapErr, eyre};
use std::fs;
use std::{env, process::ExitCode};

use lopdf::Document;
use rasterizer::offscreen::pdf_to_rgba_image;

const PAGE: u32 = 1;
const DEFAULT_SCALE: f32 = 2.0;

async fn save_pdf_to_png(pdf_path: &str, output_path: &str) -> Result<()> {
    let bytes =
        fs::read(pdf_path).wrap_err_with(|| eyre!("Failed to read PDF file: {}", pdf_path))?;
    let doc = Document::load_mem(&bytes).wrap_err("Failed to parse PDF document")?;

    let image = pdf_to_rgba_image(&doc, PAGE, DEFAULT_SCALE).await?;

    image
        .save(output_path)
        .wrap_err_with(|| eyre!("Failed to save PNG file: {}", output_path))?;

    Ok(())
}

fn main() -> Result<ExitCode> {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        2 => {
            let pdf_path = &args[1];
            let output_path = "out.png";
            pollster::block_on(save_pdf_to_png(pdf_path, output_path))?;
            Ok(ExitCode::SUCCESS)
        }
        3 => {
            let pdf_path = &args[1];
            let output_path = &args[2];
            pollster::block_on(save_pdf_to_png(pdf_path, output_path))?;
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("Usage: {} <pdf_file> [output.png]", args[0]);
            eprintln!("If output file is not specified, defaults to 'out.png'");
            Ok(ExitCode::FAILURE)
        }
    }
}
