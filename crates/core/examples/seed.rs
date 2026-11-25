//! Fill a data folder with two sample libraries, for trying the app out:
//!
//!     cargo run -p wisp_core --example seed -- <data folder> [a.pdf] [more.pdf …] [--supplement s1.pdf …]
//!
//! The first PDF is attached to the first paper of the first library; any
//! further ones become papers of the second library, each with its PDF as
//! full text, so 写作 has sentences to show. PDFs after `--supplement` are
//! the first paper's attachments S1, S2, … The first paper of the second
//! library gets a figure, methods (one linked to it) and introductions.

use wisp_core::annotation::{Kind, TextStyle};
use wisp_core::figure::Figure;
use wisp_core::libraries;
use wisp_core::excerpt::{self, Excerpt};
use wisp_core::model::{FieldDef, FieldKind, Paper};
use wisp_core::store::Store;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let supplements: Vec<std::path::PathBuf> = match args.iter().position(|a| a == "--supplement") {
        Some(at) => args.split_off(at).into_iter().skip(1).map(std::path::PathBuf::from).collect(),
        None => vec![],
    };
    let mut args = args.into_iter();
    let root = std::path::PathBuf::from(args.next().expect("usage: seed <data folder> [a.pdf]"));
    let pdf = args.next().map(std::path::PathBuf::from);
    let more: Vec<std::path::PathBuf> = args.map(std::path::PathBuf::from).collect();
    std::fs::create_dir_all(&root)?;

    let llm = libraries::create(&root, "大语言模型", "Transformer 架构、预训练语言模型及其评测方法；关注注意力机制的改进与长上下文建模。", "violet")?;
    let mut store = Store::open(&llm.dir)?;
    let mut method = FieldDef { name: "方法".into(), kind: FieldKind::Tags, options: vec!["Transformer".into(), "RNN".into(), "预训练".into()], in_table: true, ..Default::default() };
    store.save_field(&mut method)?;
    let mut level = FieldDef { name: "重要程度".into(), kind: FieldKind::Select, options: vec!["必读".into(), "参考".into(), "了解".into()], in_table: true, ..Default::default() };
    store.save_field(&mut level)?;
    let papers = [
        ("Attention Is All You Need", "Vaswani, Ashish; Shazeer, Noam; Parmar, Niki; Uszkoreit, Jakob; Jones, Llion; Gomez, Aidan N.; Kaiser, Lukasz; Polosukhin, Illia", 2017, "Advances in Neural Information Processing Systems", "10.48550/arxiv.1706.03762", "The dominant sequence transduction models are based on complex recurrent or convolutional neural networks that include an encoder and a decoder. We propose a new simple network architecture, the Transformer, based solely on attention mechanisms, dispensing with recurrence and convolutions entirely.", "preprint", vec!["注意力", "架构"], "Transformer", "必读", "read"),
        ("BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding", "Devlin, Jacob; Chang, Ming-Wei; Lee, Kenton; Toutanova, Kristina", 2019, "NAACL-HLT", "10.18653/v1/n19-1423", "We introduce a new language representation model called BERT, which stands for Bidirectional Encoder Representations from Transformers.", "published", vec!["预训练"], "Transformer, 预训练", "必读", "skimmed"),
        ("Long Short-Term Memory", "Hochreiter, Sepp; Schmidhuber, Jürgen", 1997, "Neural Computation", "10.1162/neco.1997.9.8.1735", "Learning to store information over extended time intervals by recurrent backpropagation takes a very long time, mostly because of insufficient, decaying error backflow.", "published", vec!["架构"], "RNN", "参考", "unread"),
        ("Language Models are Few-Shot Learners", "Brown, Tom B.; Mann, Benjamin; Ryder, Nick; Subbiah, Melanie", 2020, "Advances in Neural Information Processing Systems", "", "We show that scaling up language models greatly improves task-agnostic, few-shot performance.", "published", vec!["预训练", "评测"], "Transformer, 预训练", "", "unread"),
        ("一篇只有题目的条目：等待补全信息", "", 0, "", "", "", "", vec![], "", "", "unread"),
    ];
    let mut first = None;
    for (i, (title, authors, year, journal, doi, abstract_text, status, tags, methods, importance, reading)) in papers.into_iter().enumerate() {
        let mut paper = Paper { title: title.into(), authors: authors.into(), year: (year > 0).then_some(year), journal: journal.into(), doi: doi.into(), abstract_text: abstract_text.into(), publication_status: status.into(), tags: tags.into_iter().map(String::from).collect(), reading: reading.into(), starred: i == 0, source: "示例数据".into(), ..Default::default() };
        if !methods.is_empty() { paper.fields.insert(method.id.clone(), methods.split(", ").map(String::from).collect()); }
        if !importance.is_empty() { paper.fields.insert(level.id.clone(), vec![importance.into()]); }
        if i == 0 {
            paper.notes = "编码器和解码器都只用注意力，训练可以并行。\n位置编码用的是正弦函数。".into();
            paper.decide("included", "奠基性工作");
        }
        // Spread the added dates out so "newest first" has an order.
        paper.added_at = format!("2026-11-{:02}T08:00:00.000Z", 10 - i);
        let added = store.add_paper(paper)?;
        if i == 0 { first = Some(added.id) }
    }
    if let (Some(id), Some(pdf)) = (first.as_ref(), pdf) {
        let file = store.import_file(id, &pdf, "fulltext")?;
        let text = wisp_core::fulltext::file_pages(&pdf);
        store.save_fulltext_pages(&file.id, &text)?;
        // Reading marks, for 全库查找 to find: the first long sentence of the first page highlighted with a note,
        // a sticky note there, a text box on the last page.
        let sentence = text.ok().and_then(|pages| pages.first()?.split(". ").map(str::trim).find(|s| s.len() >= 40).map(String::from));
        if let (Some(pages), Some(sentence)) = (wisp_core::pdftext::file_marks(std::fs::read(&pdf)?), sentence) {
            let glyphs: Vec<String> = pages[0].iter().map(|m| m.text.clone()).collect();
            let Some((range, _)) = wisp_core::fulltext::find_quote(&glyphs, &[], &sentence) else { anyhow::bail!("the first sentence is not among the page's glyphs") };
            let stretch = &pages[0][range];
            let highlight = store.add_pdf_annotation(id, &file.id, 1, Kind::Highlight, &wisp_core::pdftext::marks_text(stretch), &wisp_core::pdftext::mark_boxes(stretch))?;
            store.update_pdf_annotation_note(&file.id, &highlight.id, "回头核对这一句和方法部分是否一致")?;
            store.add_pdf_shape(id, &file.id, 1, Kind::Note, [72., 90., 72., 90.], "这一页的数据要和图 2 对照")?;
            store.add_pdf_text(id, &file.id, pages.len(), [72., 72., 300., 110.], "比较两种激素的作用", &TextStyle::default())?;
        }
    }
    if let Some(id) = &first {
        for path in &supplements { store.import_file(id, path, "supplement")?; }
    }

    let roots = libraries::create(&root, "植物根尖单细胞", "根尖分生组织的单细胞转录组研究：细胞类型图谱、发育轨迹和干细胞微环境。", "green")?;
    let mut store = Store::open(&roots.dir)?;
    for (index, path) in more.iter().enumerate() {
        let pages = wisp_core::fulltext::file_pages(path);
        // The title is the first heading of some length; a file that gives no text keeps its name.
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = pages.as_ref().ok().and_then(|pages| wisp_core::structure::blocks(pages).into_iter().find(|b| b.heading && b.text.split_whitespace().count() >= 6 && !b.text.to_lowercase().contains("open access"))).map_or(stem.clone(), |b| b.text);
        let record = |title: String| Paper { title, year: Some(2020 + (index % 6) as i32), source: "示例数据".into(), ..Default::default() };
        // Two files with the same first heading are still two papers.
        let paper = store.add_paper(record(title)).or_else(|_| store.add_paper(record(stem)))?;
        let file = store.import_file(&paper.id, path, "fulltext")?;
        store.save_fulltext_pages(&file.id, &pages)?;
    }
    // With PDFs of its own the library needs no sample record (one of them may be this very paper).
    if more.is_empty() { store.add_paper(Paper { title: "A single-cell RNA sequencing profiles the developmental landscape of Arabidopsis root".into(), authors: "Zhang, Tian-Qi; Xu, Zhou-Geng; Shang, Guan-Dong; Wang, Jia-Wei".into(), year: Some(2019), journal: "Molecular Plant".into(), doi: "10.1016/j.molp.2019.04.004".into(), ..Default::default() })?; }
    // 方法库: two methods of its first paper, one linked to the blot it gave, and one written down by hand;
    // 导言库: two ways papers lead into their question.
    if let Some(paper) = store.list_papers()?.into_iter().next_back() {
        let mut blot = Figure { paper_id: paper.id.clone(), page: 6, label: "Fig. 3".into(), legend: "Western blot of WOX5-GFP in root tips after auxin treatment; actin as the loading control.".into(), tags: vec!["western blot".into()], ..Default::default() };
        store.add_figure(&mut blot, &blot_picture()?)?;
        let methods = [
            ("Western blot", 9, "Total proteins were extracted from 5-day-old root tips, separated by 10% SDS-PAGE and transferred to PVDF membranes (90 min, 100 V). Membranes were blocked in 5% milk and incubated with anti-GFP (1:2000) overnight at 4 °C.", "转膜我们改成 60 min，条带一样清楚；一抗 1:5000 也够。", vec!["蛋白", "western blot"], true),
            ("根尖原生质体分离", 4, "Root tips (0.5 cm) were digested in 1.5% cellulase R10 and 0.4% macerozyme R10 for 2 h at 22 °C with gentle shaking, filtered through a 40 μm strainer and washed twice.", "酶解超过 2 h 细胞活力下降，单细胞测序前控制在 1.5 h。", vec!["单细胞", "原生质体"], false),
        ];
        for (name, page, text, notes, tags, linked) in methods {
            let mut method = Excerpt { paper_id: paper.id.clone(), page, name: name.into(), text: text.into(), notes: notes.into(), tags: tags.into_iter().map(String::from).collect(), ..Default::default() };
            store.add_excerpt(&mut method)?;
            if linked { store.set_excerpt_figures(&method.id, std::slice::from_ref(&blot.id))? }
        }
        let intros = [
            ("从组织的复杂性切入", 1, "The root is a complex organ composed of many cell types, each arising from stem cells in the meristem. How these cell types are specified, and in which order, has been inferred mostly from a handful of marker genes.", "先讲器官由很多细胞类型组成，再说以前只靠少数标记基因：空白一句话就点出来了。", vec!["空白", "单细胞"]),
            ("从技术的局限切入", 2, "Bulk RNA-seq averages over thousands of cells and hides the transitions between states. Single-cell RNA sequencing now makes it possible to follow these transitions directly, yet a developmental atlas of the Arabidopsis root at this resolution is still missing.", "用旧方法的局限引出新方法，最后落到「还没有图谱」，正好接上本文的目的。", vec!["方法局限", "空白"]),
        ];
        for (name, page, text, notes, tags) in intros {
            let mut intro = Excerpt { kind: excerpt::INTRO.into(), paper_id: paper.id.clone(), page, name: name.into(), text: text.into(), notes: notes.into(), tags: tags.into_iter().map(String::from).collect(), ..Default::default() };
            store.add_excerpt(&mut intro)?;
        }
        let mut own = Excerpt { name: "qPCR 引物验证".into(), text: "每对引物做 5 个梯度的标准曲线，扩增效率在 90%–110% 之间才用。".into(), tags: vec!["qPCR".into()], ..Default::default() };
        store.add_excerpt(&mut own)?;
    }
    println!("seeded {}", root.display());
    Ok(())
}

/// A picture like a western blot: six lanes, a band that grows lane by lane over a loading control.
fn blot_picture() -> anyhow::Result<Vec<u8>> {
    let picture = image::RgbImage::from_fn(480, 240, |x, y| {
        let lane = ((x as f32 - 40.) / 80.).round().clamp(0., 5.);
        let (dx, strength) = ((x as f32 - (40. + lane * 80.)) / 30., 0.25 + lane * 0.14);
        let band = |at: f32, depth: f32| depth * (-(dx.powi(4)) - ((y as f32 - at) / 9.).powi(2)).exp();
        let ink = (band(80., strength) + band(170., 0.7)).min(1.);
        let grey = (245. - ink * 215.) as u8;
        image::Rgb([grey, grey, grey])
    });
    let mut bytes = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode_image(&picture)?;
    Ok(bytes)
}
