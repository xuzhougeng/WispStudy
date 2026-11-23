//! What becomes of a Mermaid diagram: `cargo run -p wisp_core --example mermaid -- a.mmd [dark] > a.svg`
//! writes the SVG an answer's ```` ```mermaid ```` block is drawn from (in the colours of a light page, or
//! with `dark` of a dark one), or says why Mermaid does not read the source. Any SVG viewer shows the
//! result; what the window adds is only the pixels.

use wisp_core::mermaid::{Palette, draw};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: mermaid <file.mmd> [dark]");
        std::process::exit(2);
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("{path}: {err}");
            std::process::exit(1);
        }
    };
    let palette = if args.next().as_deref() == Some("dark") {
        Palette { dark: true, ground: "#222326".into(), panel: "#2A2B2F".into(), text: "#E6E7EA".into(), secondary: "#ABAFB7".into(), line: "#333539".into(), line_strong: "#47494F".into(), note: "#2C3A4F".into(), ..Palette::light() }
    } else {
        Palette::light()
    };
    match draw(&source, &palette) {
        Ok(drawing) => {
            eprintln!("{} x {}", drawing.width, drawing.height);
            println!("{}", drawing.svg);
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
