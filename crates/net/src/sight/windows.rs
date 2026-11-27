//! Windows: `Windows.Media.Ocr`, the recognition that comes with a language of the system.

use super::{Drawn, Word};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// The lines of text on a picture of a page, each as its words with their boxes as fractions of the picture
/// from its top left.
pub fn recognise(drawn: &Drawn) -> Result<Vec<Vec<Word>>, String> {
    let engine = engine()?;
    let failed = |err: windows::core::Error| format!("系统文字识别没有读成：{}", err.message());
    // The picture as Windows takes it: blue first.
    let mut pixels = drawn.rgba.clone();
    for pixel in pixels.as_chunks_mut::<4>().0 { pixel.swap(0, 2) }
    let writer = DataWriter::new().map_err(failed)?;
    writer.WriteBytes(&pixels).map_err(failed)?;
    let buffer = writer.DetachBuffer().map_err(failed)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, drawn.width as i32, drawn.height as i32, BitmapAlphaMode::Ignore).map_err(failed)?;
    let read = engine.RecognizeAsync(&bitmap).map_err(failed)?.get().map_err(failed)?;
    let (w, h) = (drawn.width.max(1) as f32, drawn.height.max(1) as f32);
    let mut lines = vec![];
    for line in read.Lines().map_err(failed)? {
        let mut words: Vec<Word> = vec![];
        for word in line.Words().map_err(failed)? {
            let text = word.Text().map_err(failed)?.to_string();
            let place = word.BoundingRect().map_err(failed)?;
            // A letter of Chinese or Japanese comes as a word of its own: written after the one before it, not set apart.
            let joined = words.last().is_some_and(|last| wisp_core::sight::written_together(&last.text, &text));
            words.push(Word { text, rect: [place.X / w, place.Y / h, (place.X + place.Width) / w, (place.Y + place.Height) / h], joined });
        }
        if !words.is_empty() { lines.push(words) }
    }
    Ok(lines)
}

/// What reads: the first of the person's languages Windows has a recognition for, else any it has.
fn engine() -> Result<OcrEngine, String> {
    OcrEngine::TryCreateFromUserProfileLanguages()
        .ok()
        .or_else(|| OcrEngine::AvailableRecognizerLanguages().ok()?.into_iter().find_map(|language| OcrEngine::TryCreateFromLanguage(&language).ok()))
        .ok_or_else(|| "Windows 没有装任何语言的文字识别：在「设置 › 时间和语言 › 语言和区域」里给一种语言加上「光学字符识别」，再试一次。".to_string())
}
