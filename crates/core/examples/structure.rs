//! What the structuring makes of a file, for looking at a PDF that comes out wrong:
//!
//!     cargo run -p wisp_core --example structure -- a.pdf [blocks|sentences|pages|outline|captions|figures|aloud [title [journal]]|apply <reply.txt>]
//!
//! Prints the text's language, how many sentences each part got and how
//! each part opens; with `blocks`, every block and its label; with
//! `sentences`, every sentence kept; with `pages`, the page text; with
//! `outline`, what a model is asked when it sorts the parts; with `apply`,
//! the parts a model's answer (in a file) gives; with `captions`, the
//! captions of figures and tables each page shows, as the figure library reads them; with
//! `figures`, where the file shows each figure (its caption, the piece of the page found for it) and what
//! every mention of a figure in the text leads to; with
//! `aloud`, what 朗读 sends page by page, one sentence a line (the paper's title and journal, if given, find
//! the first page's title).

use wisp_core::structure;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("usage: structure <a.pdf> [blocks|pages]"));
    let mode = args.next().unwrap_or_default();
    if mode == "captions" {
        let Some(pages) = std::fs::read(&path).ok().and_then(wisp_core::pdftext::file_marks) else { return println!("cannot read {}", path.display()) };
        for (index, marks) in pages.iter().enumerate() {
            for caption in wisp_core::figure::captions(marks) {
                println!("p{:<3} [{}] {}", index + 1, caption.label, caption.legend);
            }
        }
        return;
    }
    // Where the file shows its figures, and where its text speaks of them: what the reader makes of a click on a number.
    if mode == "figures" {
        let Some(pages) = std::fs::read(&path).ok().and_then(wisp_core::pdftext::file_drawn) else { return println!("cannot read {}", path.display()) };
        let mut index = wisp_core::figref::Index::default();
        for (at, (marks, shapes, size)) in pages.iter().enumerate() {
            let shown = wisp_core::figref::shown(marks, shapes, *size);
            let loose = wisp_core::figref::unclaimed(shapes, &shown, *size);
            index.add(at, shown, loose, size.1);
        }
        index.settle();
        let rect = |[x0, y0, x1, y1]: [f32; 4]| format!("[{x0:.0},{y0:.0} {x1:.0},{y1:.0}]");
        for place in index.places() {
            let shown = &place.shown;
            let figure = match (shown.figure, place.apart) {
                (Some(figure), _) => format!("figure {}", rect(figure)),
                (None, Some((page, figure))) => format!("figure on p{} {}", page + 1, rect(figure)),
                (None, None) => "no figure found".to_string(),
            };
            println!("p{:<3} [{}] w{} caption {} {figure}  {}", place.page + 1, shown.label, shown.weight, rect(shown.caption), shown.legend.chars().take(50).collect::<String>());
        }
        for (at, (marks, ..)) in pages.iter().enumerate() {
            for mention in wisp_core::figref::mentions(marks) {
                let text = wisp_core::pdftext::marks_text(&marks[mention.from..=mention.to]);
                let to = match index.find(mention.key) {
                    Some(place) if wisp_core::figref::own(marks, &mention, at, place) => "itself".to_string(),
                    Some(place) => format!("p{} [{}]", place.page + 1, place.shown.label),
                    None => "nowhere in this file".to_string(),
                };
                println!("p{:<3} {text:?} -> {to}", at + 1);
            }
        }
        return;
    }
    // What 译文 would translate, one line per request: the page, the box it stands in, the text.
    if mode == "passages" {
        let Some(pages) = std::fs::read(&path).ok().and_then(wisp_core::pdftext::file_marks) else { return println!("cannot read {}", path.display()) };
        for (index, passage) in wisp_core::translation::passages(&pages).iter().enumerate() {
            let [x0, y0, x1, y1] = passage.rect;
            println!("{index:4} p{:<3} {} [{x0:.0},{y0:.0} {x1:.0},{y1:.0}] {:.1}pt {}", passage.page, if passage.heading { "H" } else { "P" }, passage.size, passage.text.chars().take(90).collect::<String>());
        }
        return;
    }
    let pages = match wisp_core::fulltext::file_pages(&path) {
        Ok(pages) => pages,
        Err(err) => return println!("{err}"),
    };
    if mode == "aloud" {
        let paper = wisp_core::model::Paper { title: args.next().unwrap_or_default(), journal: args.next().unwrap_or_default(), ..Default::default() };
        for (page, piece) in wisp_core::speech::from_pages(&pages, 1, &paper) {
            println!("=================== page {page}");
            for sentence in wisp_core::speech::sentences(&piece) { println!("{}", sentence.trim()) }
        }
        return;
    }
    if mode == "pages" {
        for (index, page) in pages.iter().enumerate() {
            println!("=================== page {}\n{page}", index + 1);
        }
        return;
    }
    let blocks = structure::blocks(&pages);
    if mode == "outline" { return print!("{}", structure::outline_prompt("", &blocks)) }
    let labels = match mode.as_str() {
        "apply" => structure::parse_outline(&std::fs::read_to_string(args.next().expect("apply <reply.txt>")).expect("reply"), &blocks, "").expect("outline"),
        _ => structure::assign(&blocks, ""),
    };
    if mode == "blocks" {
        for (index, (block, label)) in blocks.iter().zip(&labels).enumerate() {
            println!("{index:4} p{:<3} {:12} {} {}", block.page, label, if block.heading { "H" } else { "P" }, block.text.chars().take(100).collect::<String>());
        }
        return;
    }
    let sentences = structure::sentences(&blocks, &labels);
    if mode == "sentences" {
        for sentence in &sentences {
            println!("{:12} p{:<3} {}", sentence.section, sentence.page, sentence.text);
        }
        return;
    }
    let (_, sure) = structure::assign_sure(&blocks, "");
    let (open, paragraphs) = (structure::open_paragraphs(&blocks, &sure), structure::prose_paragraphs(&blocks));
    println!("{} pages, {} blocks, {} sentences", pages.len(), blocks.len(), sentences.len());
    println!("language {:?}, {open} of {paragraphs} paragraphs guessed, worth asking a model: {}", structure::body_language(&blocks, &labels).unwrap_or_else(|| structure::language(&pages)), structure::worth_asking(open, paragraphs));
    for (key, name) in structure::SECTIONS {
        let part: Vec<_> = sentences.iter().filter(|s| s.section == key).collect();
        if part.is_empty() { continue }
        println!("\n{name} ({key}): {} sentences", part.len());
        for sentence in part.iter().take(3) {
            println!("  p{} [{}] {}", sentence.page, sentence.heading, sentence.text);
        }
    }
}
