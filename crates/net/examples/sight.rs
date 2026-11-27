//! What the system's own text recognition reads off a PDF: `cargo run -p wisp_net --example sight -- FILE.pdf`
//! prints each page's text as it would be kept; `--lines` prints the lines as the system gave them instead,
//! each with the box it stands in (points from the page's top left), in the order they are read. For a page
//! whose text comes out in the wrong order or cut in the wrong places.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: sight FILE.pdf [--lines]");
        std::process::exit(2);
    };
    let lines = args.iter().any(|arg| arg == "--lines");
    let file = std::fs::read(path).unwrap_or_else(|err| {
        eprintln!("{path}: {err}");
        std::process::exit(1);
    });
    let started = std::time::Instant::now();
    let pages = wisp_net::sight::seen(&file).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });
    for (number, seen) in pages.iter().enumerate() {
        println!("===== page {} ({} × {} pt, {} lines)", number + 1, seen.size[0], seen.size[1], seen.lines.len());
        if lines {
            let marks = wisp_core::sight::marks(seen);
            for ([x0, y0, x1, y1], text) in wisp_core::pdftext::mark_boxes(&marks).iter().zip(line_texts(&marks)) {
                println!("[{x0:6.1} {y0:6.1} {x1:6.1} {y1:6.1}] {text}");
            }
        } else {
            println!("{}", wisp_core::sight::text(seen));
        }
    }
    eprintln!("{} pages in {:.1} s", pages.len(), started.elapsed().as_secs_f32());
}

/// The text of each line of a page's marks, cut where `mark_boxes` starts a box: two marks stand on one line
/// when it draws one box around them.
fn line_texts(marks: &[wisp_core::pdftext::Mark]) -> Vec<String> {
    let mut lines: Vec<Vec<wisp_core::pdftext::Mark>> = vec![];
    for mark in marks {
        match lines.last_mut().filter(|line| wisp_core::pdftext::mark_boxes(&[line[line.len() - 1].clone(), mark.clone()]).len() == 1) {
            Some(line) => line.push(mark.clone()),
            None => lines.push(vec![mark.clone()]),
        }
    }
    lines.iter().map(|line| wisp_core::pdftext::marks_text(line)).collect()
}
