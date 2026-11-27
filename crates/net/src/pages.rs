//! The pages of a PDF as pictures: for a model that reads a file whose pages
//! give no text, and for the system's own recognition (`sight`). Drawn with
//! hayro, the renderer the window shows pages with, so `wisp` draws what the
//! window draws.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::path::Path;

/// A page as it was drawn: its pixels (RGBA, on white, so every one is opaque) and the page's size in points.
pub struct Drawn {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub size: (f32, f32),
}

/// Draw the pages of a PDF that `wanted` names (from 0), `limit` at the most, each `scale(width, height)`
/// pixels to the point, and hand each to `each` with its number; how many pages the file has. A page is let
/// go of before the next is drawn. What `each` refuses stops the file.
pub fn draw(data: Vec<u8>, wanted: impl Fn(usize) -> bool, limit: usize, scale: impl Fn(f32, f32) -> f32, each: &mut dyn FnMut(usize, Drawn) -> Result<(), String>) -> Result<usize, String> {
    let pdf = Pdf::new(data).map_err(|err| format!("无法解析这个 PDF（{err:?}）。"))?;
    let (cache, interpreter, settings) = (RenderCache::new(), InterpreterSettings::default(), RenderSettings::default());
    let count = pdf.pages().len();
    for (index, page) in pdf.pages().iter().enumerate().filter(|(index, _)| wanted(*index)).take(limit) {
        let (w, h) = page.render_dimensions();
        let scale = scale(w, h);
        let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(page, &cache, &interpreter, &settings, &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE }))).map_err(|_| "这个 PDF 的页面画不出来。".to_string())?;
        each(index, Drawn { width: pixmap.width() as u32, height: pixmap.height() as u32, rgba: pixmap.data_as_u8_slice().to_vec(), size: (w, h) })?;
    }
    Ok(count)
}

/// A drawn page as a PNG without its transparency: on white, the colour kept is the colour itself.
pub fn png(drawn: Drawn) -> Result<Vec<u8>, String> {
    let picture = image::RgbaImage::from_raw(drawn.width, drawn.height, drawn.rgba).ok_or("这个 PDF 的页面画不出来。")?;
    let mut png = vec![];
    image::DynamicImage::ImageRgba8(picture).to_rgb8().write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|err| format!("页面图像写不出来：{err}"))?;
    Ok(png)
}

/// The pages of a PDF from `skip` on, at most `limit` of them, as PNG
/// pictures for a model to read a file that gives no text of its own, and
/// how many pages the file has. One and a half pixels per point: enough
/// to read print by, and small enough to send.
pub fn pictures(path: &Path, skip: usize, limit: usize) -> Result<(Vec<Vec<u8>>, usize), String> {
    pictures_of(path, |index| index >= skip, limit)
}

/// Pictures of these pages (from 0) of a PDF, in order: pages read again.
pub fn pictures_at(path: &Path, at: &[usize]) -> Result<Vec<Vec<u8>>, String> {
    let (pictures, _) = pictures_of(path, |index| at.contains(&index), at.len())?;
    if pictures.len() != at.len() { return Err("要读的页面不在这个 PDF 里。".into()) }
    Ok(pictures)
}

fn pictures_of(path: &Path, wanted: impl Fn(usize) -> bool, limit: usize) -> Result<(Vec<Vec<u8>>, usize), String> {
    let data = std::fs::read(path).map_err(|err| format!("无法读取文件：{err}"))?;
    let mut out = vec![];
    let count = draw(data, wanted, limit, |w, h| 1.5_f32.min(2000. / w.max(h).max(1.)), &mut |_, drawn| {
        out.push(png(drawn)?);
        Ok(())
    })?;
    if out.is_empty() { return Err("这个 PDF 没有页面。".into()) }
    Ok((out, count))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PDF of one page a text, each line of a text set under the one before it in Helvetica.
    pub(crate) fn pdf_saying(pages: &[&[&str]]) -> Vec<u8> {
        let mut objects: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), String::new(), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
        let mut kids = vec![];
        for lines in pages {
            let text: String = lines.iter().enumerate().map(|(n, line)| format!("BT /F1 14 Tf 72 {} Td ({line}) Tj ET\n", 700 - n * 24)).collect();
            objects.push(format!("<< /Length {} >>\nstream\n{text}endstream", text.len()));
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", objects.len()));
            kids.push(format!("{} 0 R", objects.len()));
        }
        objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len());
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![];
        for (n, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", n + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets { out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()) }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    #[test]
    fn pages_are_drawn_as_pictures() {
        let pdf = pdf_saying(&[&["Root hairs"], &["Shoots"], &["Leaves"]]);
        // Every page asked for, at the scale asked for, with ink on it; and how many the file has.
        let mut seen = vec![];
        let count = draw(pdf.clone(), |index| index != 1, 5, |_, _| 2., &mut |index, drawn| {
            let inked = drawn.rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 128).count();
            seen.push((index, drawn.width, drawn.height, drawn.size, inked > 50, drawn.rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255)));
            Ok(())
        }).unwrap();
        assert_eq!((count, seen), (3, vec![(0, 1224, 1584, (612., 792.), true, true), (2, 1224, 1584, (612., 792.), true, true)]));
        // What `each` refuses stops the file; a file that is no PDF says so.
        assert_eq!(draw(pdf.clone(), |_| true, 5, |_, _| 1., &mut |_, _| Err("够了".into())), Err("够了".to_string()));
        assert!(draw(b"not a pdf".to_vec(), |_| true, 5, |_, _| 1., &mut |_, _| Ok(())).unwrap_err().contains("无法解析"));
        // As files for a model: from a page on, or the pages named, each a PNG.
        let path = std::env::temp_dir().join(format!("wisp-pages-{}.pdf", std::process::id()));
        std::fs::write(&path, &pdf).unwrap();
        let (drawn, count) = pictures(&path, 1, 40).unwrap();
        assert_eq!((drawn.len(), count), (2, 3));
        assert!(drawn.iter().all(|png| png.starts_with(b"\x89PNG")));
        assert_eq!(image::load_from_memory(&drawn[0]).map(|picture| (picture.width(), picture.height())).ok(), Some((918, 1188)));
        assert_eq!(pictures_at(&path, &[2]).unwrap().len(), 1);
        assert!(pictures_at(&path, &[7]).is_err() && pictures(&path, 9, 40).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
