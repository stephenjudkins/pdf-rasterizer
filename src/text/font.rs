use eyre::{Result, bail, eyre};
use lopdf::{Document, Object, ObjectId};
use owned_ttf_parser::{AsFaceRef, OwnedFace};
use std::fmt;

use crate::{FromPDF, get};

pub struct Font {
    pub name: String,
    pub font: OwnedFace,
    pub widths: Vec<f32>,
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font").field("name", &self.name).finish()
    }
}

impl<'a> FromPDF for Font {
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
            [Object::Integer(0), Object::Array(ws)] => ws
                .iter()
                .map(|i| i.as_float().map_err(|e| eyre!("{e:?}")))
                .collect::<Result<Vec<_>>>()?,
            _ => bail!("Expected [0 [widths..]]"),
        };

        let content: Vec<u8> = get(doc, descriptor.get(b"FontFile2")?)?;

        let font = load_font(content)?;

        let name = get(doc, descriptor.get(b"FontName")?)?;

        Ok(Font { name, font, widths })
    }
}

pub fn load_font(data: Vec<u8>) -> Result<OwnedFace> {
    let o = OwnedFace::from_vec(data, 0).map_err(|_| eyre!("Could not parse font"))?;

    Ok(o)
}
