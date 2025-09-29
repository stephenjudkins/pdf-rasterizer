use eyre::{Result, eyre};
use femtovg::{
    Canvas, Color, ImageFlags, ImageSource, Paint, Path, Renderer,
    img::{DynamicImage, Rgba},
};
use lopdf::Object;
use rusttype::Scale;

use crate::{Coord, DeviceScale, GraphicsState, transform_from};

const TEXT_SCALE: f32 = 1000.;

pub fn draw_text<T: Renderer>(
    scale: &DeviceScale,
    canvas: &mut Canvas<T>,
    gs: &mut GraphicsState,
    glyphs: &[Object],
) -> Result<()> {
    let ts = gs
        .text_state
        .as_mut()
        .ok_or_else(|| eyre!("no font state"))?;
    let font = ts.font.as_ref().ok_or_else(|| eyre!("no font sent"))?;

    for glyph in glyphs {
        match glyph {
            Object::String(bytes, _) => {
                let glyph_ids = bytes
                    .chunks_exact(2)
                    .into_iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]));

                for glyph_id in glyph_ids {
                    eprintln!("{glyph_id:?}");
                    let Coord { x, y } = transform_from(
                        &Coord {
                            x: ts.position / TEXT_SCALE * ts.size,
                            y: 0.,
                        },
                        &ts.matrix,
                        scale,
                    );

                    let width: f32 = *font.widths.get(glyph_id as usize).unwrap_or(&0.);
                    ts.position += width;

                    let glyph = font
                        .font
                        .glyph(rusttype::GlyphId(glyph_id))
                        .scaled(Scale::uniform(ts.size * scale.scale));

                    let positioned = glyph.positioned(rusttype::point(0., 0.));

                    match positioned.pixel_bounding_box() {
                        Some(metrics) => {
                            let mut image = DynamicImage::new_rgba8(
                                metrics.width() as u32,
                                metrics.height() as u32,
                            )
                            .to_rgba8();
                            let Color { r, g, b, a: _ } = gs.non_stroke_color;
                            positioned.draw(|x, y, v| {
                                image.put_pixel(
                                    x as u32,
                                    y as u32,
                                    Rgba([
                                        (r * 255.) as u8,
                                        (g * 255.) as u8,
                                        (b * 255.) as u8,
                                        (v * 255.) as u8,
                                    ]),
                                )
                            });

                            let w = metrics.width() as f32;
                            let h = metrics.height() as f32;
                            let x0 = x + (metrics.min.x as f32);
                            let y0 = y + (metrics.min.y as f32);
                            let image_id = canvas.create_image(
                                ImageSource::try_from(&DynamicImage::from(image))?,
                                ImageFlags::REPEAT_Y,
                            )?;
                            let img_paint = Paint::image(
                                image_id,
                                x0,
                                y0,
                                metrics.width() as f32,
                                metrics.height() as f32,
                                0.0,
                                1.0,
                            );
                            let mut path = Path::new();
                            path.rect(x0, y0, w, h);
                            canvas.fill_path(&path, &img_paint);
                        }
                        _ => (),
                    }
                }
            }
            Object::Real(s) => ts.position -= s,
            _ => (),
        }
    }

    Ok(())
}
