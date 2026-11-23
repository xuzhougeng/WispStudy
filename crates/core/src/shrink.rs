//! A stored PDF made smaller (#242): a paper of 30 pages can weigh 100 MB
//! when its figures are kept as the camera or the scanner gave them.
//!
//! What is written anew is the pictures, nothing else: an image larger
//! than a page needs is scaled down to `LONGEST` pixels on its longer side
//! (about 200 dpi across an A4 page), and a photograph is kept as a JPEG of
//! `QUALITY`. A drawing, a chart, a screenshot (whatever a JPEG would not
//! make clearly smaller) stays lossless. Then the objects nothing points at
//! are left out and the streams kept bare are packed. Text, fonts, pages,
//! links and the places on a page stay as they were, so the marks, the
//! text read off the file and the places read at still fit it.
//!
//! An image is left alone when it would not come out clearly smaller, or
//! when writing it anew could change how it looks: a colour space other
//! than gray or RGB (CMYK, an indexed palette, spot colours), other than 8
//! bits a component, a `Decode` array, a colour-key mask, a mask the image
//! is blended against (`Matte`). A PDF with a password is not touched: its
//! protection would not survive. The result is checked before it is given
//! back: the same pages, and the same text on each of them.

use anyhow::{Context as _, Result, anyhow, bail};
use image::{DynamicImage, ExtendedColorType, ImageFormat, imageops::FilterType};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{HashSet, VecDeque};
use std::io::Write as _;

/// The longest side an image keeps, in pixels.
pub const LONGEST: u32 = 2400;
/// The quality of an image kept as a JPEG.
pub const QUALITY: u8 = 80;
/// A PDF this heavy is offered the 压缩 button beside its name; any PDF can be made smaller from its menu.
pub const HEAVY: u64 = 10 * 1024 * 1024;
/// An image stream smaller than this is not worth writing anew.
const SMALL: usize = 24 * 1024;

/// A PDF made smaller: its bytes, and what was done.
#[derive(Debug)]
pub struct Shrunk {
    pub bytes: Vec<u8>,
    /// The size of the file it came from.
    pub before: usize,
    /// The images written anew.
    pub images: usize,
}

impl Shrunk {
    /// Whether keeping it is worth it: at least a tenth smaller. Less than
    /// that is not worth a file that differs from the publisher's.
    pub fn worth(&self) -> bool {
        self.bytes.len() * 10 <= self.before * 9
    }
}

/// `data` made smaller, checked to read as it did. An error is a sentence
/// for the person.
pub fn shrink(data: &[u8]) -> Result<Shrunk> {
    let mut doc = Document::load_mem(data).map_err(|err| anyhow!("无法解析这个 PDF 的结构（{err}）。"))?;
    if doc.trailer.get(b"Encrypt").is_ok() { bail!("这个 PDF 带有密码或权限保护，压缩会去掉它的保护，所以没有压缩。") }
    let pages = doc.get_pages().len();
    if pages == 0 { bail!("这个 PDF 里找不到页面。") }
    let images = shrink_images(&mut doc);
    for id in unused(&doc) { doc.objects.remove(&id); }
    doc.compress();
    let mut bytes = vec![];
    doc.save_to(&mut bytes).context("无法写出压缩后的 PDF")?;
    let shrunk = Shrunk { bytes, before: data.len(), images };
    check(data, &shrunk.bytes, pages)?;
    Ok(shrunk)
}

/// Whether the new file reads as the old one: the same number of pages to
/// both readers, and the same text on each page.
fn check(before: &[u8], after: &[u8], pages: usize) -> Result<()> {
    const BROKEN: &str = "压缩后的文件和原文件读起来不一样，没有保存。";
    let parsed = Document::load_mem(after).map_err(|_| anyhow!(BROKEN))?;
    if parsed.get_pages().len() != pages { bail!(BROKEN) }
    let read = |data: &[u8]| std::panic::catch_unwind(|| crate::pdftext::pages(data.to_vec())).unwrap_or_else(|_| Err(String::new()));
    match (read(before), read(after)) {
        (Ok(old), Ok(new)) if old == new => Ok(()),
        // What the viewer could not read before is not asked to read the same now; it still has to open.
        (Err(_), Ok(new)) if new.len() == pages => Ok(()),
        (Err(_), Err(_)) => Ok(()),
        _ => bail!(BROKEN),
    }
}

/// Write the images anew where it pays; how many were.
fn shrink_images(doc: &mut Document) -> usize {
    // Soft masks are kept lossless, and those a picture is blended against at its own size are left alone.
    let (mut masks, mut matted) = (HashSet::new(), HashSet::new());
    for (&id, object) in &doc.objects {
        let Some(dict) = image_dict(object) else { continue };
        if let Ok(mask) = dict.get(b"SMask").and_then(Object::as_reference) {
            masks.insert(mask);
            if doc.get_object(mask).ok().and_then(image_dict).is_some_and(|mask| mask.has(b"Matte")) { matted.extend([id, mask]); }
        }
    }
    let ids: Vec<ObjectId> = doc.objects.iter().filter(|(id, object)| !matted.contains(*id) && image_dict(object).is_some()).map(|(id, _)| *id).collect();
    // One image a core: decoding and encoding are the time this takes.
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8);
    let queue = std::sync::Mutex::new(ids.iter());
    let doc_ref = &*doc;
    let done: Vec<(ObjectId, Written)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (queue, masks) = (&queue, &masks);
                scope.spawn(move || {
                    let mut done = vec![];
                    // The lock is let go inside `and_then`, before the image is worked on.
                    while let Some(&id) = queue.lock().ok().and_then(|mut q| q.next()) {
                        let Ok(Object::Stream(stream)) = doc_ref.get_object(id) else { continue };
                        let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rewrite(doc_ref, stream, masks.contains(&id)))).ok().flatten();
                        if let Some(written) = written { done.push((id, written)) }
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let count = done.len();
    for (id, written) in done {
        let Some(Object::Stream(stream)) = doc.objects.get_mut(&id) else { continue };
        stream.dict.remove(b"DecodeParms");
        stream.dict.set("Filter", Object::Name(written.filter.to_vec()));
        stream.dict.set("Width", written.width as i64);
        stream.dict.set("Height", written.height as i64);
        stream.dict.set("BitsPerComponent", 8);
        stream.set_content(written.bytes);
    }
    count
}

/// The dictionary of an image XObject.
fn image_dict(object: &Object) -> Option<&Dictionary> {
    let Object::Stream(stream) = object else { return None };
    (stream.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice())).then_some(&stream.dict)
}

/// An image written anew.
struct Written {
    bytes: Vec<u8>,
    filter: &'static [u8],
    width: u32,
    height: u32,
}

/// The image of `stream` written smaller, when it can be without changing how it looks.
fn rewrite(doc: &Document, stream: &Stream, mask: bool) -> Option<Written> {
    let dict = &stream.dict;
    let size = stream.content.len();
    if size < SMALL { return None }
    let number = |key: &[u8]| dict.get(key).ok().and_then(|o| doc.dereference(o).ok()).and_then(|(_, o)| o.as_i64().ok());
    if dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) || dict.has(b"Decode") || matches!(dict.get(b"Mask"), Ok(Object::Array(_))) { return None }
    if number(b"BitsPerComponent") != Some(8) { return None }
    let (width, height) = (u32::try_from(number(b"Width")?).ok()?, u32::try_from(number(b"Height")?).ok()?);
    if width == 0 || height == 0 { return None }
    let channels = channels(doc, dict.get(b"ColorSpace").ok()?)?;
    if mask && channels != 1 { return None }
    let filters: Vec<&[u8]> = if dict.has(b"Filter") { stream.filters().ok()? } else { vec![] };
    let jpeg = filters == [b"DCTDecode".as_slice()];
    let pixels = if jpeg {
        if dict.has(b"DecodeParms") { return None }
        let picture = image::load_from_memory_with_format(&stream.content, ImageFormat::Jpeg).ok()?;
        if (picture.width(), picture.height()) != (width, height) || picture.color().channel_count() != channels { return None }
        if channels == 1 { picture.into_luma8().into_raw() } else { picture.into_rgb8().into_raw() }
    } else {
        if !filters.iter().all(|f| [b"FlateDecode".as_slice(), b"LZWDecode", b"ASCII85Decode"].contains(f)) { return None }
        // The TIFF predictor is one lopdf does not undo.
        if dict.get(b"DecodeParms").and_then(Object::as_dict).and_then(|p| p.get(b"Predictor")).and_then(Object::as_i64).is_ok_and(|p| p == 2) { return None }
        let mut raw = if filters.is_empty() { stream.content.clone() } else { stream.decompressed_content().ok()? };
        let need = width as usize * height as usize * channels as usize;
        if raw.len() < need { return None }
        raw.truncate(need);
        raw
    };
    let (to_width, to_height) = fitted(width, height);
    let smaller = (to_width, to_height) != (width, height);
    // A mask at its own size gains nothing: it stays lossless.
    if mask && !smaller { return None }
    let pixels = if smaller { scaled(pixels, width, height, channels, to_width, to_height)? } else { pixels };
    let kind = if channels == 1 { ExtendedColorType::L8 } else { ExtendedColorType::Rgb8 };
    let as_jpeg = || {
        let mut out = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY).encode(&pixels, to_width, to_height, kind).ok().map(|()| out)
    };
    let as_flate = || {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&pixels).ok()?;
        encoder.finish().ok()
    };
    let (bytes, filter): (Vec<u8>, &'static [u8]) = if mask {
        (as_flate()?, b"FlateDecode")
    } else if jpeg {
        (as_jpeg()?, b"DCTDecode")
    } else {
        // Lossless unless a JPEG is less than half: then it is a photograph, and a JPEG does not show.
        let lossless = if smaller { as_flate()? } else { stream.content.clone() };
        match as_jpeg() {
            Some(jpeg) if jpeg.len() * 2 <= lossless.len() => (jpeg, b"DCTDecode"),
            _ if smaller => (lossless, b"FlateDecode"),
            _ => return None,
        }
    };
    (bytes.len() * 10 <= size * 9).then_some(Written { bytes, filter, width: to_width, height: to_height })
}

/// The components of a pixel in a colour space written anew as it is: gray or RGB.
fn channels(doc: &Document, space: &Object) -> Option<u8> {
    let (_, space) = doc.dereference(space).ok()?;
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            _ => None,
        },
        Object::Array(parts) => match parts.first()?.as_name().ok()? {
            b"ICCBased" => {
                let (_, profile) = doc.dereference(parts.get(1)?).ok()?;
                match profile.as_stream().ok()?.dict.get(b"N").and_then(Object::as_i64).ok()? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                }
            }
            b"CalGray" => Some(1),
            b"CalRGB" => Some(3),
            _ => None,
        },
        _ => None,
    }
}

/// The size an image is kept at: its own, or scaled down to `LONGEST` on its longer side.
pub fn fitted(width: u32, height: u32) -> (u32, u32) {
    let longer = width.max(height);
    if longer <= LONGEST { return (width, height) }
    let scale = LONGEST as f64 / longer as f64;
    (((width as f64 * scale).round() as u32).max(1), ((height as f64 * scale).round() as u32).max(1))
}

fn scaled(pixels: Vec<u8>, width: u32, height: u32, channels: u8, to_width: u32, to_height: u32) -> Option<Vec<u8>> {
    let picture = if channels == 1 { DynamicImage::ImageLuma8(image::GrayImage::from_raw(width, height, pixels)?) } else { DynamicImage::ImageRgb8(image::RgbImage::from_raw(width, height, pixels)?) };
    let picture = picture.resize_exact(to_width, to_height, FilterType::Triangle);
    Some(if channels == 1 { picture.into_luma8().into_raw() } else { picture.into_rgb8().into_raw() })
}

/// The objects nothing reached from the trailer points at: pictures replaced
/// by an earlier edit, objects of an incremental update that were let go.
fn unused(doc: &Document) -> Vec<ObjectId> {
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut todo: VecDeque<&Object> = doc.trailer.iter().map(|(_, v)| v).collect();
    while let Some(object) = todo.pop_front() {
        match object {
            Object::Reference(id) => {
                if seen.insert(*id) && let Some(target) = doc.objects.get(id) { todo.push_back(target) }
            }
            Object::Array(items) => todo.extend(items.iter()),
            Object::Dictionary(dict) => todo.extend(dict.iter().map(|(_, v)| v)),
            Object::Stream(stream) => todo.extend(stream.dict.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    doc.objects.keys().filter(|id| !seen.contains(id)).copied().collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::dictionary;

    /// A one-page PDF with a line of text and one picture of `width` × `height` pixels: a photograph or a
    /// drawing, kept as a JPEG or lossless.
    pub(crate) fn paper(width: u32, height: u32, photo: bool, as_jpeg: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"});
        // A photograph is noise over a gradient, a drawing is a few flat blocks.
        let pixels: Vec<u8> = (0..height).flat_map(|y| (0..width).flat_map(move |x| {
            let noise = ((x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)) >> 7) as u8 % 48;
            if photo { [(x * 200 / width) as u8 + noise, (y * 200 / height) as u8 + noise, 90 + noise] } else if (x / 300 + y / 300) % 2 == 0 { [255, 255, 255] } else { [20, 60, 160] }
        })).collect();
        let image = if as_jpeg {
            let mut out = vec![];
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98).encode(&pixels, width, height, ExtendedColorType::Rgb8).unwrap();
            Stream::new(dictionary! {"Type" => "XObject", "Subtype" => "Image", "Width" => width as i64, "Height" => height as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode"}, out)
        } else {
            let mut stream = Stream::new(dictionary! {"Type" => "XObject", "Subtype" => "Image", "Width" => width as i64, "Height" => height as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8}, pixels);
            stream.compress().unwrap();
            stream
        };
        let image = doc.add_object(image);
        let content = b"BT /F1 12 Tf 72 720 Td (Roots grow toward water.) Tj ET q 400 0 0 300 100 300 cm /Im1 Do Q".to_vec();
        let content = doc.add_object(Stream::new(dictionary! {}, content));
        let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Contents" => content, "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}, "XObject" => dictionary! {"Im1" => image}}});
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1}));
        // An object nothing points at, as an edit leaves behind.
        doc.add_object(Stream::new(dictionary! {}, vec![7; 50_000]).with_compression(false));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn picture(bytes: &[u8]) -> Dictionary {
        let doc = Document::load_mem(bytes).unwrap();
        doc.objects.values().find_map(image_dict).unwrap().clone()
    }

    #[test]
    fn a_large_photograph_is_scaled_and_kept_as_jpeg() {
        let before = paper(4800, 3600, true, false);
        let shrunk = shrink(&before).unwrap();
        assert!(shrunk.worth(), "{} → {}", shrunk.before, shrunk.bytes.len());
        assert_eq!(shrunk.images, 1);
        let image = picture(&shrunk.bytes);
        assert_eq!((image.get(b"Width").unwrap().as_i64().unwrap(), image.get(b"Height").unwrap().as_i64().unwrap()), (2400, 1800));
        assert_eq!(image.get(b"Filter").unwrap().as_name().unwrap(), b"DCTDecode");
        // The text is what it was.
        assert_eq!(crate::pdftext::pages(shrunk.bytes.clone()).unwrap(), crate::pdftext::pages(before).unwrap());
    }

    #[test]
    fn a_drawing_stays_lossless() {
        let shrunk = shrink(&paper(4800, 3600, false, false)).unwrap();
        let image = picture(&shrunk.bytes);
        assert_eq!(image.get(b"Filter").unwrap().as_name().unwrap(), b"FlateDecode");
        assert_eq!(image.get(b"Width").unwrap().as_i64().unwrap(), 2400);
    }

    #[test]
    fn a_heavy_jpeg_at_its_size_is_written_lighter() {
        let shrunk = shrink(&paper(2000, 1500, true, true)).unwrap();
        assert_eq!(shrunk.images, 1);
        assert_eq!(picture(&shrunk.bytes).get(b"Width").unwrap().as_i64().unwrap(), 2000);
    }

    #[test]
    fn a_small_file_is_not_worth_it() {
        let shrunk = shrink(&paper(300, 200, false, false)).unwrap();
        assert_eq!(shrunk.images, 0);
        // The object nothing pointed at is gone.
        assert!(shrunk.bytes.len() < shrunk.before);
    }

    #[test]
    fn what_is_not_a_pdf_or_is_locked_is_refused() {
        assert!(shrink(b"not a pdf").is_err());
        let mut doc = Document::load_mem(&paper(300, 200, true, false)).unwrap();
        doc.trailer.set("Encrypt", dictionary! {"Filter" => "Standard"});
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        assert!(shrink(&bytes).unwrap_err().to_string().contains("保护"));
    }

    #[test]
    fn fitted_keeps_the_proportions() {
        assert_eq!(fitted(1200, 800), (1200, 800));
        assert_eq!(fitted(9600, 2400), (2400, 600));
        assert_eq!(fitted(100, 24000), (10, 2400));
    }
}
