pub mod font;

use eyre::{Result, eyre};
use femtovg::{Canvas, Paint, Path, Renderer};
use lopdf::Object;
use rustybuzz::ttf_parser::OutlineBuilder;

use crate::{Coord, DeviceScale, GraphicsState, RenderSettings, TextState, transform_from};

const TEXT_SCALE: f32 = 1000.;

fn tx(ts: &TextState, scale: &DeviceScale, Coord { x, y }: &Coord) -> Coord {
    transform_from(
        &Coord {
            x: (x + (ts.position as f32)) / TEXT_SCALE * ts.size,
            y: y / TEXT_SCALE * ts.size,
        },
        &ts.matrix,
        scale,
    )
}

struct FontPath<'a> {
    pub path: &'a mut Path,
    ts: TextState,
    scale: &'a DeviceScale,
}

impl OutlineBuilder for FontPath<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let xy = tx(&self.ts, &self.scale, &Coord { x, y });
        self.path.move_to(xy.x, xy.y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let xy = tx(&self.ts, &self.scale, &Coord { x, y });
        self.path.line_to(xy.x, xy.y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let xy1 = tx(&self.ts, &self.scale, &Coord { x: x1, y: y1 });
        let xy = tx(&self.ts, &self.scale, &Coord { x, y });
        self.path.quad_to(xy1.x, xy1.y, xy.x, xy.y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let xy1 = tx(&self.ts, &self.scale, &Coord { x: x1, y: y1 });
        let xy2 = tx(&self.ts, &self.scale, &Coord { x: x2, y: y2 });
        let xy = tx(&self.ts, &self.scale, &Coord { x, y });
        self.path.bezier_to(xy1.x, xy1.y, xy2.x, xy2.y, xy.x, xy.y);
        // self.bezier_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.path.close()
    }
}

pub fn draw_text<T: Renderer>(
    scale: &DeviceScale,
    canvas: &mut Canvas<T>,
    gs: &mut GraphicsState,
    glyphs: &[Object],
    render_settings: &RenderSettings,
) -> Result<()> {
    let ts = gs
        .text_state
        .as_mut()
        .ok_or_else(|| eyre!("no font state"))?;
    let font = ts.font.as_ref().ok_or_else(|| eyre!("no font sent"))?;
    eprintln!("{glyphs:?}");

    for glyph in glyphs {
        match glyph {
            Object::String(bytes, _) => {
                let glyph_ids = bytes
                    .chunks_exact(2)
                    .into_iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]));

                for glyph_idx in glyph_ids {
                    let glyph_id = rustybuzz::ttf_parser::GlyphId(glyph_idx);

                    let width: i64 = *font.widths.get(glyph_id.0 as usize).unwrap_or(&0);

                    let face = font.face();
                    let mut path = FontPath {
                        path: &mut Path::new(),
                        ts: ts.clone(),
                        scale,
                    };

                    match face.outline_glyph(glyph_id, &mut path) {
                        Some(_) => {
                            let color = gs.non_stroke_color;
                            ts.position += width;
                            canvas.fill_path(
                                &mut path.path,
                                &Paint::color(color).with_anti_alias(render_settings.anti_alias),
                            );
                        }
                        _ => (),
                    }
                }
            }
            // Object::Real(s) => ts.position += s,
            Object::Integer(i) => ts.position -= i,
            _ => (),
        }
    }

    Ok(())
}
