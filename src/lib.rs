use core::fmt;
use std::{collections::HashMap, fmt::Debug, rc::Rc};

use eyre::{Result, bail, eyre};
use femtovg::{
    Canvas, ImageFlags, ImageSource, Paint, Path, Renderer,
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

        let font = rusttype::Font::try_from_vec(content).ok_or(eyre!("could not load font"))?;

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

impl Default for TextMatrix {
    fn default() -> Self {
        Self {
            a: 1,
            b: 0,
            c: 0,
            d: 1,
            e: 0,
            f: 0,
        }
    }
}

#[derive(Default, Debug)]
pub struct TextState<'a> {
    pub position: f32,
    pub size: f32,
    pub matrix: TextMatrix,
    pub font: Option<Rc<Font<'a>>>,
}

#[derive(Clone, Copy)]
pub struct Coord {
    pub x: f32,
    pub y: f32,
}

pub fn transform(xy: &Coord, ctm: &CTM) -> Coord {
    let CTM { a, b, c, d, e, f } = ctm;
    let Coord { x, y } = xy;

    Coord {
        x: a * x + c * y + e,
        y: b * x + d * y + f,
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

#[derive(Clone, Debug, Default)]
pub struct GraphicsState {
    pub ctm: CTM,
}

#[derive(Debug)]
pub struct State<'a> {
    pub graphics_state: Vec<GraphicsState>,
    pub text_state: Option<TextState<'a>>,
}

impl Default for State<'_> {
    fn default() -> Self {
        Self {
            graphics_state: vec![Default::default()],
            text_state: Default::default(),
        }
    }
}

const TEXT_SCALE: f32 = 1000.;

pub fn draw_text<T: Renderer>(
    scale: f32,
    canvas: &mut Canvas<T>,
    ts: &mut TextState,
    gs: &mut &GraphicsState,
    glyphs: &[Object],
) -> Result<()> {
    let font = ts.font.as_ref().ok_or(eyre!("no font sent"))?;

    for glyph in glyphs {
        match glyph {
            Object::String(bytes, _) => {
                let glyph_ids: Vec<u16> = bytes
                    .chunks_exact(2)
                    .into_iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();

                for glyph_id in glyph_ids {
                    let Coord { x, y } = transform(
                        &Coord {
                            x: ts.position / TEXT_SCALE * ts.size,
                            y: 0.,
                        },
                        &gs.ctm,
                    );
                    let width: f32 = *font.widths.get(glyph_id as usize).unwrap_or(&0.);
                    ts.position += width;

                    let glyph = font
                        .font
                        .glyph(rusttype::GlyphId(glyph_id))
                        .scaled(Scale::uniform(ts.size * scale));
                    let positioned = glyph.positioned(rusttype::point(0., 0.));

                    match positioned.pixel_bounding_box() {
                        Some(metrics) => {
                            let mut image = DynamicImage::new_rgba8(
                                metrics.width() as u32,
                                metrics.height() as u32,
                            )
                            .to_rgba8();

                            positioned.draw(|x, y, v| {
                                let p = 255 - ((v * 255.0) as u8);
                                image.put_pixel(x as u32, y as u32, Rgba([p, p, p, 255 - p]))
                            });

                            let w = metrics.width() as f32;
                            let h = metrics.height() as f32;
                            let x0 = x * scale + (metrics.min.x as f32);
                            let y0 = 0.0 - (y * scale) + (metrics.min.y as f32);
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

    canvas.reset();

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

pub fn draw_doc<T: Renderer>(doc: &Document, canvas: &mut Canvas<T>, page: u32) -> Result<()> {
    let page_id = doc
        .get_pages()
        .get(&page)
        .ok_or(eyre!("No such page"))?
        .clone();
    let size: (u32, u32) = dimensions(doc.get_dictionary(page_id)?)?;
    let scale = canvas.width() as f32 / size.0 as f32;
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

    for op in content.operations {
        match (op.operator.as_str(), &op.operands[..]) {
            ("BT", []) => {
                state.text_state = Some(TextState::default());
            }
            (
                "Tm",
                [
                    Object::Integer(a),
                    Object::Integer(b),
                    Object::Integer(c),
                    Object::Integer(d),
                    Object::Integer(e),
                    Object::Integer(f),
                ],
            ) => match &mut state.text_state {
                Some(ts) => {
                    ts.matrix = TextMatrix {
                        a: *a,
                        b: *b,
                        c: *c,
                        d: *d,
                        e: *e,
                        f: *f,
                    }
                }
                _ => (),
            },
            ("Tf", [Object::Name(n), size]) => match font_map.get(n) {
                Some(font) => match &mut state.text_state {
                    Some(ts) => {
                        ts.font = Some(font.clone());
                        ts.size = size.as_float().unwrap();
                    }
                    _ => (),
                },
                None => (),
            },
            ("TJ", [text]) => match &mut state.text_state {
                Some(ts) => match &mut state.graphics_state.last() {
                    Some(gs) => {
                        draw_text(scale, canvas, ts, gs, text.as_array()?)?;
                    }
                    _ => (),
                },
                _ => (),
            },
            ("ET", []) => {
                state.text_state = None;
            }
            ("cm", [a0, b0, c0, d0, e0, f0]) => {
                let ctm = CTM {
                    a: a0.as_float().unwrap(),
                    b: b0.as_float().unwrap(),
                    c: c0.as_float().unwrap(),
                    d: d0.as_float().unwrap(),
                    e: e0.as_float().unwrap(),
                    f: f0.as_float().unwrap(),
                };

                let gs = state
                    .graphics_state
                    .last_mut()
                    .ok_or(eyre!("empty graphics stack"))?;

                let next = concat(&gs.ctm, &ctm);
                gs.ctm = next
            }
            ("q", []) => {
                let gs = state.graphics_state.last().cloned().unwrap_or_default();
                state.graphics_state.push(gs);
            }
            ("Q", []) => {
                state
                    .graphics_state
                    .pop()
                    .ok_or(eyre!("Popped empty graphics stack"))?;
            }
            _ => (),
        }
    }

    Ok(())
}
