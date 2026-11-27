//! The system's own text recognition: macOS Vision and Windows.Media.Ocr read
//! pictures of a PDF's pages (`pages::draw`), and `wisp_core::sight` makes a
//! page's text of the words they saw and where. No key and no bill, and the
//! file stays on the machine.

use crate::pages::{self, Drawn};
use wisp_core::ocr;
use wisp_core::sight::{Seen, Word};

#[cfg(target_os = "macos")]
#[path = "sight/macos.rs"]
mod system;
#[cfg(target_os = "windows")]
#[path = "sight/windows.rs"]
mod system;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod system {
    /// No recognition of the system's own here: the same sentence `ocr::ready` has for it.
    pub fn recognise(_: &super::Drawn) -> Result<Vec<Vec<super::Word>>, String> {
        Err("这个系统没有自带的文字识别（macOS 和 Windows 有）：请用 DeepSeek OCR 或看图模型。".into())
    }
}

/// The most pixels along a page's longer side: Windows reads no picture larger (`OcrEngine::MaxImageDimension`).
const LONGEST: f32 = 2600.;
/// Pixels to the point a page is drawn with to be read: a scan of 300 dots to the inch keeps most of what it has.
const SCALE: f32 = 3.;

/// One page as the system sees it: the lines of its picture, their words placed in points from the page's top left.
fn see(drawn: &Drawn) -> Result<Seen, String> {
    let (w, h) = drawn.size;
    // To a tenth of a point: what is kept of a page stays small.
    let tenth = |n: f32| (n * 10.).round() / 10.;
    let mut lines = system::recognise(drawn)?;
    for word in lines.iter_mut().flatten() {
        let [x0, y0, x1, y1] = word.rect.map(|n| n.clamp(0., 1.));
        word.rect = [tenth(x0 * w), tenth(y0 * h), tenth(x1 * w), tenth(y1 * h)];
    }
    Ok(Seen { size: [w, h], lines })
}

/// The pages of a PDF as the system reads them, each drawn and read in turn.
pub fn read(file: &[u8]) -> Result<Vec<ocr::Page>, String> {
    Ok(seen(file)?.iter().map(wisp_core::sight::page).collect())
}

/// The pages of a PDF as the system sees them.
pub fn seen(file: &[u8]) -> Result<Vec<Seen>, String> {
    let mut out = vec![];
    pages::draw(file.to_vec(), |_| true, usize::MAX, |w, h| SCALE.min(LONGEST / w.max(h).max(1.)), &mut |_, drawn| {
        out.push(see(&drawn)?);
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_reads_the_pages_of_a_file() {
        let pdf = crate::pages::tests::pdf_saying(&[&["Root hairs grow longer", "when phosphate is scarce."], &["Shoots stay short."]]);
        let read = read(&pdf);
        if !ocr::system_reads() {
            assert!(read.unwrap_err().contains("没有自带的文字识别"));
            return;
        }
        // A machine without a language to read by (a Windows with none installed) says so, and that is all there is to test on it.
        let pages = match read {
            Ok(pages) => pages,
            Err(why) => return assert!(why.contains("文字识别"), "{why}"),
        };
        assert_eq!(pages.len(), 2);
        let bare = |text: &str| wisp_core::fulltext::bare(text);
        assert!(bare(&pages[0].text).contains("roothairsgrowlonger") && bare(&pages[0].text).contains("phosphateisscarce"), "{:?}", pages[0].text);
        assert!(bare(&pages[1].text).contains("shootsstayshort"), "{:?}", pages[1].text);
        // Two lines of the page as two lines of its text, in their order; and the words kept with their places.
        assert!(pages[0].text.find("Root").unwrap() < pages[0].text.find("when").unwrap() && pages[0].text.lines().count() >= 2, "{:?}", pages[0].text);
        let kept = wisp_core::sight::kept(&pages[0].raw).unwrap();
        assert_eq!(kept.size, [612., 792.]);
        // Set at 72 points from the left, its baseline 92 from the top: found thereabouts.
        let first = &kept.lines[0][0];
        assert!(first.text.starts_with("Root") && (first.rect[0] - 72.).abs() < 6. && first.rect[1] > 70. && first.rect[3] < 100., "{first:?}");
    }
}
