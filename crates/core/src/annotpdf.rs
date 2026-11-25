//! A copy of a PDF with its reading marks written in as standard PDF annotations (#273), so another reader
//! shows them and the file can be handed on: a highlight is a `/Highlight`, an underline an `/Underline`
//! (one quadrilateral a line, in the order Acrobat writes them: upper left, upper right, lower left, lower
//! right), a box a `/Square`, a sticky note a `/Text` and a text box a `/FreeText`. What the person wrote is
//! the `/Contents`. Pieces marked to be cut out (`Kind::Cut`) are a note to oneself and stay out.
//!
//! Each annotation carries its own appearance (`/AP /N`), drawn as the window draws the mark, since several
//! readers do not make one up: a highlight's colour is multiplied into the page (`/BM /Multiply`), so the
//! words under it stay black. A text box's words are drawn in fonts no file needs to embed: Helvetica
//! (WinAnsi), Symbol (Greek, arrows, mathematics) and ZapfDingbats (✓, ①) for what they hold, and
//! `STSong-Light` (`UniGB-UCS2-H`, one of the CJK fonts every PDF reader is to know) for Chinese script.
//! macOS draws that font's ideographs and full-width punctuation right and its other letters (→, α, •, kana)
//! as unrelated ideographs, whatever the CMap, so a letter goes to it only when it is of that script or no
//! base font has it even taken apart (`set`). A reader without a font still has the words in `/Contents`.
//!
//! The marks are kept in points from the top left of the page as hayro draws it (`pdftext::Mark`). Where
//! that page lies in the PDF's own space (the crop box, or the media box, with its origin, inherited from
//! the page tree, turned by `/Rotate`) is hayro's `initial_transform`, so the export inverts exactly that.
//! `/UserUnit` scales the marks and the annotations alike, so it needs nothing.
//!
//! The file is written anew with lopdf, as `shrink` writes it; the source is never touched, and the result
//! goes to a temporary name beside the target before it takes the target's name.

use crate::annotation::{Annotation, Kind, hue};
use anyhow::{Context as _, Result, anyhow, bail};
use hayro_interpret::TransformExt;
use hayro_interpret::hayro_syntax::Pdf;
use hayro_interpret::hayro_syntax::page::Rotation;
use hayro_interpret::kurbo::{Affine, Point};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};
use std::fmt::Write as _;
use std::path::Path;

/// A text box's words, as the window sets them (`pdf.rs`): their size in points, the room between lines
/// (times the size), the room inside the box's edge, and the width of its edge.
const TEXT_PT: f32 = 11.;
const TEXT_LEADING: f32 = 1.4;
const TEXT_PAD: f32 = 4.;
const TEXT_EDGE: f32 = 0.9;
/// The side of a sticky note's icon, in points.
const NOTE_SIDE: f32 = 20.;
/// The width of a box's line, and how thick an underline is drawn.
const BOX_LINE: f32 = 1.5;
const UNDERLINE: f32 = 1.3;
/// A highlight is drawn over the page at this strength, as `Theme::highlight` does.
const HIGHLIGHT_ALPHA: f32 = 0.32;

/// A copy with the marks in it.
#[derive(Debug)]
pub struct Marked {
    pub bytes: Vec<u8>,
    /// The marks written in.
    pub written: usize,
    /// Marks of a page the file does not have, or with no place on it: left out.
    pub left: usize,
}

/// Whether a mark goes into an exported file: everything another reader should see, not a piece marked
/// to be cut out later.
pub fn exported(kind: Kind) -> bool {
    kind != Kind::Cut
}

/// The PDF `source` with `marks` written in, saved at `target` (never over `source`): written to a
/// temporary name beside it first, so a failure leaves no half-written file. An error is a sentence for
/// the person.
pub fn write(source: &Path, marks: &[Annotation], target: &Path) -> Result<Marked> {
    if let (Ok(a), Ok(b)) = (source.canonicalize(), target.canonicalize()) && a == b { bail!("不能把导出的文件存在原文件上：原文件不改动，请换一个名字或位置。") }
    let data = std::fs::read(source).with_context(|| format!("无法读取 {}", source.display()))?;
    let marked = marked(&data, marks)?;
    let name = target.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_else(|| "export.pdf".into());
    let part = target.with_file_name(format!(".{name}.part"));
    let saved = std::fs::write(&part, &marked.bytes).and_then(|_| std::fs::rename(&part, target));
    if let Err(err) = saved {
        let _ = std::fs::remove_file(&part);
        return Err(anyhow!("无法写入 {}：{err}", target.display()));
    }
    Ok(marked)
}

/// `data` with `marks` (those of this file) written in as annotations, after any it has. The same file
/// and marks give the same bytes.
pub fn marked(data: &[u8], marks: &[Annotation]) -> Result<Marked> {
    let mut doc = Document::load_mem(data).map_err(|err| anyhow!("无法解析这个 PDF 的结构（{err}）。"))?;
    if doc.trailer.get(b"Encrypt").is_ok() { bail!("这个 PDF 带有密码或权限保护，另存一份会去掉它的保护，所以没有导出。") }
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let places = places(data)?;
    if pages.is_empty() { bail!("这个 PDF 里找不到页面。") }
    if places.len() != pages.len() { bail!("这个 PDF 的页面结构不完整，两种读法得到的页数不一样（{} 和 {}），没有导出。", pages.len(), places.len()) }
    let mut shared = Shared::default();
    let (mut written, mut left) = (0, 0);
    for mark in marks.iter().filter(|mark| exported(mark.kind)) {
        let (Some(&page), Some(place)) = (mark.page.checked_sub(1).and_then(|i| pages.get(i)), mark.page.checked_sub(1).and_then(|i| places.get(i))) else { left += 1; continue };
        let Some(annotation) = annotation(&mut doc, &mut shared, mark, page, place) else { left += 1; continue };
        let id = doc.add_object(annotation);
        add_to_page(&mut doc, page, id)?;
        written += 1;
    }
    let mut bytes = vec![];
    doc.save_to(&mut bytes).context("无法写出带标注的 PDF")?;
    Ok(Marked { bytes, written, left })
}

/// Where a page as the window draws it lies in the PDF's space.
struct Place {
    /// From points from the top left of the page drawn to the PDF's default user space.
    to_pdf: Affine,
    /// The page's `/Rotate`, in degrees.
    rotate: i64,
}

/// Each page's place, read by hayro as the window reads it.
fn places(data: &[u8]) -> Result<Vec<Place>> {
    let pdf = Pdf::new(data.to_vec()).map_err(|_| anyhow!("无法解析这个 PDF 的结构。"))?;
    Ok(pdf.pages().iter().map(|page| {
        let rotate = match page.rotation() { Rotation::None => 0, Rotation::Horizontal => 90, Rotation::Flipped => 180, Rotation::FlippedHorizontal => 270 };
        Place { to_pdf: page.initial_transform(true).to_kurbo().inverse(), rotate }
    }).collect())
}

/// Objects the annotations share: the graphics state that multiplies a highlight in, and the two fonts.
#[derive(Default)]
struct Shared {
    multiply: Option<ObjectId>,
    latin: Option<ObjectId>,
    symbol: Option<ObjectId>,
    dingbats: Option<ObjectId>,
    cjk: Option<ObjectId>,
}

impl Shared {
    fn multiply(&mut self, doc: &mut Document) -> ObjectId {
        *self.multiply.get_or_insert_with(|| doc.add_object(dictionary! {"Type" => "ExtGState", "BM" => "Multiply", "CA" => 1, "ca" => 1}))
    }

    fn fonts(&mut self, doc: &mut Document) -> Dictionary {
        let latin = *self.latin.get_or_insert_with(|| doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding"}));
        // Symbol and ZapfDingbats are read in their own encodings.
        let symbol = *self.symbol.get_or_insert_with(|| doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Symbol"}));
        let dingbats = *self.dingbats.get_or_insert_with(|| doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "ZapfDingbats"}));
        let cjk = *self.cjk.get_or_insert_with(|| {
            let descriptor = dictionary! {
                "Type" => "FontDescriptor", "FontName" => "STSong-Light", "Flags" => 6,
                "FontBBox" => vec![(-25).into(), (-254).into(), 1000.into(), 880.into()],
                "ItalicAngle" => 0, "Ascent" => 880, "Descent" => -120, "CapHeight" => 880, "StemV" => 93,
            };
            let cid = dictionary! {
                "Type" => "Font", "Subtype" => "CIDFontType0", "BaseFont" => "STSong-Light",
                "CIDSystemInfo" => dictionary! {"Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("GB1"), "Supplement" => 2},
                "FontDescriptor" => descriptor, "DW" => 1000,
            };
            doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type0", "BaseFont" => "STSong-Light", "Encoding" => "UniGB-UCS2-H", "DescendantFonts" => vec![cid.into()]})
        });
        dictionary! {"Helv" => latin, "Symbol" => symbol, "ZaDb" => dingbats, "STSong" => cjk}
    }
}

/// The annotation of one mark on its page, with its appearance; `None` for a mark with no place to stand.
fn annotation(doc: &mut Document, shared: &mut Shared, mark: &Annotation, page: ObjectId, place: &Place) -> Option<Dictionary> {
    let boxes: Vec<[f32; 4]> = mark.boxes.iter().map(|b| crate::handles::normal(*b)).filter(|b| b.iter().all(|n| n.is_finite())).collect();
    let first = *boxes.first()?;
    let mut dict = dictionary! {"Type" => "Annot", "P" => page, "F" => 4, "NM" => text(&mark.id), "M" => date(&mark.updated_at), "CreationDate" => date(&mark.created_at)};
    // What is drawn, in the window's points: the appearance's content starts with the turn to the PDF's space.
    let mut draw = cm(place.to_pdf);
    let mut resources = Dictionary::new();
    let rect = match mark.kind {
        Kind::Highlight | Kind::Underline => {
            let highlight = mark.kind == Kind::Highlight;
            dict.set("Subtype", if highlight { "Highlight" } else { "Underline" });
            let quads: Vec<Object> = boxes.iter().flat_map(|&[x0, y0, x1, y1]| [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]).flat_map(|(x, y)| {
                let p = place.to_pdf * Point::new(x as f64, y as f64);
                [Object::Real(p.x as f32), Object::Real(p.y as f32)]
            }).collect();
            dict.set("QuadPoints", quads);
            let color = if highlight { over_white(hsl(hue(&mark.color)), HIGHLIGHT_ALPHA) } else { over_white(hsl((215., 0.85, 0.48)), 0.9) };
            dict.set("C", rgb(color));
            if highlight {
                resources.set("ExtGState", dictionary! {"Multiply" => shared.multiply(doc)});
                draw.push_str("/Multiply gs ");
            }
            let _ = write!(draw, "{} rg ", colour(color));
            for &[x0, y0, x1, y1] in &boxes {
                let (top, height) = if highlight { (y0, y1 - y0) } else { (y1 - UNDERLINE, UNDERLINE) };
                let _ = write!(draw, "{} {} {} {} re ", n(x0), n(top), n(x1 - x0), n(height));
            }
            draw.push('f');
            bounds(place, &boxes)
        }
        Kind::Box => {
            dict.set("Subtype", "Square");
            let color = hsl(hue(&mark.color));
            dict.set("C", rgb(color));
            dict.set("BS", dictionary! {"Type" => "Border", "W" => Object::Real(BOX_LINE), "S" => "S"});
            let [x0, y0, x1, y1] = first;
            let _ = write!(draw, "{} RG {} w {} {} {} {} re S", colour(color), n(BOX_LINE), n(x0), n(y0), n(x1 - x0), n(y1 - y0));
            let half = BOX_LINE / 2.;
            bounds(place, &[[x0 - half, y0 - half, x1 + half, y1 + half]])
        }
        Kind::Note => {
            dict.set("Subtype", "Text");
            dict.set("Name", "Comment");
            dict.set("Open", false);
            let paper = hsl((46., 0.95, 0.78));
            dict.set("C", rgb(paper));
            let [x, y, ..] = first;
            let s = NOTE_SIDE;
            let _ = write!(draw, "{} rg {} RG 0.8 w {} {} {} {} re B ", colour(paper), colour(hsl((40., 0.6, 0.45))), n(x + 0.4), n(y + 0.4), n(s - 0.8), n(s - 0.8));
            let _ = write!(draw, "{} RG 1.2 w", colour(hsl((35., 0.7, 0.25))));
            for line in [0.32, 0.5, 0.68] { let _ = write!(draw, " {} {} m {} {} l", n(x + s * 0.25), n(y + s * line), n(x + s * 0.75), n(y + s * line)); }
            draw.push_str(" S");
            bounds(place, &[[x, y, x + s, y + s]])
        }
        Kind::Text => {
            dict.set("Subtype", "FreeText");
            let style = mark.style.clone().unwrap_or_default();
            let (fill, border, ink) = (text_fill(&style.fill), text_border(&style.border), text_ink(&style.ink));
            let edge = if border.is_some() { TEXT_EDGE } else { 0. };
            let [x0, y0, x1, mut y1] = first;
            let width = (x1 - x0 - 2. * (edge + TEXT_PAD)).max(1.);
            let lines = wrap(&mark.note, width);
            // As the window does, the box is never shorter than its words.
            y1 = y1.max(y0 + lines.len() as f32 * TEXT_PT * TEXT_LEADING + 2. * (edge + TEXT_PAD));
            dict.set("DA", text(&format!("{} rg /Helv {} Tf", colour(ink), n(TEXT_PT))));
            if let Some(fill) = fill { dict.set("C", rgb(fill)) }
            dict.set("BS", dictionary! {"Type" => "Border", "W" => Object::Real(edge), "S" => "S"});
            if place.rotate != 0 { dict.set("Rotate", place.rotate) }
            if let Some(fill) = fill { let _ = write!(draw, "{} rg {} {} {} {} re f ", colour(fill), n(x0), n(y0), n(x1 - x0), n(y1 - y0)); }
            if let Some(border) = border { let _ = write!(draw, "{} RG {} w {} {} {} {} re S ", colour(border), n(edge), n(x0 + edge / 2.), n(y0 + edge / 2.), n(x1 - x0 - edge), n(y1 - y0 - edge)); }
            if !lines.is_empty() {
                resources.set("Font", shared.fonts(doc));
                let _ = write!(draw, "BT {} rg ", colour(ink));
                let leading = TEXT_PT * TEXT_LEADING;
                for (i, line) in lines.iter().enumerate() {
                    // The baseline where the window's line of this height puts it.
                    let base = y0 + edge + TEXT_PAD + i as f32 * leading + (leading - TEXT_PT) / 2. + 0.8 * TEXT_PT;
                    let mut x = x0 + edge + TEXT_PAD;
                    for (font, run, wide) in runs(line) {
                        let _ = write!(draw, "/{} {} Tf 1 0 0 -1 {} {} Tm {} Tj ", font.name(), n(TEXT_PT), n(x), n(base), run);
                        x += wide * TEXT_PT;
                    }
                }
                draw.push_str("ET");
            }
            bounds(place, &[[x0, y0, x1, y1]])
        }
        Kind::Cut => return None,
    };
    if !mark.note.trim().is_empty() { dict.set("Contents", text(&mark.note)) }
    let [llx, lly, urx, ury] = rect;
    if !(urx > llx && ury > lly) { return None }
    let rect: Vec<Object> = rect.iter().map(|&v| Object::Real(v)).collect();
    dict.set("Rect", rect.clone());
    let mut look = Stream::new(dictionary! {"Type" => "XObject", "Subtype" => "Form", "BBox" => rect, "Resources" => resources}, draw.into_bytes());
    let _ = look.compress();
    let look = doc.add_object(look);
    dict.set("AP", dictionary! {"N" => look});
    Some(dict)
}

/// Put an annotation on a page, after those it has: its `/Annots` array, inline or an object of its own.
fn add_to_page(doc: &mut Document, page: ObjectId, id: ObjectId) -> Result<()> {
    let held = doc.get_dictionary(page).map_err(|_| anyhow!("这个 PDF 的一页读不出来。"))?.get(b"Annots").ok().cloned();
    match held {
        Some(Object::Reference(array)) if matches!(doc.get_object(array), Ok(Object::Array(_))) => {
            if let Ok(Object::Array(annots)) = doc.get_object_mut(array) { annots.push(id.into()) }
        }
        Some(Object::Array(mut annots)) => {
            annots.push(id.into());
            doc.get_dictionary_mut(page)?.set("Annots", annots);
        }
        _ => doc.get_dictionary_mut(page)?.set("Annots", vec![Object::from(id)]),
    }
    Ok(())
}

/// The smallest rectangle in the PDF's space holding these rectangles of the page drawn, as `[llx lly urx ury]`.
fn bounds(place: &Place, boxes: &[[f32; 4]]) -> [f32; 4] {
    let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for &[x0, y0, x1, y1] in boxes {
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let p = place.to_pdf * Point::new(x as f64, y as f64);
            out = [out[0].min(p.x as f32), out[1].min(p.y as f32), out[2].max(p.x as f32), out[3].max(p.y as f32)];
        }
    }
    out
}

/// The operator that turns the window's points into the PDF's space.
fn cm(to_pdf: Affine) -> String {
    let [a, b, c, d, e, f] = to_pdf.as_coeffs();
    format!("{} {} {} {} {} {} cm ", n(a as f32), n(b as f32), n(c as f32), n(d as f32), n(e as f32), n(f as f32))
}

/// A number as a content stream writes it: at most three decimals, no trailing zeros.
fn n(v: f32) -> String {
    let s = format!("{:.3}", if v.abs() < 0.0005 { 0. } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A text string: plain when it is ASCII, else UTF-16BE behind its byte order mark, as the standard says.
fn text(s: &str) -> Object {
    if s.is_ascii() { return Object::String(s.as_bytes().to_vec(), StringFormat::Literal) }
    let mut bytes = vec![0xFE, 0xFF];
    bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes, StringFormat::Hexadecimal)
}

/// A date of the library (`2026-11-24T10:00:00.000Z`) as a PDF writes one (`D:20261124100000Z`).
fn date(iso: &str) -> Object {
    let digits: String = iso.chars().take(19).filter(char::is_ascii_digit).collect();
    text(&format!("D:{digits}Z"))
}

type Rgb = [f32; 3];

fn hsl((h, s, l): (f32, f32, f32)) -> Rgb {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let h = (h.rem_euclid(360.)) / 60.;
    let x = c * (1. - (h % 2. - 1.).abs());
    let (r, g, b) = match h as u32 { 0 => (c, x, 0.), 1 => (x, c, 0.), 2 => (0., c, x), 3 => (0., x, c), 4 => (x, 0., c), _ => (c, 0., x) };
    let m = l - c / 2.;
    [r + m, g + m, b + m]
}

/// A colour laid over white paper at a strength.
fn over_white(color: Rgb, alpha: f32) -> Rgb {
    color.map(|c| 1. - alpha * (1. - c))
}

fn rgb(color: Rgb) -> Vec<Object> {
    color.iter().map(|&c| Object::Real((c * 1000.).round() / 1000.)).collect()
}

fn colour(color: Rgb) -> String {
    color.map(n).join(" ")
}

/// A text box's colours as the window draws them (`Theme::text_fill`, `text_border`, `text_ink`); `none` is none.
fn text_fill(key: &str) -> Option<Rgb> {
    match key {
        "none" => None,
        "white" => Some([1., 1., 1.]),
        _ => { let (h, s, _) = hue(key); Some(hsl((h, s.min(0.9), 0.9))) }
    }
}

fn text_border(key: &str) -> Option<Rgb> {
    match key {
        "none" => None,
        "black" => Some(hsl((0., 0., 0.12))),
        "gray" => Some(hsl((0., 0., 0.55))),
        _ => Some(hsl(hue(key))),
    }
}

fn text_ink(key: &str) -> Rgb {
    match key {
        "gray" => hsl((0., 0., 0.42)),
        "red" | "orange" | "green" | "blue" | "purple" => { let (h, s, _) = hue(key); hsl((h, s.min(0.85), 0.38)) }
        _ => hsl((0., 0., 0.1)),
    }
}

/// The fonts a text box's words are set in: three of the fourteen every reader carries, for the letters they
/// hold, and `STSong-Light` for Chinese and Japanese script. macOS draws only that script and the full-width
/// forms right in the CJK font (an arrow, a Greek letter or a bullet comes out as some ideograph), so a letter
/// goes to it only when it is of that script or no base font has it, even taken apart (`set`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Font { Helv, Symbol, Dingbats, Song }

impl Font {
    fn name(self) -> &'static str {
        match self { Font::Helv => "Helv", Font::Symbol => "Symbol", Font::Dingbats => "ZaDb", Font::Song => "STSong" }
    }
}

/// A letter as it is drawn: its font, its code there (a byte in a base font, UCS-2 in `STSong`, which is
/// `UniGB-UCS2-H`) and its width in ems.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Glyph { font: Font, code: u16, wide: f32 }

/// Helvetica's widths for ASCII, in thousandths of an em.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
/// What WinAnsi holds beyond Latin-1, at 0x80 to 0x9F, with Helvetica's widths.
const WINANSI: [(char, u8, u16); 27] = [
    ('€', 0x80, 556), ('‚', 0x82, 222), ('ƒ', 0x83, 556), ('„', 0x84, 333), ('…', 0x85, 1000), ('†', 0x86, 556), ('‡', 0x87, 556), ('ˆ', 0x88, 333), ('‰', 0x89, 1000),
    ('Š', 0x8A, 667), ('‹', 0x8B, 333), ('Œ', 0x8C, 1000), ('Ž', 0x8E, 611), ('‘', 0x91, 222), ('’', 0x92, 222), ('“', 0x93, 333), ('”', 0x94, 333), ('•', 0x95, 350),
    ('–', 0x96, 556), ('—', 0x97, 1000), ('˜', 0x98, 333), ('™', 0x99, 1000), ('š', 0x9A, 500), ('›', 0x9B, 333), ('œ', 0x9C, 944), ('ž', 0x9E, 500), ('Ÿ', 0x9F, 667),
];
/// The Greek letters, as Symbol has them under Latin ones, and that font's widths of those.
const GREEK: [(&str, &str); 2] = [("ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩΩ∆ϑϕϖ", "ABGDEZHQIKLMNXOPRSTUFCYWWDJjv"), ("αβγδεζηθικλμνξοπρςστυφχψω", "abgdezhqiklmnxoprVstufcyw")];
const SYMBOL_LETTERS: [u16; 52] = [
    722, 667, 722, 612, 611, 763, 603, 722, 333, 631, 722, 686, 889, 722, 722, 768, 741, 556, 592, 611, 690, 439, 768, 645, 795, 611,
    631, 549, 549, 494, 439, 521, 411, 603, 329, 603, 549, 549, 576, 521, 549, 549, 521, 549, 603, 439, 576, 713, 686, 493, 686, 494,
];
/// Symbol's arrows, the signs of mathematics and the rest it holds that Helvetica does not.
const SYMBOL: [(char, u8, u16); 65] = [
    ('∀', 0x22, 713), ('∃', 0x24, 549), ('∋', 0x27, 439), ('∗', 0x2A, 500), ('−', 0x2D, 549), ('≅', 0x40, 549), ('∴', 0x5C, 863), ('⊥', 0x5E, 658), ('∼', 0x7E, 549),
    ('ϒ', 0xA1, 620), ('′', 0xA2, 247), ('≤', 0xA3, 549), ('⁄', 0xA4, 167), ('∞', 0xA5, 713), ('♣', 0xA7, 753), ('♦', 0xA8, 753), ('♥', 0xA9, 753), ('♠', 0xAA, 753),
    ('↔', 0xAB, 1042), ('←', 0xAC, 987), ('↑', 0xAD, 603), ('→', 0xAE, 987), ('↓', 0xAF, 603), ('″', 0xB2, 411), ('≥', 0xB3, 549), ('∝', 0xB5, 713), ('∂', 0xB6, 494),
    ('≠', 0xB9, 549), ('≡', 0xBA, 549), ('≈', 0xBB, 549), ('↵', 0xBF, 658), ('ℵ', 0xC0, 823), ('ℑ', 0xC1, 686), ('ℜ', 0xC2, 795), ('℘', 0xC3, 987), ('⊗', 0xC4, 768),
    ('⊕', 0xC5, 768), ('∅', 0xC6, 823), ('∩', 0xC7, 768), ('∪', 0xC8, 768), ('⊃', 0xC9, 713), ('⊇', 0xCA, 713), ('⊄', 0xCB, 713), ('⊂', 0xCC, 713), ('⊆', 0xCD, 713),
    ('∈', 0xCE, 713), ('∉', 0xCF, 713), ('∠', 0xD0, 768), ('∇', 0xD1, 713), ('∏', 0xD5, 823), ('√', 0xD6, 549), ('⋅', 0xD7, 250), ('∧', 0xD9, 603), ('∨', 0xDA, 603),
    ('⇔', 0xDB, 1042), ('⇐', 0xDC, 987), ('⇑', 0xDD, 603), ('⇒', 0xDE, 987), ('⇓', 0xDF, 603), ('◊', 0xE0, 494), ('〈', 0xE1, 329), ('∑', 0xE5, 713), ('〉', 0xF1, 329),
    ('∫', 0xF2, 274), ('ϕ', 0x6A, 603),
];
/// The marks of ZapfDingbats a note is likely to hold: ticks and crosses, stars and shapes; ① to ⑩ follow at 0xAC.
const DINGBATS: [(char, u8, u16); 10] = [
    ('✓', 0x33, 755), ('✔', 0x34, 846), ('✗', 0x37, 744), ('✘', 0x38, 723), ('★', 0x48, 816), ('●', 0x6C, 791), ('■', 0x6E, 761), ('▲', 0x73, 892), ('▼', 0x74, 892), ('◆', 0x75, 788),
];

/// A letter in a base font, if one has it.
fn base(c: char) -> Option<Glyph> {
    let glyph = |font, code: u8, wide: u16| Some(Glyph { font, code: code as u16, wide: wide as f32 / 1000. });
    match c {
        ' '..='~' => return glyph(Font::Helv, c as u8, HELVETICA[c as usize - 32]),
        '\u{a0}'..='\u{ff}' => return glyph(Font::Helv, c as u8, 556),
        '①'..='⑩' => return glyph(Font::Dingbats, 0xAC + (c as u32 - '①' as u32) as u8, 788),
        _ => {}
    }
    if let Some(&(_, code, wide)) = WINANSI.iter().find(|w| w.0 == c) { return glyph(Font::Helv, code, wide) }
    for (greek, latin) in GREEK {
        if let Some(i) = greek.chars().position(|g| g == c) {
            let letter = latin.as_bytes()[i];
            let at = if letter.is_ascii_uppercase() { letter - b'A' } else { 26 + letter - b'a' };
            return glyph(Font::Symbol, letter, SYMBOL_LETTERS[at as usize]);
        }
    }
    if let Some(&(_, code, wide)) = SYMBOL.iter().find(|w| w.0 == c) { return glyph(Font::Symbol, code, wide) }
    DINGBATS.iter().find(|w| w.0 == c).and_then(|&(_, code, wide)| glyph(Font::Dingbats, code, wide))
}

/// Whether a letter is of the script the CJK font is for: ideographs and their radicals, kana and bopomofo,
/// CJK punctuation and the full-width forms.
fn cjk(c: char) -> bool {
    matches!(c, '\u{2e80}'..='\u{2fdf}' | '\u{3000}'..='\u{312f}' | '\u{31a0}'..='\u{31ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{fe30}'..='\u{fe4f}' | '\u{ff00}'..='\u{ffef}')
}

/// Quotation marks, the dash and the ellipsis Chinese is written with too: set in the font of the words they
/// stand by, full width among Chinese and narrow among Latin.
fn shared(c: char) -> bool {
    matches!(c, '“' | '”' | '‘' | '’' | '—' | '…')
}

/// A line as it is drawn, letter by letter. A letter no font has is taken apart (`ā` is `a`, `₂` is `2`, `℃` is
/// `°C`) when its parts are in a base font, else left to the CJK font, which other readers may have it in;
/// letters beyond the Basic Multilingual Plane, which neither encoding writes, are left out.
fn set(line: &str) -> Vec<Glyph> {
    use unicode_normalization::UnicodeNormalization as _;
    let song = |c: char| Glyph { font: Font::Song, code: c as u16, wide: 1. };
    let chars: Vec<char> = line.chars().collect();
    let alone: Vec<Option<Vec<Glyph>>> = chars.iter().map(|&c| {
        if shared(c) { return None }
        if let Some(glyph) = base(c) { return Some(vec![glyph]) }
        if cjk(c) { return Some(vec![song(c)]) }
        let parts: Option<Vec<Glyph>> = c.nfkd().filter(|&p| !unicode_normalization::char::is_combining_mark(p)).map(base).collect();
        Some(match parts {
            Some(parts) if !parts.is_empty() => parts,
            _ if (c as u32) < 0x10000 => vec![song(c)],
            _ => vec![],
        })
    }).collect();
    // Of a shared mark, the nearest letter that is neither a space nor shared: an opening quotation mark
    // looks after itself first, the others before.
    let among = |i: usize, ahead: bool| -> bool {
        let font = |j: usize| alone[j].as_ref().and_then(|g| g.first()).filter(|g| !(g.font == Font::Helv && g.code == 32)).map(|g| g.font);
        let before = (0..i).rev().find_map(font);
        let after = (i + 1..chars.len()).find_map(font);
        (if ahead { after.or(before) } else { before.or(after) }) == Some(Font::Song)
    };
    let mut out = vec![];
    for (i, glyphs) in alone.iter().enumerate() {
        match glyphs {
            Some(glyphs) => out.extend_from_slice(glyphs),
            None if among(i, matches!(chars[i], '“' | '‘')) => out.push(song(chars[i])),
            None => out.extend(base(chars[i])),
        }
    }
    out
}

/// How wide a letter is, in ems, as `wrap` counts it: a shared mark at the wider of its two.
fn advance(c: char) -> f32 {
    if shared(c) { return 1. }
    set(&c.to_string()).iter().map(|g| g.wide).sum()
}

/// Whether a letter is part of a word a line breaks around, not one it may break after.
fn word(c: char) -> bool {
    !cjk(c) && !shared(c)
}

/// The lines a text box's words take in a width of points: its own lines, each broken at the last space that
/// fits, or after any letter of a script written without spaces, or inside a word longer than the line.
fn wrap(text: &str, width: f32) -> Vec<String> {
    let room = width / TEXT_PT;
    let mut lines = vec![];
    for paragraph in text.trim_end().split('\n') {
        let paragraph = paragraph.trim_end_matches('\r');
        let mut line = String::new();
        let mut wide = 0.;
        // The pieces a line may break between: a word of Latin letters with the spaces after it, or one other letter.
        let mut pieces: Vec<String> = vec![];
        for c in paragraph.chars().map(|c| if c == '\t' { ' ' } else { c }) {
            match pieces.last_mut() {
                Some(last) if word(c) && last.chars().last().is_some_and(|l| word(l) && (l != ' ' || c == ' ')) => last.push(c),
                _ => pieces.push(c.to_string()),
            }
        }
        for piece in pieces {
            let size: f32 = piece.chars().map(advance).sum();
            let bare: f32 = piece.trim_end().chars().map(advance).sum();
            if wide + bare > room && !line.is_empty() {
                lines.push(line.trim_end().to_string());
                (line, wide) = (String::new(), 0.);
            }
            if bare > room {
                // A word longer than the line is cut where it reaches the edge.
                for c in piece.chars() {
                    if wide + advance(c) > room && !line.is_empty() {
                        lines.push(line.trim_end().to_string());
                        (line, wide) = (String::new(), 0.);
                    }
                    line.push(c);
                    wide += advance(c);
                }
                continue;
            }
            line.push_str(&piece);
            wide += size;
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(String::is_empty) { lines.pop(); }
    lines
}

/// A line's letters in runs of one font, each as the string `Tj` shows (the codes of a base font as a literal
/// string, those of the CJK font as a hex string) and its width in ems.
fn runs(line: &str) -> Vec<(Font, String, f32)> {
    let mut out: Vec<(Font, Vec<u16>, f32)> = vec![];
    for glyph in set(line) {
        match out.last_mut() {
            Some((font, codes, wide)) if *font == glyph.font => { codes.push(glyph.code); *wide += glyph.wide }
            _ => out.push((glyph.font, vec![glyph.code], glyph.wide)),
        }
    }
    out.into_iter().map(|(font, codes, wide)| (font, if font == Font::Song { hex(&codes) } else { literal(&codes) }, wide)).collect()
}

/// Bytes of a base font as a literal string.
fn literal(codes: &[u16]) -> String {
    let mut out = String::from("(");
    for &code in codes {
        match code as u8 {
            c @ (b'(' | b')' | b'\\') => { out.push('\\'); out.push(c as char) }
            c @ b' '..=b'~' => out.push(c as char),
            c => { let _ = write!(out, "\\{c:03o}"); }
        }
    }
    out.push(')');
    out
}

/// UCS-2 codes for the CJK font as a hex string.
fn hex(codes: &[u16]) -> String {
    let mut out = String::from("<");
    for code in codes { let _ = write!(out, "{code:04X}"); }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::TextStyle;

    /// What a page of a test file says of its geometry, on itself or on the page tree above it.
    #[derive(Default, Clone)]
    struct Geometry {
        media: Option<[i64; 4]>,
        crop: Option<[i64; 4]>,
        rotate: Option<i64>,
        user_unit: Option<f32>,
    }

    fn put(dict: &mut Dictionary, g: &Geometry) {
        let array = |b: [i64; 4]| b.iter().map(|&v| Object::Integer(v)).collect::<Vec<_>>();
        if let Some(b) = g.media { dict.set("MediaBox", array(b)) }
        if let Some(b) = g.crop { dict.set("CropBox", array(b)) }
        if let Some(r) = g.rotate { dict.set("Rotate", r) }
        if let Some(u) = g.user_unit { dict.set("UserUnit", Object::Real(u)) }
    }

    /// A one-page PDF: a line of Helvetica at 72 720, the geometry given on the page and on its tree, and
    /// `annots` already on the page (inline or, with `indirect`, an array object of its own).
    fn file(page: &Geometry, tree: &Geometry, annots: usize, indirect: bool) -> Vec<u8> {
        file_with(page, tree, annots, indirect, "BT /F1 12 Tf 72 720 Td (Roots grow toward water.) Tj ET")
    }

    fn file_with(page: &Geometry, tree: &Geometry, annots: usize, indirect: bool, content: &str) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding"});
        let content = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let mut dict = dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => content, "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}};
        put(&mut dict, page);
        let old: Vec<Object> = (0..annots).map(|i| doc.add_object(dictionary! {"Type" => "Annot", "Subtype" => "Square", "Rect" => vec![10.into(), 10.into(), (20 + i as i64).into(), 20.into()], "NM" => Object::string_literal(format!("old-{i}"))}).into()).collect();
        if annots > 0 { if indirect { let array = doc.add_object(old); dict.set("Annots", array) } else { dict.set("Annots", old) } }
        let page_id = doc.add_object(dict);
        let mut pages = dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1};
        put(&mut pages, tree);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn letter() -> Geometry {
        Geometry { media: Some([0, 0, 612, 792]), ..Geometry::default() }
    }

    fn mark(kind: Kind, boxes: Vec<[f32; 4]>, note: &str) -> Annotation {
        Annotation {
            id: format!("A-{}", kind.key()), paper_id: "P".into(), resource_id: "R".into(), page: 1, kind, quote: if kind.shape() { String::new() } else { "Roots".into() },
            boxes, note: note.into(), color: if matches!(kind, Kind::Highlight | Kind::Box) { "yellow".into() } else { String::new() },
            style: (kind == Kind::Text).then(TextStyle::default), created_at: "2026-11-20T08:30:00.000Z".into(), updated_at: "2026-11-24T10:05:09.123Z".into(),
        }
    }

    /// The annotations on the first page of `bytes`, in order.
    fn annots(bytes: &[u8]) -> Vec<Dictionary> {
        let doc = Document::load_mem(bytes).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let mut list = match doc.get_dictionary(page).unwrap().get(b"Annots").unwrap() {
            Object::Reference(id) => doc.get_object(*id).unwrap().as_array().unwrap().clone(),
            Object::Array(a) => a.clone(),
            _ => panic!(),
        };
        list.iter_mut().map(|o| doc.get_dictionary(o.as_reference().unwrap()).unwrap().clone()).collect()
    }

    fn numbers(dict: &Dictionary, key: &[u8]) -> Vec<f32> {
        dict.get(key).unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
    }

    /// A highlight at the window's [10 20 110 30] on a page of each geometry: its quad and rectangle in the
    /// PDF's space, worked out by hand from what the standard says each one means.
    #[test]
    fn marks_land_where_the_page_shows_them() {
        let hl = |page: Geometry, tree: Geometry| {
            let out = marked(&file(&page, &tree, 0, false), &[mark(Kind::Highlight, vec![[10., 20., 110., 30.]], "")]).unwrap();
            let a = annots(&out.bytes).remove(0);
            (numbers(&a, b"QuadPoints"), numbers(&a, b"Rect"))
        };
        // Upright: y counts up from the bottom of the media box.
        let (quad, rect) = hl(letter(), Geometry::default());
        assert!(close(&quad, &[10., 772., 110., 772., 10., 762., 110., 762.]), "{quad:?}");
        assert!(close(&rect, &[10., 762., 110., 772.]), "{rect:?}");
        // A crop box: the page drawn is the crop box, its top left at (50, 700).
        let (quad, _) = hl(Geometry { crop: Some([50, 60, 550, 700]), ..letter() }, Geometry::default());
        assert!(close(&quad, &[60., 680., 160., 680., 60., 670., 160., 670.]), "{quad:?}");
        // A media box away from the origin, inherited from the page tree.
        let (quad, _) = hl(Geometry::default(), Geometry { media: Some([100, 200, 700, 1000]), ..Geometry::default() });
        assert!(close(&quad, &[110., 980., 210., 980., 110., 970., 210., 970.]), "{quad:?}");
        // A crop box larger than the media box is cut to it.
        let (quad, _) = hl(Geometry { crop: Some([-100, -100, 1000, 1000]), ..letter() }, Geometry::default());
        assert!(close(&quad, &[10., 772., 110., 772., 10., 762., 110., 762.]), "{quad:?}");
        // Turned a quarter clockwise: the page's left edge is at the top, so the window's top left is the
        // PDF's (0, 0), across is up the PDF's y and down is along its x.
        let (quad, rect) = hl(Geometry { rotate: Some(90), ..letter() }, Geometry::default());
        assert!(close(&quad, &[20., 10., 20., 110., 30., 10., 30., 110.]), "{quad:?}");
        assert!(close(&rect, &[20., 10., 30., 110.]), "{rect:?}");
        // Upside down: the window's top left is the PDF's bottom right.
        let (quad, _) = hl(Geometry { rotate: Some(180), ..letter() }, Geometry::default());
        assert!(close(&quad, &[602., 20., 502., 20., 602., 30., 502., 30.]), "{quad:?}");
        // A quarter the other way, inherited: the window's top left is the PDF's top right.
        let (quad, _) = hl(letter(), Geometry { rotate: Some(270), ..Geometry::default() });
        assert!(close(&quad, &[592., 782., 592., 682., 582., 782., 582., 682.]), "{quad:?}");
        // Turned, with a crop box off the origin: −90 is 270.
        let (quad, _) = hl(Geometry { crop: Some([50, 60, 550, 700]), rotate: Some(-90), ..letter() }, Geometry::default());
        assert!(close(&quad, &[530., 690., 530., 590., 520., 690., 520., 590.]), "{quad:?}");
        // A user unit scales the marks and the annotations alike: the numbers are those of the plain page.
        let (quad, _) = hl(Geometry { user_unit: Some(2.), ..letter() }, Geometry::default());
        assert!(close(&quad, &[10., 772., 110., 772., 10., 762., 110., 762.]), "{quad:?}");
    }

    /// The window's glyph boxes, read by hayro from a cropped page turned each way (its text set so that it
    /// reads upright as shown, as a scanned landscape page's is), come back onto the words: the box of the
    /// first glyph, taken back to the PDF's space, holds a point inside the 'R' as the content draws it.
    #[test]
    fn a_highlight_of_read_glyphs_covers_them() {
        for (rotate, tm, inside) in [(0, "1 0 0 1 72 720", (75., 723.)), (90, "0 1 -1 0 300 72", (296., 75.)), (180, "-1 0 0 -1 500 300", (497., 297.)), (270, "0 -1 1 0 300 700", (304., 697.))] {
            let data = file_with(&Geometry { crop: Some([30, 40, 580, 760]), rotate: Some(rotate), ..letter() }, &Geometry::default(), 0, false, &format!("BT /F1 12 Tf {tm} Tm (Roots grow toward water.) Tj ET"));
            let pdf = Pdf::new(data.clone()).unwrap();
            let cache = hayro_interpret::InterpreterCache::new();
            let glyphs = crate::pdftext::page_marks(&pdf.pages()[0], &cache);
            assert!(!glyphs.is_empty(), "rotate {rotate}");
            let g = &glyphs[0];
            assert_eq!(g.text, "R");
            let (a, b) = (place_point(&data, (g.x0, g.y - g.size * 0.7)), place_point(&data, (g.x1, g.y)));
            let (lo_x, hi_x, lo_y, hi_y) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
            assert!((lo_x..=hi_x).contains(&inside.0) && (lo_y..=hi_y).contains(&inside.1), "rotate {rotate}: {a:?} {b:?}");
        }
    }

    fn place_point(data: &[u8], (x, y): (f32, f32)) -> (f32, f32) {
        let p = places(data).unwrap()[0].to_pdf * Point::new(x as f64, y as f64);
        (p.x as f32, p.y as f32)
    }

    #[test]
    fn every_kind_is_a_standard_annotation_with_its_look() {
        let marks = [
            mark(Kind::Highlight, vec![[72., 60., 200., 74.], [72., 76., 150., 90.]], "根向水生长"),
            mark(Kind::Underline, vec![[72., 100., 200., 114.]], "plain note"),
            mark(Kind::Box, vec![[300., 300., 400., 380.]], ""),
            mark(Kind::Note, vec![[500., 100., 500., 100.]], "便签：再读一遍"),
            mark(Kind::Text, vec![[100., 400., 280., 440.]], "文本框 with English words that wrap onto another line"),
            mark(Kind::Cut, vec![[100., 500., 200., 600.]], "for the slides"),
        ];
        let out = marked(&file(&letter(), &Geometry::default(), 0, false), &marks).unwrap();
        assert_eq!((out.written, out.left), (5, 0));
        let doc = Document::load_mem(&out.bytes).unwrap();
        let list = annots(&out.bytes);
        let kinds: Vec<&[u8]> = list.iter().map(|a| a.get(b"Subtype").unwrap().as_name().unwrap()).collect();
        assert_eq!(kinds, [&b"Highlight"[..], b"Underline", b"Square", b"Text", b"FreeText"]);
        for a in &list {
            assert_eq!(a.get(b"F").unwrap().as_i64().unwrap(), 4);
            assert!(a.get(b"P").unwrap().as_reference().is_ok());
            assert_eq!(a.get(b"M").unwrap().as_str().unwrap(), b"D:20261124100509Z");
            // Every one has its own appearance, a form the size of its rectangle.
            let look = doc.get_object(a.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap()).unwrap().as_stream().unwrap();
            assert_eq!(look.dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Form");
            assert!(close(&numbers(&look.dict, b"BBox"), &numbers(a, b"Rect")));
        }
        // Two lines, two quads; the ids and the notes as written: Chinese as UTF-16BE behind its mark.
        assert_eq!(numbers(&list[0], b"QuadPoints").len(), 16);
        assert_eq!(list[0].get(b"NM").unwrap().as_str().unwrap(), b"A-highlight");
        let chinese = list[0].get(b"Contents").unwrap().as_str().unwrap();
        assert_eq!(&chinese[..2], &[0xFE, 0xFF]);
        assert_eq!(String::from_utf16(&chinese[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()).unwrap(), "根向水生长");
        assert_eq!(list[1].get(b"Contents").unwrap().as_str().unwrap(), b"plain note");
        assert!(list[2].get(b"Contents").is_err());
        // A highlight is multiplied into the page, in the pale colour the window shows.
        let look = |a: &Dictionary| { let s = doc.get_object(a.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap()).unwrap().as_stream().unwrap().clone(); (String::from_utf8_lossy(&s.decompressed_content().unwrap_or(s.content.clone())).to_string(), s.dict.get(b"Resources").unwrap().as_dict().unwrap().clone()) };
        let (draw, res) = look(&list[0]);
        assert!(draw.contains("/Multiply gs"), "{draw}");
        let gs = res.get(b"ExtGState").unwrap().as_dict().unwrap().get(b"Multiply").unwrap().as_reference().unwrap();
        assert_eq!(doc.get_dictionary(gs).unwrap().get(b"BM").unwrap().as_name().unwrap(), b"Multiply");
        let c = numbers(&list[0], b"C");
        assert!(c[0] > 0.95 && c[2] < 0.8, "{c:?}");
        // A text box: its words in Helvetica and in the CJK font, its box grown to hold them.
        let (draw, res) = look(&list[4]);
        assert!(draw.contains("/STSong 11 Tf") && draw.contains("<6587672C6846>") && draw.contains("( with English words"), "{draw}");
        let fonts = res.get(b"Font").unwrap().as_dict().unwrap();
        let cjk = doc.get_dictionary(fonts.get(b"STSong").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(cjk.get(b"Encoding").unwrap().as_name().unwrap(), b"UniGB-UCS2-H");
        let rect = numbers(&list[4], b"Rect");
        assert!(rect[3] - rect[1] > 40., "{rect:?}");
    }

    #[test]
    fn marks_already_in_the_file_stay() {
        for indirect in [false, true] {
            let out = marked(&file(&letter(), &Geometry::default(), 2, indirect), &[mark(Kind::Box, vec![[300., 300., 400., 380.]], "")]).unwrap();
            let names: Vec<Vec<u8>> = annots(&out.bytes).iter().map(|a| a.get(b"NM").unwrap().as_str().unwrap().to_vec()).collect();
            assert_eq!(names, [b"old-0".to_vec(), b"old-1".to_vec(), b"A-box".to_vec()]);
        }
    }

    #[test]
    fn the_same_marks_give_the_same_file() {
        let data = file(&letter(), &Geometry { rotate: Some(90), ..Geometry::default() }, 1, false);
        let marks = [mark(Kind::Highlight, vec![[72., 60., 200., 74.]], "x"), mark(Kind::Text, vec![[100., 400., 280., 440.]], "两次")];
        assert_eq!(marked(&data, &marks).unwrap().bytes, marked(&data, &marks).unwrap().bytes);
    }

    #[test]
    fn a_mark_of_a_page_not_there_is_left_out() {
        let mut far = mark(Kind::Highlight, vec![[72., 60., 200., 74.]], "");
        far.page = 3;
        let out = marked(&file(&letter(), &Geometry::default(), 0, false), &[far]).unwrap();
        assert_eq!((out.written, out.left), (0, 1));
    }

    #[test]
    fn a_protected_or_broken_file_is_refused_with_a_sentence() {
        assert!(marked(b"not a pdf", &[]).unwrap_err().to_string().contains("无法解析"));
        let mut doc = Document::load_mem(&file(&letter(), &Geometry::default(), 0, false)).unwrap();
        let encrypt = doc.add_object(dictionary! {"Filter" => "Standard", "V" => 1, "R" => 2, "O" => Object::string_literal(vec![0u8; 32]), "U" => Object::string_literal(vec![0u8; 32]), "P" => -4});
        doc.trailer.set("Encrypt", encrypt);
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        let err = marked(&bytes, &[]).unwrap_err().to_string();
        assert!(err.contains("保护") || err.contains("无法解析"), "{err}");
    }

    #[test]
    fn written_beside_and_never_over_the_source() {
        let dir = std::env::temp_dir().join(format!("wisp-annotpdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("paper.pdf");
        let data = file(&letter(), &Geometry::default(), 0, false);
        std::fs::write(&source, &data).unwrap();
        let marks = [mark(Kind::Note, vec![[500., 100., 500., 100.]], "n")];
        assert!(write(&source, &marks, &source).is_err());
        let target = dir.join("paper (annotated).pdf");
        assert_eq!(write(&source, &marks, &target).unwrap().written, 1);
        assert_eq!(std::fs::read(&source).unwrap(), data);
        assert_eq!(annots(&std::fs::read(&target).unwrap()).len(), 1);
        assert!(!dir.join(".paper (annotated).pdf.part").exists());
        // A target that cannot be written leaves nothing behind.
        assert!(write(&source, &marks, &dir.join("missing").join("x.pdf")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_wrap_as_a_line_holds_them() {
        assert_eq!(wrap("", 100.), Vec::<String>::new());
        assert_eq!(wrap("one\n\ntwo", 100.), ["one", "", "two"]);
        // 11 pt Chinese letters: 9 to a line of 100 points.
        assert_eq!(wrap("一二三四五六七八九十", 100.), ["一二三四五六七八九", "十"]);
        let lines = wrap("Roots grow toward water and away from light", 100.);
        assert!(lines.len() > 1 && lines.iter().all(|l| l.chars().map(advance).sum::<f32>() * TEXT_PT <= 100.), "{lines:?}");
        // A Greek word is a word too, broken at its spaces.
        assert_eq!(wrap("αβγ δεζ", 30.), ["αβγ", "δεζ"]);
    }

    /// Each letter goes to a font that draws it everywhere: the CJK font only for that script, since macOS
    /// draws the symbols of GB2312 in it as other ideographs (→ as 於).
    #[test]
    fn letters_go_to_a_font_that_draws_them() {
        let fonts = |line: &str| runs(line).into_iter().map(|(font, run, _)| (font, run)).collect::<Vec<_>>();
        let pair = |font: Font, run: &str| (font, run.to_string());
        assert_eq!(fonts("文本 box"), [pair(Font::Song, "<6587672C>"), pair(Font::Helv, "( box)")]);
        assert_eq!(fonts("BR → x"), [pair(Font::Helv, "(BR )"), pair(Font::Symbol, "(\\256)"), pair(Font::Helv, "( x)")]);
        assert_eq!(fonts("αΔ≤"), [pair(Font::Symbol, "(aD\\243)")]);
        assert_eq!(fonts("• € ™"), [pair(Font::Helv, "(\\225 \\200 \\231)")]);
        assert_eq!(fonts("✓①"), [pair(Font::Dingbats, "(3\\254)")]);
        // Taken apart: a letter with a mark, a subscript, a sign made of two.
        assert_eq!(fonts("ā CO₂ 4℃"), [pair(Font::Helv, "(a CO2 4\\260C)")]);
        // Quotation marks and the dash stand with the words beside them.
        assert_eq!(fonts("说“好”"), [pair(Font::Song, "<8BF4201C597D201D>")]);
        assert_eq!(fonts("说“ok”—"), [pair(Font::Song, "<8BF4>"), pair(Font::Helv, "(\\223ok\\224\\227)")]);
        // Full-width punctuation stays in the CJK font, which draws it right; beyond the BMP nothing does.
        assert_eq!(fonts("！（）😀"), [pair(Font::Song, "<FF01FF08FF09>")]);
        assert_eq!(literal(&[b'a' as u16, b'(' as u16, b')' as u16, b'\\' as u16]), "(a\\(\\)\\\\)");
        // What is drawn is as wide as the fonts say: Helvetica's `B` and `R`, Symbol's arrow.
        let wide: f32 = runs("BR→").iter().map(|r| r.2).sum();
        assert!((wide - (0.667 + 0.722 + 0.987)).abs() < 1e-4, "{wide}");
    }
}
