//! A PDF made smaller as the 压缩 button makes it, to look at:
//!
//!     cargo run -p wisp_core --release --example shrink -- in.pdf [out.pdf]   written smaller, with the sizes and the time
//!     cargo run -p wisp_core --release --example shrink -- in.pdf --largest   the heaviest objects of a file, and what they are

use lopdf::{Document, Object};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else { return eprintln!("usage: shrink <in.pdf> [out.pdf | --largest]") };
    let data = std::fs::read(input).expect("read the file");
    if args.get(1).is_some_and(|a| a == "--largest") { return largest(&data) }
    let output = args.get(1).cloned().unwrap_or_else(|| input.trim_end_matches(".pdf").to_string() + ".smaller.pdf");
    let start = std::time::Instant::now();
    match wisp_core::shrink::shrink(&data) {
        Ok(shrunk) => {
            let mb = |n: usize| n as f64 / 1e6;
            println!("{:.1} MB → {:.1} MB, {} images written anew, {:.1}s, worth keeping: {}", mb(shrunk.before), mb(shrunk.bytes.len()), shrunk.images, start.elapsed().as_secs_f64(), shrunk.worth());
            std::fs::write(&output, &shrunk.bytes).expect("write the result");
            println!("{output}");
        }
        Err(err) => eprintln!("{err:#}"),
    }
}

fn largest(data: &[u8]) {
    let doc = Document::load_mem(data).expect("parse the file");
    let mut streams: Vec<_> = doc.objects.iter().filter_map(|(id, o)| o.as_stream().ok().map(|s| (s.content.len(), id, s))).collect();
    streams.sort_by_key(|(size, ..)| std::cmp::Reverse(*size));
    let name = |s: &lopdf::Stream, key: &[u8]| s.dict.get(key).ok().map(|o| match o { Object::Name(n) => String::from_utf8_lossy(n).to_string(), Object::Integer(i) => i.to_string(), Object::Reference(r) => format!("{} {} R", r.0, r.1), other => format!("{other:?}").chars().take(40).collect() }).unwrap_or_default();
    for (size, id, s) in streams.iter().take(25) {
        println!("{:>10}  {:>6} {}  {} {} {}x{} bpc={} cs={} filter={} smask={}", size, id.0, id.1, name(s, b"Type"), name(s, b"Subtype"), name(s, b"Width"), name(s, b"Height"), name(s, b"BitsPerComponent"), name(s, b"ColorSpace"), name(s, b"Filter"), name(s, b"SMask"));
    }
    println!("{} streams, {} bytes in all", streams.len(), streams.iter().map(|(size, ..)| size).sum::<usize>());
}
