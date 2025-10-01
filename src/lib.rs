use std::{collections::HashMap, fmt::Debug, rc::Rc};

use eyre::{Result, bail, eyre};

pub mod offscreen;
pub mod text;

use kurbo::BezPath;
use lopdf::{Dictionary, Document, Object, ObjectId, content::Content};
use peniko::{Color, Fill};
pub use text::font::Font;
use vello::Scene;

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
pub struct TextState {
    pub position: f32,
    pub size: f32,
    pub matrix: CTM,
    pub font: Option<Rc<Font>>,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Coord {
    pub x: f32,
    pub y: f32,
}

pub fn transform_from(xy: &Coord, ctm: &CTM, scale: &DeviceScale) -> Coord {
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
pub struct GraphicsState {
    pub ctm: CTM,
    pub stroke_color: Color,
    pub non_stroke_color: Color,
    pub path: BezPath,
    pub text_state: Option<TextState>,
    pub line_width: f32,
    pub current_point: Coord,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Default::default(),
            stroke_color: Color::BLACK,
            non_stroke_color: Color::BLACK,
            path: BezPath::new(),
            text_state: None,
            line_width: 1.,
            current_point: Coord::default(),
        }
    }
}

#[derive(Debug)]
pub struct State {
    pub gs: GraphicsState,
    pub stack: Vec<GraphicsState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            gs: Default::default(),
            stack: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub anti_alias: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self { anti_alias: true }
    }
}

pub fn dimensions(page: &Dictionary) -> Result<(f32, f32)> {
    match &*page.get(b"MediaBox")?.as_array()?.clone() {
        [_, _, w, h] => Ok((w.as_float()?, h.as_float()?)),
        other => bail!("Expected [0 0 w h], but {:?}", other),
    }
}

pub struct DeviceScale {
    height: u32,
    scale: f32,
}

fn to_color(r: &Object, g: &Object, b: &Object) -> Result<Color> {
    Ok(Color::new([
        r.as_float()?,
        g.as_float()?,
        b.as_float()?,
        1.0,
    ]))
}

pub fn draw_doc(
    doc: &Document,
    scene: &mut Scene,
    width: u32,
    height: u32,
    page: u32,
    settings: &RenderSettings,
) -> Result<()> {
    let page_id = doc
        .get_pages()
        .get(&page)
        .ok_or_else(|| eyre!("No such page"))?
        .clone();
    let page_dict = doc.get_dictionary(page_id)?;
    let size: (f32, f32) = dimensions(page_dict)?;
    let scale = DeviceScale {
        height,
        scale: width as f32 / size.0,
    };

    let fonts = doc.get_page_fonts(page_id)?;
    let default_dict = Dictionary::default();
    let resource_dict = doc
        .get_dict_in_dict(page_dict, b"Resources")
        .unwrap_or(&default_dict);

    let ext_gstate_map: HashMap<Vec<u8>, Dictionary> = match resource_dict.get(b"ExtGState") {
        Ok(obj) => match obj.as_dict() {
            Ok(ext_gstate_dict) => ext_gstate_dict
                .iter()
                .filter_map(|(name, obj_ref)| {
                    obj_ref
                        .as_reference()
                        .ok()
                        .and_then(|id| doc.get_dictionary(id).ok())
                        .map(|dict| (name.clone(), dict.clone()))
                })
                .collect(),
            Err(_) => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    };

    let font_map: HashMap<Vec<u8>, Rc<Font>> = fonts
        .iter()
        .flat_map(|(font_id, font_obj)| {
            Font::from_pdf(doc, &Object::Dictionary((*font_obj).clone()))
                .ok()
                .map(|font| (font_id.clone(), Rc::new(font)))
        })
        .collect();

    let raw = doc.get_page_content(page_id)?;
    let content = Content::decode(&raw)?;

    let mut state = State::default();

    let transform = |state: &State, x: &Object, y: &Object| -> Result<Coord> {
        Ok(transform_from(
            &Coord {
                x: x.as_float()?,
                y: y.as_float()?,
            },
            &state.gs.ctm,
            &scale,
        ))
    };

    for op in content.operations {
        let o = op.operator.as_str();
        eprintln!("op: {:?} {:?}", o, &op.operands[..]);
        match (o, &op.operands[..]) {
            ("BT", []) => {
                state.gs.text_state = Some(TextState::default());
            }
            ("Tm", [a, b, c, d, e, f]) => {
                if let Some(ts) = &mut state.gs.text_state {
                    let tm_params = CTM {
                        a: a.as_float()?,
                        b: b.as_float()?,
                        c: c.as_float()?,
                        d: d.as_float()?,
                        e: e.as_float()?,
                        f: f.as_float()?,
                    };
                    ts.matrix = concat(&state.gs.ctm, &tm_params);
                    eprintln!("{:?}", ts.matrix);
                }
            }
            ("Tf", [Object::Name(n), size]) => {
                if let Some(font) = font_map.get(n) {
                    if let Some(ts) = &mut state.gs.text_state {
                        ts.font = Some(font.clone());
                        ts.size = size.as_float()?;
                    }
                }
            }

            ("TJ", [text]) => {
                text::draw_text(&scale, scene, &mut state.gs, text.as_array()?, &settings)?;
            }
            ("ET", []) => {
                state.gs.text_state = None;
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

                state.gs.ctm = concat(&state.gs.ctm, &ctm);
            }

            ("q", []) => {
                state.stack.push(state.gs.clone());
            }
            ("Q", []) => {
                state.gs = state.stack.pop().ok_or_else(|| {
                    eyre!("Popped empty graphics stack: unbalanced q/Q operators")
                })?;
            }
            ("scn", [r, g, b]) => {
                let next = to_color(r, g, b)?;
                state.gs.non_stroke_color = Color::new([
                    next.components[0],
                    next.components[1],
                    next.components[2],
                    state.gs.non_stroke_color.components[3],
                ]);
            }
            ("SCN", [r, g, b]) => {
                let next = to_color(r, g, b)?;
                state.gs.stroke_color = Color::new([
                    next.components[0],
                    next.components[1],
                    next.components[2],
                    state.gs.stroke_color.components[3],
                ]);
            }
            ("m", [x, y]) => {
                let xy = transform(&state, &x, y)?;
                state.gs.path.move_to((xy.x as f64, xy.y as f64));
                state.gs.current_point = xy;
            }
            ("l", [x, y]) => {
                let xy = transform(&state, &x, y)?;
                state.gs.path.line_to((xy.x as f64, xy.y as f64));
                state.gs.current_point = xy;
            }
            ("v", [x2, y2, x3, y3]) => {
                let xy1 = state.gs.current_point;
                let xy2 = transform(&state, &x2, &y2)?;
                let xy3 = transform(&state, &x3, &y3)?;
                state.gs.path.curve_to(
                    (xy1.x as f64, xy1.y as f64),
                    (xy2.x as f64, xy2.y as f64),
                    (xy3.x as f64, xy3.y as f64),
                );
                state.gs.current_point = xy3;
            }
            ("c", [x1, y1, x2, y2, x3, y3]) => {
                let xy1 = transform(&state, &x1, &y1)?;
                let xy2 = transform(&state, &x2, &y2)?;
                let xy3 = transform(&state, &x3, &y3)?;
                state.gs.path.curve_to(
                    (xy1.x as f64, xy1.y as f64),
                    (xy2.x as f64, xy2.y as f64),
                    (xy3.x as f64, xy3.y as f64),
                );
                state.gs.current_point = xy3;
            }
            ("re", [xo, yo, wo, ho]) => {
                let x = xo.as_float()?;
                let y = yo.as_float()?;
                let w = wo.as_float()?;
                let h = ho.as_float()?;
                let xy0 = transform_from(&Coord { x, y }, &state.gs.ctm, &scale);
                let xy1 = transform_from(&Coord { x: x + w, y: y + h }, &state.gs.ctm, &scale);
                let wh = Coord {
                    x: xy1.x - xy0.x,
                    y: xy1.y - xy0.y,
                };
                use kurbo::{Rect, Shape};
                let rect = Rect::new(
                    xy0.x as f64,
                    xy0.y as f64,
                    (xy0.x + wh.x) as f64,
                    (xy0.y + wh.y) as f64,
                );
                state.gs.path.extend(rect.path_elements(0.1));
            }
            ("h", []) => {
                state.gs.path.close_path();
            }
            ("f" | "f*", []) => {
                let fill_rule = if o == "f" {
                    Fill::NonZero
                } else {
                    Fill::EvenOdd
                };
                use kurbo::Affine;
                scene.fill(
                    fill_rule,
                    Affine::IDENTITY,
                    state.gs.non_stroke_color,
                    None,
                    &state.gs.path,
                );
                state.gs.path = BezPath::new();
            }
            ("w", [lw]) => {
                state.gs.line_width = lw.as_float()?;
            }
            ("S", []) => {
                use kurbo::Affine;
                use peniko::kurbo::Stroke;
                let stroke = Stroke::new(state.gs.line_width as f64 * scale.scale as f64);
                scene.stroke(
                    &stroke,
                    Affine::IDENTITY,
                    state.gs.stroke_color,
                    None,
                    &state.gs.path,
                );
                state.gs.path = BezPath::new();
            }
            ("B", []) => {
                use kurbo::Affine;
                use peniko::kurbo::Stroke;
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    state.gs.non_stroke_color,
                    None,
                    &state.gs.path,
                );
                let stroke = Stroke::new(state.gs.line_width as f64 * scale.scale as f64);
                scene.stroke(
                    &stroke,
                    Affine::IDENTITY,
                    state.gs.stroke_color,
                    None,
                    &state.gs.path,
                );
                state.gs.path = BezPath::new();
            }
            ("gs", [Object::Name(name)]) => {
                if let Some(gstate_dict) = ext_gstate_map.get(name) {
                    if let Some(ca) = gstate_dict.get(b"ca").and_then(|ca| ca.as_float()).ok() {
                        let c = state.gs.non_stroke_color;
                        state.gs.non_stroke_color =
                            Color::new([c.components[0], c.components[1], c.components[2], ca]);
                    }
                    if let Some(ca) = gstate_dict.get(b"CA").and_then(|ca| ca.as_float()).ok() {
                        let c = state.gs.stroke_color;
                        state.gs.stroke_color =
                            Color::new([c.components[0], c.components[1], c.components[2], ca]);
                    }
                }
            }

            (_o, _a) => {
                // eprintln!("op: {:?} {:?}", _o, _a);
            }
        }
    }

    Ok(())
}
