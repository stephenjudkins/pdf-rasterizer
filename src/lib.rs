use core::fmt;
use std::{collections::HashMap, fmt::Debug, rc::Rc};

use eyre::{Result, bail, eyre};
use femtovg::{
    Canvas, Color, FillRule, ImageFlags, ImageSource, Paint, Path, Renderer,
    img::{DynamicImage, Rgba},
};
use lopdf::{Dictionary, Document, Object, ObjectId, content::Content};
use rusttype::Scale;

fn get<A: FromPDF>(doc: &Document, root: &Object) -> Result<A> {
    A::from_pdf(doc, root)
}

pub trait FromPDF: Sized {
    fn from_pdf(doc: &Document, root: &Object) -> Result<Self>;
}

impl FromPDF for Vec<u8> {
    fn from_pdf(doc: &Document, root: &Object) -> Result<Self> {
        Ok(doc
            .get_object(root.as_reference()?)?
            .as_stream()?
            .decompressed_content()?)
    }
}

impl FromPDF for ObjectId {
    fn from_pdf(_: &Document, root: &Object) -> Result<Self> {
        Ok(root.as_reference()?)
    }
}

impl FromPDF for String {
    fn from_pdf(_: &Document, root: &Object) -> Result<Self> {
        Ok(String::from_utf8(root.as_name()?.to_vec())?)
    }
}

impl<A: FromPDF> FromPDF for Vec<A> {
    fn from_pdf(doc: &Document, root: &Object) -> Result<Self> {
        match root {
            Object::Array(objs) => objs.iter().map(|o| A::from_pdf(doc, o)).collect(),
            _ => Err(eyre!("expected Array")),
        }
    }
}

impl FromPDF for f32 {
    fn from_pdf(_: &Document, root: &Object) -> Result<Self> {
        match root {
            Object::Real(i) => Ok(*i),
            Object::Integer(i) => Ok(*i as f32),
            _ => bail!("not a number"),
        }
    }
}

impl FromPDF for i64 {
    fn from_pdf(_: &Document, root: &Object) -> Result<Self> {
        Ok(root.as_i64()?)
    }
}

impl<'a> FromPDF for Font<'a> {
    fn from_pdf(doc: &Document, root: &Object) -> Result<Self> {
        let font = root.as_dict()?;
        let descendant_fonts: Vec<ObjectId> = get(doc, font.get(b"DescendantFonts")?)?;
        let descendent_font = doc.get_dictionary(match descendant_fonts[..] {
            [id] => id,
            _ => Err(eyre!("expected one DescendantFont"))?,
        })?;
        let descriptor =
            doc.get_dictionary(descendent_font.get(b"FontDescriptor")?.as_reference()?)?;

        let widths: Vec<f32> = match &descendent_font.get(b"W")?.as_array()?[..] {
            [Object::Integer(0), Object::Array(ws)] => get(doc, &Object::Array(ws.clone()))?,
            _ => bail!("Expected [0 [widths..]]"),
        };

        let content: Vec<u8> = get(doc, descriptor.get(b"FontFile2")?)?;

        let font =
            rusttype::Font::try_from_vec(content).ok_or_else(|| eyre!("could not load font"))?;

        let name = get(doc, descriptor.get(b"FontName")?)?;

        Ok(Font { name, font, widths })
    }
}

pub struct Font<'a> {
    pub name: String,
    pub font: rusttype::Font<'a>,
    pub widths: Vec<f32>,
}

impl fmt::Debug for Font<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font").field("name", &self.name).finish()
    }
}

#[derive(Debug)]
pub struct TextMatrix {
    pub a: i64,
    pub b: i64,
    pub c: i64,
    pub d: i64,
    pub e: i64,
    pub f: i64,
}

#[derive(Default, Debug, Clone)]
pub struct TextState<'a> {
    pub position: f32,
    pub size: f32,
    pub matrix: CTM,
    pub font: Option<Rc<Font<'a>>>,
}

#[derive(Clone, Copy)]
pub struct Coord {
    pub x: f32,
    pub y: f32,
}

pub fn transform(xy: &Coord, ctm: &CTM, scale: &DeviceScale) -> Coord {
    let CTM { a, b, c, d, e, f } = ctm;
    let Coord { x, y } = xy;

    Coord {
        x: scale.scale * (a * x + c * y + e),
        y: scale.height as f32 - (scale.scale * (b * x + d * y + f)),
    }
}

pub fn concat(m1: &CTM, m2: &CTM) -> CTM {
    CTM {
        a: m1.a * m2.a + m1.c * m2.b,
        b: m1.b * m2.a + m1.d * m2.b,
        c: m1.a * m2.c + m1.c * m2.d,
        d: m1.b * m2.c + m1.d * m2.d,
        e: m1.a * m2.e + m1.b * m2.f + m1.e,
        f: m1.b * m2.e + m1.d * m2.f + m1.f,
    }
}

#[derive(Clone)]
pub struct CTM {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for CTM {
    fn default() -> Self {
        Self {
            a: 1.,
            b: 0.,
            c: 0.,
            d: 1.,
            e: 0.,
            f: 0.,
        }
    }
}

impl Debug for CTM {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            format!(
                "[{} {} {} {} {} {}]",
                self.a, self.b, self.c, self.d, self.e, self.f
            )
            .as_str(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct GraphicsState<'a> {
    pub ctm: CTM,
    pub stroke_color: Color,
    pub non_stroke_color: Color,
    pub path: Path,
    pub text_state: Option<TextState<'a>>,
    pub line_width: f32,
}

impl Default for GraphicsState<'_> {
    fn default() -> Self {
        Self {
            ctm: Default::default(),
            stroke_color: Color::black(),
            non_stroke_color: Color::black(),
            path: Path::new(),
            text_state: None,
            line_width: 1.,
        }
    }
}

#[derive(Debug)]
pub struct State<'a> {
    pub graphics_state: Vec<GraphicsState<'a>>,
}

impl Default for State<'_> {
    fn default() -> Self {
        Self {
            graphics_state: vec![Default::default()],
        }
    }
}

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
                    let Coord { x, y } = transform(
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

pub fn dimensions(page: &Dictionary) -> Result<(u32, u32)> {
    match page.get(b"MediaBox")?.as_array()?.as_slice() {
        &[
            Object::Integer(x),
            Object::Integer(y),
            Object::Integer(w),
            Object::Integer(h),
        ] if x == 0 && y == 0 => match (w.try_into(), h.try_into()) {
            (Ok(w), Ok(h)) => Ok((w, h)),
            (w, h) => bail!("Expected w, h > 0, got {:?}", (w, h)),
        },
        other => bail!("Expected [0 0 w h], but {:?}", other),
    }
}

fn alter_gfx_state<F, A>(state: &mut State, mut f: F) -> Result<A>
where
    F: FnMut(&mut GraphicsState) -> Result<A>,
{
    let gs = state
        .graphics_state
        .last_mut()
        .ok_or(eyre!("empty graphics stack"))?;

    f(gs)
}

pub struct DeviceScale {
    height: u32,
    scale: f32,
}

fn to_color(r: &Object, g: &Object, b: &Object) -> Result<Color> {
    Ok(Color::rgbf(r.as_float()?, g.as_float()?, b.as_float()?))
}

pub fn draw_doc<T: Renderer>(doc: &Document, canvas: &mut Canvas<T>, page: u32) -> Result<()> {
    let page_id = doc
        .get_pages()
        .get(&page)
        .ok_or_else(|| eyre!("No such page"))?
        .clone();
    let size: (u32, u32) = dimensions(doc.get_dictionary(page_id)?)?;
    let scale = DeviceScale {
        height: canvas.height(),
        scale: canvas.width() as f32 / size.0 as f32,
    };

    let fonts = doc.get_page_fonts(page_id)?;

    let font_map_result: Result<HashMap<Vec<u8>, Rc<Font>>> = fonts
        .iter()
        .map(|(font_id, font_obj)| {
            let font = Font::from_pdf(doc, &Object::Dictionary((*font_obj).clone()))?;
            Ok((font_id.clone(), Rc::new(font)))
        })
        .collect();

    let font_map = font_map_result?;

    let raw = doc.get_page_content(page_id)?;
    let content = Content::decode(&raw)?;

    let mut state = State::default();

    let mut tp = Path::new();
    tp.move_to(100., 100.);
    tp.line_to(200., 200.);
    canvas.stroke_path(&tp, &Paint::color(Color::black()));

    for op in content.operations {
        let o = op.operator.as_str();
        match (o, &op.operands[..]) {
            ("BT", []) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.text_state = Some(TextState::default());
                    Ok(())
                })?;
            }
            ("Tm", [a, b, c, d, e, f]) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.text_state = Some(TextState::default());
                    match &mut gs.text_state {
                        Some(ts) => {
                            let tm = CTM {
                                a: a.as_float()?,
                                b: b.as_float()?,
                                c: c.as_float()?,
                                d: d.as_float()?,
                                e: e.as_float()?,
                                f: f.as_float()?,
                            };

                            ts.matrix = concat(&gs.ctm, &tm);
                        }
                        _ => (),
                    }

                    Ok(())
                })?;
            }
            ("Tf", [Object::Name(n), size]) => alter_gfx_state(&mut state, |gs| {
                match font_map.get(n) {
                    Some(font) => match &mut gs.text_state {
                        Some(ts) => {
                            ts.font = Some(font.clone());
                            ts.size = size.as_float().unwrap();
                        }
                        _ => (),
                    },
                    None => (),
                }
                Ok(())
            })?,

            ("TJ", [text]) => alter_gfx_state(&mut state, |gs| {
                draw_text(&scale, canvas, gs, text.as_array()?)?;
                Ok(())
            })?,
            ("ET", []) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.text_state = None;

                    Ok(())
                })?;
            }
            ("cm", [a, b, c, d, e, f]) => {
                let ctm = CTM {
                    a: a.as_float()?,
                    b: b.as_float()?,
                    c: c.as_float()?,
                    d: d.as_float()?,
                    e: e.as_float()?,
                    f: f.as_float()?,
                };

                alter_gfx_state(&mut state, |gs| {
                    gs.ctm = concat(&gs.ctm, &ctm);
                    Ok(())
                })?;
            }

            ("q", []) => {
                let gs = state.graphics_state.last().cloned().unwrap_or_default();
                state.graphics_state.push(gs);
            }
            ("Q", []) => {
                state
                    .graphics_state
                    .pop()
                    .ok_or_else(|| eyre!("Popped empty graphics stack"))?;
            }
            ("scn", [r, g, b]) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.non_stroke_color = to_color(r, g, b)?;
                    Ok(())
                })?;
            }
            ("SCN", [r, g, b]) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.stroke_color = to_color(r, g, b)?;
                    Ok(())
                })?;
            }
            ("m", [x, y]) => {
                alter_gfx_state(&mut state, |gs| {
                    let xy = transform(
                        &Coord {
                            x: x.as_float()?,
                            y: y.as_float()?,
                        },
                        &gs.ctm,
                        &scale,
                    );
                    gs.path.move_to(xy.x, xy.y);
                    Ok(())
                })?;
            }
            ("l", [x, y]) => {
                alter_gfx_state(&mut state, |gs| {
                    let xy = transform(
                        &Coord {
                            x: x.as_float()?,
                            y: y.as_float()?,
                        },
                        &gs.ctm,
                        &scale,
                    );
                    gs.path.line_to(xy.x, xy.y);
                    Ok(())
                })?;
            }
            ("c", [x1, y1, x2, y2, x3, y3]) => {
                alter_gfx_state(&mut state, |gs| {
                    let xy1 = transform(
                        &Coord {
                            x: x1.as_float()?,
                            y: y1.as_float()?,
                        },
                        &gs.ctm,
                        &scale,
                    );
                    let xy2 = transform(
                        &Coord {
                            x: x2.as_float()?,
                            y: y2.as_float()?,
                        },
                        &gs.ctm,
                        &scale,
                    );
                    let xy3 = transform(
                        &Coord {
                            x: x3.as_float()?,
                            y: y3.as_float()?,
                        },
                        &gs.ctm,
                        &scale,
                    );
                    gs.path.bezier_to(xy1.x, xy1.y, xy2.x, xy2.y, xy3.x, xy3.y);
                    Ok(())
                })?;
            }
            ("re", [xo, yo, wo, ho]) => {
                alter_gfx_state(&mut state, |gs| {
                    let x = xo.as_float()?;
                    let y = yo.as_float()?;
                    let w = wo.as_float()?;
                    let h = ho.as_float()?;
                    let xy0 = transform(&Coord { x, y }, &gs.ctm, &scale);
                    let xy1 = transform(&Coord { x: x + w, y: y + h }, &gs.ctm, &scale);
                    let wh = Coord {
                        x: xy1.x - xy0.x,
                        y: xy1.y - xy0.y,
                    };
                    gs.path.rect(xy0.x, xy0.y, wh.x, wh.y);
                    Ok(())
                })?;
            }
            ("h", []) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.path.close();
                    Ok(())
                })?;
            }
            ("f" | "f*", []) => {
                alter_gfx_state(&mut state, |gs| {
                    let fill_rule = if o == "f" {
                        FillRule::NonZero
                    } else {
                        FillRule::EvenOdd
                    };
                    // eprintln!("{:?}", &gs.path);
                    canvas.fill_path(
                        &gs.path,
                        &Paint::color(gs.non_stroke_color).with_fill_rule(fill_rule),
                    );
                    gs.path = Path::new();
                    Ok(())
                })?;
            }
            ("w", [lw]) => {
                alter_gfx_state(&mut state, |gs| {
                    gs.line_width = lw.as_float()?;
                    Ok(())
                })?;
            }
            ("S", []) => {
                alter_gfx_state(&mut state, |gs| {
                    canvas.stroke_path(
                        &gs.path,
                        &Paint::color(gs.stroke_color).with_line_width(gs.line_width * scale.scale),
                    );
                    gs.path = Path::new();
                    Ok(())
                })?;
            }

            (o, a) => {
                eprintln!("op: {:?} {:?}", o, a);
            }
        }
    }

    Ok(())
}
