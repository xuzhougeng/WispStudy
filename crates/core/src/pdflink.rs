//! Links in a PDF: where a page's link annotations lead, the web addresses written in its text, and what a
//! reader is told of an address before it is opened. A file is not to be trusted: an address goes to the
//! browser only when it is a web page or a mail, and the reader is first shown where it truly leads, with
//! what about it may mislead (`check`). The window draws and asks; nothing here opens anything.

use crate::pdftext::{self, Mark};
use hayro_interpret::TransformExt;
use hayro_interpret::hayro_syntax::object::{Array, Dict, Name, String as PdfString};
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::kurbo::Point;
use std::ops::Range;
use url::{Host, Url};

/// A place on a page that leads to an address.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// Where it leads, as the file has it (for an address written in the text, as it is written).
    pub target: String,
    /// One box per line, in page points from the top left, as `pdftext::Mark`s stand.
    pub boxes: Vec<[f32; 4]>,
    /// The words of the page it stands on: what the reader believes it leads to.
    pub shown: String,
}

/// The most of a page one link may cover: a link over half a page would catch every click on its text.
const MOST_OF_PAGE: f32 = 0.5;
/// A box narrower or lower than this, in points, is no place to click.
const LEAST_SIDE: f32 = 1.;
/// How an address written in the text begins, in lower case; those without a scheme are read as `https://`.
const OPENERS: [&str; 5] = ["https://", "http://", "www.", "dx.doi.org/", "doi.org/"];
/// What a line may end with when the address on it goes on at the start of the next.
const BROKEN_AFTER: &str = "/-_=&?#%~";

/// The links of a page: its link annotations that lead to an address (`/URI`), then the web addresses its
/// text spells out that no annotation covers already. `marks` are the page's glyphs (`pdftext::page_marks`).
pub fn page_links(page: &Page<'_>, marks: &[Mark]) -> Vec<Link> {
    let (width, height) = page.render_dimensions();
    let annotated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| annotated(page))).unwrap_or_default();
    let mut links: Vec<Link> = annotated
        .into_iter()
        .filter(|(_, boxes)| boxes.iter().map(|b| (b[2] - b[0]) * (b[3] - b[1])).sum::<f32>() <= width * height * MOST_OF_PAGE)
        .map(|(target, boxes)| {
            let shown = shown_under(marks, &boxes);
            Link { target, boxes, shown }
        })
        .collect();
    let covered = |boxes: &[[f32; 4]]| links.iter().flat_map(|link| &link.boxes).any(|own| boxes.iter().any(|b| meet(own, b)));
    let written: Vec<Link> = written(marks).into_iter().map(|(target, from, to)| Link { boxes: pdftext::mark_boxes(&marks[from..=to]), shown: target.clone(), target }).filter(|link| !covered(&link.boxes)).collect();
    links.extend(written);
    links
}

/// Whether two boxes overlap.
fn meet(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The page's link annotations that lead to an address: each address with its boxes in page points from the
/// top left. One link set as several annotations of the same address in a row (a line each) is one.
fn annotated(page: &Page<'_>) -> Vec<(String, Vec<[f32; 4]>)> {
    let Some(annots) = page.raw().get::<Array>(b"Annots") else { return vec![] };
    let place = page.initial_transform(true).to_kurbo();
    let on_page = |points: &[(f64, f64)]| -> Option<[f32; 4]> {
        let points: Vec<Point> = points.iter().map(|(x, y)| place * Point::new(*x, *y)).collect();
        let (xs, ys) = (points.iter().map(|p| p.x as f32), points.iter().map(|p| p.y as f32));
        let b = [xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max)];
        (b.iter().all(|v| v.is_finite()) && b[2] - b[0] >= LEAST_SIDE && b[3] - b[1] >= LEAST_SIDE).then_some(b)
    };
    let mut out: Vec<(String, Vec<[f32; 4]>)> = vec![];
    for annot in annots.iter::<Dict>() {
        if annot.get::<Name>(b"Subtype").is_none_or(|name| name.as_str() != "Link") { continue }
        // Hidden (flag 2): not drawn, not to be clicked.
        if annot.get::<i64>(b"F").is_some_and(|flags| flags & 2 != 0) { continue }
        // Only an address: a link that starts a program (`Launch`) or opens another file is never followed from here.
        let Some(action) = annot.get::<Dict>(b"A") else { continue };
        if action.get::<Name>(b"S").is_none_or(|name| name.as_str() != "URI") { continue }
        let Some(target) = action.get::<PdfString>(b"URI").map(|uri| decode(uri.as_bytes())).filter(|t| !t.is_empty()) else { continue };
        let Some(rect) = annot.get::<Vec<f64>>(b"Rect").filter(|r| r.len() == 4) else { continue };
        let Some(whole) = on_page(&[(rect[0], rect[1]), (rect[2], rect[3]), (rect[0], rect[3]), (rect[2], rect[1])]) else { continue };
        // A link over several lines may give a quadrilateral for each; those outside its rectangle are not its own.
        let quads: Vec<[f32; 4]> = annot.get::<Vec<f64>>(b"QuadPoints").unwrap_or_default().as_chunks::<8>().0.iter().filter_map(|q| on_page(&[(q[0], q[1]), (q[2], q[3]), (q[4], q[5]), (q[6], q[7])])).filter(|b| meet(b, &[whole[0] - 1., whole[1] - 1., whole[2] + 1., whole[3] + 1.])).collect();
        let boxes = if quads.is_empty() { vec![whole] } else { quads };
        match out.last_mut() {
            Some((last, kept)) if *last == target => kept.extend(boxes),
            _ => out.push((target, boxes)),
        }
    }
    out
}

/// An address as the file stores it: UTF-16 after its byte order mark, else UTF-8, else one byte a letter.
fn decode(bytes: &[u8]) -> String {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        String::from_utf16_lossy(&rest.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect::<Vec<_>>())
    } else {
        match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => bytes.iter().map(|b| char::from(*b)).collect(),
        }
    };
    text.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_string()
}

/// The words of a page under some boxes: the glyphs a third of whose width is within one of them, on its
/// line. A box drawn a little short of its words (the first letter of `https` half out) still has them all.
fn shown_under(marks: &[Mark], boxes: &[[f32; 4]]) -> String {
    let under: Vec<Mark> = marks
        .iter()
        .filter(|mark| {
            let [x0, y0, x1, y1] = pdftext::mark_box(mark);
            let y = (y0 + y1) / 2.;
            boxes.iter().any(|b| y >= b[1] - 1. && y <= b[3] + 1. && x1.min(b[2]) - x0.max(b[0]) >= (x1 - x0) / 3.)
        })
        .cloned()
        .collect();
    pdftext::marks_text(&under)
}

/// The web addresses a page's text spells out: each as written (a line break inside it taken out) with the
/// first and last of its marks.
pub fn written(marks: &[Mark]) -> Vec<(String, usize, usize)> {
    // The page as one text: a line break where a line ends, a space where the page sets two glyphs apart.
    let mut text = String::new();
    let mut starts = Vec::with_capacity(marks.len());
    for (at, mark) in marks.iter().enumerate() {
        if at > 0 {
            if pdftext::starts_line(&marks[at - 1], mark) { text.push('\n') } else if pdftext::spaced(&marks[at - 1], mark) && !text.ends_with(' ') { text.push(' ') }
        }
        starts.push(text.len());
        text.push_str(&mark.text);
    }
    let mark_of = |byte: usize| starts.partition_point(|start| *start <= byte).saturating_sub(1);
    addresses(&text).into_iter().map(|range| (text[range.clone()].replace('\n', ""), mark_of(range.start), mark_of(range.end - 1))).collect()
}

/// A character an address written out may have.
fn in_address(c: char) -> bool {
    c.is_ascii_graphic() && !"<>\"{}|\\^`".contains(c)
}

/// Where a text spells out web addresses: a byte range each. One begins with a scheme, `www.` or `doi.org/`,
/// not in the middle of a word, and runs to a space or a character no address has; it goes on over a line
/// break when its line ends where an address cannot (`BROKEN_AFTER`) and the next line goes on with
/// something only an address has (a digit, a slash, a dot…). Punctuation after it is the sentence's.
pub fn addresses(text: &str) -> Vec<Range<usize>> {
    // ASCII lower case keeps every byte where it was.
    let lower = text.to_ascii_lowercase();
    let mut out = vec![];
    let mut at = 0;
    while at < text.len() {
        let rest = &lower[at..];
        // Right after Chinese is a beginning: Chinese sets no space before an address.
        let begins = !text[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || "./-_@".contains(c));
        if begins && let Some(opener) = OPENERS.iter().find(|opener| rest.starts_with(**opener)) {
            let mut end = at + opener.len();
            while let Some(c) = text[end..].chars().next() {
                if c == '\n' {
                    let ends_broken = text[at..end].chars().next_back().is_some_and(|last| BROKEN_AFTER.contains(last));
                    let next: &str = text[end + 1..].split(|c: char| c.is_whitespace()).next().unwrap_or("");
                    let next = next.trim_end_matches(|c: char| ".,;:!?'\")]".contains(c));
                    let goes_on = !next.is_empty() && next.chars().all(in_address) && next.chars().any(|c| c.is_ascii_digit() || "/.-_=&?#%~".contains(c));
                    if ends_broken && goes_on { end += 1; continue }
                    break;
                }
                if !in_address(c) { break }
                end += c.len_utf8();
            }
            // The sentence's punctuation, and a bracket closed that the address did not open.
            loop {
                let found = &text[at..end];
                let Some(last) = found.chars().next_back() else { break };
                let unbalanced = |open: char, close: char| last == close && found.matches(open).count() < found.matches(close).count();
                if ".,;:!?'\"".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') { end -= last.len_utf8() } else { break }
            }
            // A site of one word (`localhost`) is no address a paper gives.
            if parse(&text[at..end].replace('\n', "")).is_some_and(|url| url.host_str().is_some_and(|host| host.contains('.'))) {
                out.push(at..end);
                at = end;
                continue;
            }
        }
        at += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// An address as a browser reads it; one without a scheme that begins as a site does (`www.`) is a web page.
fn parse(given: &str) -> Option<Url> {
    let lower = given.to_ascii_lowercase();
    let full = if OPENERS[2..].iter().any(|opener| lower.starts_with(opener)) { format!("https://{given}") } else { given.to_string() };
    let url = Url::parse(&full).ok()?;
    // A web page names a site.
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_none_or(str::is_empty) { return None }
    Some(url)
}

/// What an address leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goes {
    /// A web page, opened in the browser.
    Web,
    /// A mail to write, opened in the mail program.
    Mail,
    /// Anything else (a file on this computer, a program, a script, no address at all): never opened from here.
    Other,
}

/// What to know of an address before opening it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The words on the page name another site than the one it leads to: that site.
    Disguised(String),
    /// A name and an `@` stand before the site, where a reader takes them for it: `https://doi.org@example.com`.
    Credentials,
    /// The site's name has letters outside the Latin alphabet, which can look like others: as it reads in them.
    Lookalike(String),
    /// This computer or the local network.
    Local,
    /// A bare number for a site.
    Numeric,
    /// Not encrypted on the way (`http`).
    Plain,
}

/// What a reader is told of an address before it is opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// The address as it would be opened: as a browser reads it, else as the file has it.
    pub url: String,
    /// Where it leads: a web page's site (with its port when it names one), a mail's address, else the kind of
    /// link (`file:`); empty when it is no address at all.
    pub site: String,
    pub goes: Goes,
    /// The gravest first.
    pub warnings: Vec<Warning>,
}

impl Verdict {
    /// Whether it may be opened from here at all.
    pub fn opens(&self) -> bool {
        self.goes != Goes::Other
    }
}

/// What a link to `target` that shows the words `shown` on the page truly leads to.
pub fn check(target: &str, shown: &str) -> Verdict {
    let given = target.trim();
    let Some(url) = parse(given) else { return Verdict { url: given.to_string(), site: String::new(), goes: Goes::Other, warnings: vec![] } };
    match url.scheme() {
        "http" | "https" => {}
        "mailto" => {
            let address = url.path().split('?').next().unwrap_or("").to_string();
            return Verdict { url: url.to_string(), site: address, goes: Goes::Mail, warnings: vec![] };
        }
        other => return Verdict { url: url.to_string(), site: format!("{other}:"), goes: Goes::Other, warnings: vec![] },
    }
    let host = url.host_str().unwrap_or("").to_string();
    let mut warnings = vec![];
    // The site the page's words name, when they spell out an address of their own.
    let named = addresses(shown).into_iter().find_map(|range| parse(&shown[range].replace('\n', ""))).and_then(|url| url.host_str().map(str::to_string));
    if let Some(named) = named.filter(|named| !same_site(named, &host)) { warnings.push(Warning::Disguised(named)) }
    if !url.username().is_empty() || url.password().is_some() { warnings.push(Warning::Credentials) }
    if host.split('.').any(|label| label.starts_with("xn--")) {
        let (own, _) = idna::domain_to_unicode(&host);
        warnings.push(Warning::Lookalike(own));
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => warnings.push(if ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() { Warning::Local } else { Warning::Numeric }),
        Some(Host::Ipv6(ip)) => {
            let first = ip.segments()[0];
            // Unique local (fc00::/7) and link-local (fe80::/10) addresses are the local network's.
            warnings.push(if ip.is_loopback() || ip.is_unspecified() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80 { Warning::Local } else { Warning::Numeric });
        }
        Some(Host::Domain(name)) if name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local") => warnings.push(Warning::Local),
        _ => {}
    }
    if url.scheme() == "http" { warnings.push(Warning::Plain) }
    let site = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Verdict { url: url.to_string(), site, goes: Goes::Web, warnings }
}

/// Whether two sites' names are one site: the same but for `www.`, or one under the other (`dx.doi.org`, `doi.org`).
fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (a.strip_prefix("www.").unwrap_or(a), b.strip_prefix("www.").unwrap_or(b));
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Document, Object, Stream, dictionary};

    /// A one-page US Letter PDF with a line of Helvetica at 72 720, the content given, and these annotations.
    fn file(content: &str, annots: Vec<lopdf::Dictionary>, rotate: Option<i64>) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding"});
        let content = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let annots: Vec<Object> = annots.into_iter().map(|annot| doc.add_object(annot).into()).collect();
        let mut page = dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => content, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Annots" => annots};
        if let Some(rotate) = rotate { page.set("Rotate", rotate) }
        let page_id = doc.add_object(page);
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1}));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut bytes = vec![];
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn link(rect: [i64; 4], action: lopdf::Dictionary) -> lopdf::Dictionary {
        dictionary! {"Type" => "Annot", "Subtype" => "Link", "Rect" => rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>(), "A" => action}
    }

    fn uri(address: &str) -> lopdf::Dictionary {
        dictionary! {"S" => "URI", "URI" => Object::string_literal(address)}
    }

    fn links_of(bytes: Vec<u8>) -> Vec<Link> {
        use hayro_interpret::InterpreterCache;
        use hayro_interpret::hayro_syntax::Pdf;
        let pdf = Pdf::new(bytes).unwrap();
        let cache = InterpreterCache::new();
        let pages = pdf.pages();
        let page = pages.first().unwrap();
        let marks = pdftext::page_marks(page, &cache);
        page_links(page, &marks)
    }

    #[test]
    fn an_annotation_leads_where_its_address_says_and_knows_the_words_under_it() {
        let words = "BT /F1 12 Tf 72 720 Td (See doi.org/10.1/abc for more.) Tj ET";
        // The rectangle in the PDF's own space, upward from the bottom: the line's baseline is at 720.
        let links = links_of(file(words, vec![link([90, 715, 200, 730], uri("https://example.com/paper"))], None));
        assert_eq!(links.len(), 1, "{links:?}");
        let found = &links[0];
        assert_eq!(found.target, "https://example.com/paper");
        // From the top left: 792 - 730 = 62 to 792 - 715 = 77.
        let [x0, y0, x1, y1] = found.boxes[0];
        assert!((x0 - 90.).abs() < 0.5 && (y0 - 62.).abs() < 0.5 && (x1 - 200.).abs() < 0.5 && (y1 - 77.).abs() < 0.5, "{:?}", found.boxes);
        assert!(found.shown.contains("doi.org"), "{:?}", found.shown);
        // The address the words spell out is the annotation's: not a link of its own beside it.
        assert!(links.iter().all(|l| l.target != "doi.org/10.1/abc"));
    }

    #[test]
    fn a_rotated_page_turns_the_link_with_its_text() {
        let words = "BT /F1 12 Tf 72 720 Td (https://example.org/x) Tj ET";
        let links = links_of(file(words, vec![link([72, 715, 200, 730], uri("https://example.org/x"))], Some(90)));
        assert_eq!(links.len(), 1, "{links:?}");
        // Turned a quarter, the line runs down the page (792 wide now, 612 high): the box is taller than wide, on it.
        let [x0, y0, x1, y1] = links[0].boxes[0];
        assert!(y1 - y0 > x1 - x0 && x0 >= 0. && y0 >= 0. && x1 <= 792. && y1 <= 612., "{:?}", links[0].boxes);
    }

    #[test]
    fn only_an_address_is_a_link() {
        let words = "BT /F1 12 Tf 72 720 Td (Nothing here.) Tj ET";
        let launch = link([72, 700, 200, 730], dictionary! {"S" => "Launch", "F" => Object::string_literal("calc.exe")});
        let go_to = link([72, 600, 200, 630], dictionary! {"S" => "GoTo", "D" => vec![Object::Integer(0)]});
        let mut hidden = link([72, 500, 200, 530], uri("https://hidden.example"));
        hidden.set("F", 2);
        // Over most of the page: it would catch every click.
        let whole = link([0, 0, 612, 792], uri("https://everywhere.example"));
        let square = dictionary! {"Type" => "Annot", "Subtype" => "Square", "Rect" => vec![72.into(), 400.into(), 200.into(), 430.into()], "A" => uri("https://square.example")};
        assert_eq!(links_of(file(words, vec![launch, go_to, hidden, whole, square], None)), vec![]);
    }

    #[test]
    fn one_address_on_two_lines_in_a_row_is_one_link() {
        let words = "BT /F1 12 Tf 72 720 Td (first line) Tj 0 -14 Td (second line) Tj ET";
        let links = links_of(file(words, vec![link([72, 716, 140, 730], uri("https://example.com")), link([72, 702, 150, 716], uri("https://example.com")), link([72, 600, 140, 620], uri("https://other.example"))], None));
        assert_eq!(links.len(), 2, "{links:?}");
        assert_eq!(links[0].boxes.len(), 2);
        assert_eq!(links[1].target, "https://other.example");
    }

    #[test]
    fn an_address_written_in_the_text_is_a_link_where_no_annotation_is() {
        let words = "BT /F1 12 Tf 72 720 Td (Code: https://github.com/x/y. Data at www.example.org) Tj ET";
        let links = links_of(file(words, vec![], None));
        let targets: Vec<&str> = links.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(targets, ["https://github.com/x/y", "www.example.org"]);
        assert!(links.iter().all(|l| l.boxes.len() == 1 && l.shown == l.target));
    }

    #[test]
    fn addresses_in_a_text() {
        let found = |text: &str| addresses(text).into_iter().map(|r| text[r].replace('\n', "")).collect::<Vec<_>>();
        assert_eq!(found("see https://example.com/a_(b), and (https://x.org/y)."), ["https://example.com/a_(b)", "https://x.org/y"]);
        assert_eq!(found("见https://example.cn/路径。下一句"), ["https://example.cn/"]);
        assert_eq!(found("Available at doi.org/10.1038/s41586 and dx.doi.org/10.1/2;"), ["doi.org/10.1038/s41586", "dx.doi.org/10.1/2"]);
        // Broken over a line where an address cannot end, it goes on; a word of the sentence does not.
        assert_eq!(found("https://doi.org/10.1038/\ns41586-020-2649-2. Next"), ["https://doi.org/10.1038/s41586-020-2649-2"]);
        assert_eq!(found("https://github.com/x/\nfor details"), ["https://github.com/x/"]);
        assert_eq!(found("ends https://example.com.\nwww.next.org"), ["https://example.com", "www.next.org"]);
        // Not in the middle of a word, not an address without a site.
        assert_eq!(found("mywww.example.com, https://, http://localhost, wwwfoo"), Vec::<String>::new());
        assert_eq!(found("e-mail a@www.example.com"), Vec::<String>::new());
    }

    #[test]
    fn a_web_page_is_told_by_its_site() {
        let verdict = check("https://doi.org/10.1038/s41586-020-2649-2", "");
        assert_eq!((verdict.goes, verdict.site.as_str(), verdict.warnings.len()), (Goes::Web, "doi.org", 0));
        assert!(verdict.opens());
        // Without a scheme, as a site begins: a web page, encrypted.
        let verdict = check("www.Example.org/a b", "");
        assert_eq!((verdict.url.as_str(), verdict.site.as_str()), ("https://www.example.org/a%20b", "www.example.org"));
        assert_eq!(check("http://example.org:8080/", "").site, "example.org:8080");
        assert_eq!(check("http://example.org/", "").warnings, [Warning::Plain]);
    }

    #[test]
    fn what_may_mislead_is_said() {
        assert_eq!(check("https://evil.example/doi.org", "https://doi.org/10.1/x").warnings, [Warning::Disguised("doi.org".into())]);
        // The same site, or one under the other, is not another.
        assert_eq!(check("https://doi.org/10.1/x", "dx.doi.org/10.1/x").warnings, []);
        assert_eq!(check("https://www.nature.com/a", "nature.com/a and www.nature.com/a").warnings, []);
        // Words that spell out no address say nothing of where it leads.
        assert_eq!(check("https://academic.oup.com/x", "Supplementary data.xlsx").warnings, []);
        let verdict = check("https://doi.org@evil.example/", "");
        assert_eq!((verdict.site.as_str(), verdict.warnings.as_slice()), ("evil.example", [Warning::Credentials].as_slice()));
        assert_eq!(check("https://аpple.com/", "").warnings, [Warning::Lookalike("аpple.com".into())]);
        assert_eq!(check("https://xn--pple-43d.com/", "").warnings, [Warning::Lookalike("аpple.com".into())]);
        assert_eq!(check("http://192.168.1.1/admin", "").warnings, [Warning::Local, Warning::Plain]);
        // A number written as browsers read it is still this computer.
        assert_eq!(check("https://2130706433/", "").warnings, [Warning::Local]);
        assert_eq!(check("https://[::1]/", "").warnings, [Warning::Local]);
        assert_eq!(check("https://localhost:3000/", "").warnings, [Warning::Local]);
        assert_eq!(check("https://8.8.8.8/", "").warnings, [Warning::Numeric]);
    }

    #[test]
    fn only_a_web_page_or_a_mail_opens() {
        let mail = check("mailto:someone@example.com?subject=Hi", "");
        assert_eq!((mail.goes, mail.site.as_str()), (Goes::Mail, "someone@example.com"));
        for (address, site) in [("file:///C:/Windows/System32/calc.exe", "file:"), ("javascript:alert(1)", "javascript:"), ("ms-msdt:/id", "ms-msdt:"), ("smb://server/share", "smb:")] {
            let verdict = check(address, "");
            assert_eq!((verdict.goes, verdict.site.as_str()), (Goes::Other, site), "{address}");
            assert!(!verdict.opens());
        }
        // No address at all: kept as given, never opened.
        let verdict = check("  ../paper.pdf ", "");
        assert_eq!((verdict.goes, verdict.site.as_str(), verdict.url.as_str()), (Goes::Other, "", "../paper.pdf"));
        assert_eq!(check("https://", "").goes, Goes::Other);
    }

    #[test]
    fn an_address_is_read_in_the_encodings_a_file_uses() {
        assert_eq!(decode(b" https://example.com/\0"), "https://example.com/");
        assert_eq!(decode(&[0xFE, 0xFF, 0, b'h', 0, b't', 0, b't', 0, b'p']), "http");
        assert_eq!(decode(&[b'h', 0xE9]), "hé");
    }
}
