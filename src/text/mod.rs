pub mod font;

use eyre::{Result, eyre};
use kurbo::BezPath;
use lopdf::Object;
use owned_ttf_parser::{AsFaceRef, OutlineBuilder};
use peniko::Fill;
use vello::Scene;

use crate::{Coord, DeviceScale, GraphicsState, RenderSettings, TextState, transform_from};

const TEXT_SCALE: f32 = 1000.;

struct FontPath<'a> {
    pub path: &'a mut BezPath,
    units_per_em: u16,
    ts: TextState,
    scale: &'a DeviceScale,
}

impl<'a> FontPath<'a> {
    fn tx(&mut self, Coord { x, y }: &Coord) -> Coord {
        transform_from(
            &Coord {
                x: (x / self.units_per_em as f32 * self.ts.size)
                    + (self.ts.position / TEXT_SCALE * self.ts.size),
                y: (y) / self.units_per_em as f32 * self.ts.size,
            },
            &self.ts.matrix,
            self.scale,
        )
    }
}

impl OutlineBuilder for FontPath<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let xy = self.tx(&Coord { x, y });
        self.path.move_to((xy.x as f64, xy.y as f64));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let xy = self.tx(&Coord { x, y });
        self.path.line_to((xy.x as f64, xy.y as f64));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let xy1 = self.tx(&Coord { x: x1, y: y1 });
        let xy = self.tx(&Coord { x, y });
        self.path
            .quad_to((xy1.x as f64, xy1.y as f64), (xy.x as f64, xy.y as f64));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let xy1 = self.tx(&Coord { x: x1, y: y1 });
        let xy2 = self.tx(&Coord { x: x2, y: y2 });
        let xy = self.tx(&Coord { x, y });
        self.path.curve_to(
            (xy1.x as f64, xy1.y as f64),
            (xy2.x as f64, xy2.y as f64),
            (xy.x as f64, xy.y as f64),
        );
    }

    fn close(&mut self) {
        self.path.close_path()
    }
}

pub fn draw_text(
    scale: &DeviceScale,
    scene: &mut Scene,
    gs: &mut GraphicsState,
    glyphs: &[Object],
    _render_settings: &RenderSettings,
) -> Result<()> {
    let ts = gs
        .text_state
        .as_mut()
        .ok_or_else(|| eyre!("no font state"))?;
    let font = ts.font.as_ref().ok_or_else(|| eyre!("no font sent"))?;

    let units_per_em = font.font.as_face_ref().units_per_em();

    for glyph in glyphs {
        match glyph {
            Object::String(bytes, _) => {
                let glyph_ids = bytes
                    .chunks_exact(2)
                    .into_iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]));

                for glyph_idx in glyph_ids {
                    let glyph_id = owned_ttf_parser::GlyphId(glyph_idx);

                    let width: f32 = *font.widths.get(glyph_id.0 as usize).unwrap_or(&0.);
                    let mut path = FontPath {
                        path: &mut BezPath::new(),
                        units_per_em,
                        ts: ts.clone(),
                        scale,
                    };

                    match font.font.as_face_ref().outline_glyph(glyph_id, &mut path) {
                        Some(_) => {
                            use kurbo::Affine;
                            scene.fill(
                                Fill::EvenOdd,
                                Affine::IDENTITY,
                                gs.non_stroke_color,
                                None,
                                &*path.path,
                            );
                        }
                        _ => (),
                    }

                    ts.position += width;
                }
            }
            o => o.as_float().ok().iter().for_each(|s| ts.position -= s),
        }
    }

    Ok(())
}
