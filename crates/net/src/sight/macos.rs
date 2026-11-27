//! macOS: the Vision framework's text recognition (`VNRecognizeTextRequest`), on the machine.

use super::{Drawn, Word};
use objc2::AnyThread;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSString};
use objc2_vision::{VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

/// The languages a page that may hold Chinese is read in: Vision reads in the first that fits, and reads the
/// English of a Chinese paper along with it.
const HAN: [&str; 3] = ["zh-Hans", "zh-Hant", "en-US"];
/// The languages a page without Chinese is read in. Told to expect Chinese, Vision reads English less well:
/// `Earler` for `Earlier`, full-width brackets, a capital in the middle of a sentence.
const LATIN: [&str; 6] = ["en-US", "fr-FR", "de-DE", "es-ES", "it-IT", "pt-BR"];

/// How many bands a page is read in, one above the other. Vision looks for lines in a picture brought down
/// to a size of its own, in which a page of print is too small: read whole, a short line between two full
/// ones (a heading, the end of a paragraph) goes missing now and then. A band of a quarter of the page shows
/// it every line.
const BANDS: usize = 4;
/// How far a band reaches into its neighbours, as a share of the page: a line the end of one band cuts is
/// whole in the next.
const OVER: f32 = 0.04;

/// The lines of text on a picture of a page, each as its words with their boxes as fractions of the picture
/// from its top left.
pub fn recognise(drawn: &Drawn) -> Result<Vec<Vec<Word>>, String> {
    let picture = bitmap(drawn);
    autoreleasepool(|_| {
        let data = NSData::with_bytes(&picture);
        let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::<VNImageOption, AnyObject>::new());
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        let known: Vec<String> = unsafe { request.supportedRecognitionLanguagesAndReturnError() }.map(|known| known.iter().map(|language| language.to_string()).collect()).unwrap_or_default();
        // Not left to Vision to tell the language line by line: it takes a short line for Arabic now and then.
        let lines = read(&handler, &request, &known, &HAN)?;
        if wisp_core::sight::latin(&lines) {
            let again = read(&handler, &request, &known, &LATIN)?;
            if !again.is_empty() { return Ok(again) }
        }
        Ok(lines)
    })
}

/// The page read band by band in the first of `languages` that fits, as far as this system knows them.
fn read(handler: &VNImageRequestHandler, request: &VNRecognizeTextRequest, known: &[String], languages: &[&str]) -> Result<Vec<Vec<Word>>, String> {
    let wanted: Vec<_> = languages.iter().filter(|language| known.iter().any(|known| known == *language)).map(|language| NSString::from_str(language)).collect();
    // A system that knows none of them reads as it does by itself.
    if !wanted.is_empty() { request.setRecognitionLanguages(&NSArray::from_retained_slice(&wanted)) }
    let asked: &VNRequest = request;
    let mut found = vec![];
    for band in 0..BANDS {
        // What is read for the band, from the page's top: its own part and a little of its neighbours'.
        let (top, bottom) = ((band as f32 / BANDS as f32 - OVER).max(0.), ((band + 1) as f32 / BANDS as f32 + OVER).min(1.));
        // Vision counts from the bottom left, and takes no region that ends a hair outside the picture.
        let from = (1. - bottom as f64).clamp(0., 1.);
        unsafe { request.setRegionOfInterest(CGRect::new(CGPoint::new(0., from), CGSize::new(1., ((bottom - top) as f64).min(1. - from)))) };
        handler.performRequests_error(&NSArray::from_slice(&[asked])).map_err(|err| format!("系统文字识别没有读成：{}", err.localizedDescription()))?;
        for observation in request.results().iter().flat_map(|found| found.iter()) {
            let Some(best) = observation.topCandidates(1).firstObject() else { continue };
            let text = best.string().to_string();
            let mut words = vec![];
            for piece in wisp_core::sight::pieces(&text) {
                // Where these letters stand: a box in fractions of the band, its origin at the band's bottom left.
                let Ok(place) = (unsafe { best.boundingBoxForRange_error(NSRange::new(piece.start, piece.len)) }) else { continue };
                let place = unsafe { place.boundingBox() };
                let (x, y, w, h) = (place.origin.x as f32, place.origin.y as f32, place.size.width as f32, place.size.height as f32);
                let down = |y: f32| top + (1. - y) * (bottom - top);
                words.push(Word { text: piece.text.to_string(), rect: [x, down(y + h), x + w, down(y)], joined: piece.joined });
            }
            if words.is_empty() { continue }
            // How far inside the band the line stands; the page's own ends cut no line.
            let (above, below) = words.iter().fold((f32::MAX, f32::MIN), |(above, below), word| (above.min(word.rect[1]), below.max(word.rect[3])));
            let inside = (if top > 0. { above - top } else { 1. }).min(if bottom < 1. { bottom - below } else { 1. });
            found.push((words, inside));
        }
    }
    Ok(wisp_core::sight::without_repeats(found))
}

/// The picture as a file Vision opens without unpacking anything: a BMP of 24 bits, its rows from the bottom up.
fn bitmap(drawn: &Drawn) -> Vec<u8> {
    let (w, h) = (drawn.width as usize, drawn.height as usize);
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&((row * h) as u32).to_le_bytes());
    // 72 dots to the inch, in dots to the metre; no palette.
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    for line in drawn.rgba.chunks_exact(w * 4).rev() {
        for pixel in line.as_chunks::<4>().0 { out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]) }
        out.resize(out.len() + row - w * 3, 0);
    }
    out
}
