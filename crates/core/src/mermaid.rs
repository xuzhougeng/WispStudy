//! A diagram from Mermaid source: what an assistant writes in a
//! ```` ```mermaid ```` block (who worked with whom, the steps of a method, a
//! timeline) becomes an SVG picture. The reading, the layout and the drawing
//! are `merman`'s, a port of Mermaid that needs no browser; this module gives
//! it the colours of whoever shows the picture and makes its SVG one a plain
//! SVG renderer (resvg) draws as meant.

use merman::svg::SvgPipeline;
use merman::{MermaidEngine, OperationControl, RenderOutput, RenderRequest, Renderer, SourcePresentationConfig, SvgRequest};
use serde_json::{Map, Value};

/// The longest source that is drawn. An answer's diagram is a few hundred
/// bytes; beyond this the layout only costs time and nobody reads the result.
pub const MAX_SOURCE: usize = 24_000;

/// The colours and the font a diagram is drawn with: `#RRGGBB` each.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Palette {
    pub dark: bool,
    /// What the diagram stands on.
    pub ground: String,
    /// The fill of a node.
    pub panel: String,
    pub text: String,
    /// Lines between nodes, and the smaller words.
    pub secondary: String,
    /// The edge of a group.
    pub line: String,
    /// The edge of a node.
    pub line_strong: String,
    /// The fill of a note beside the diagram's own parts.
    pub note: String,
    /// The fills that tell apart what is counted or branched: the slices of a pie, the branches of a mind map.
    pub series: Vec<String>,
    /// A CSS font family list.
    pub font: String,
}

impl Palette {
    /// The colours of a white page, for where no window says otherwise.
    pub fn light() -> Palette {
        let hex = |values: &[&str]| values.iter().map(|value| value.to_string()).collect();
        Palette {
            dark: false,
            ground: "#FFFFFF".into(),
            panel: "#F7F7F5".into(),
            text: "#1F2329".into(),
            secondary: "#5C636E".into(),
            line: "#E7E7E3".into(),
            line_strong: "#D5D6D2".into(),
            note: "#E8F0FE".into(),
            series: hex(&["#C1D3F5", "#C0DCCD", "#E9D7BC", "#EFC7D3", "#D4CBEF", "#BCDCDC", "#C9CCD2"]),
            font: "Arial, sans-serif".into(),
        }
    }
}

/// A diagram drawn: the SVG, and its size in the SVG's own units (pixels at the size it was laid out for).
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub svg: String,
    pub width: f32,
    pub height: f32,
}

/// Whether a fenced block of this language is a diagram to draw.
pub fn is_mermaid(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid") || lang.eq_ignore_ascii_case("mmd")
}

/// What Mermaid is told about the look: its `base` theme with every colour it derives others from set to one of
/// the palette's, so nothing of its own purple and yellow is left.
fn config(palette: &Palette) -> Value {
    let p = palette;
    let mut vars = Map::new();
    let mut set = |keys: &[&str], value: &str| {
        for key in keys { vars.insert(key.to_string(), Value::String(value.to_string())); }
    };
    set(&["fontFamily"], &p.font);
    set(&["fontSize"], "14px");
    set(&["background", "edgeLabelBackground", "clusterBkg", "tertiaryColor", "pieStrokeColor"], &p.ground);
    set(&["primaryColor", "secondaryColor", "mainBkg", "actorBkg", "labelBoxBkgColor", "activationBkgColor", "attributeBackgroundColorOdd", "attributeBackgroundColorEven"], &p.panel);
    set(&["quadrant1Fill", "quadrant2Fill", "quadrant3Fill", "quadrant4Fill"], &p.panel);
    set(&["primaryTextColor", "secondaryTextColor", "tertiaryTextColor", "textColor", "nodeTextColor", "noteTextColor", "actorTextColor", "signalTextColor", "labelTextColor", "loopTextColor", "classText", "labelColor"], &p.text);
    set(&["pieTitleTextColor", "pieSectionTextColor", "pieLegendTextColor", "quadrant1TextFill", "quadrant2TextFill", "quadrant3TextFill", "quadrant4TextFill", "quadrantPointTextFill", "quadrantTitleFill", "quadrantXAxisTextFill", "quadrantYAxisTextFill"], &p.text);
    set(&["lineColor", "signalColor", "titleColor", "quadrantPointFill"], &p.secondary);
    set(&["tertiaryBorderColor", "clusterBorder", "pieOuterStrokeColor", "quadrantInternalBorderStrokeFill"], &p.line);
    set(&["primaryBorderColor", "secondaryBorderColor", "nodeBorder", "noteBorderColor", "actorBorder", "actorLineColor", "labelBoxBorderColor", "activationBorderColor", "quadrantExternalBorderStrokeFill"], &p.line_strong);
    set(&["noteBkgColor"], &p.note);
    set(&["pieStrokeWidth", "pieOuterStrokeWidth"], "1px");
    set(&["pieOpacity"], "1");
    for at in 0..12 {
        let Some(fill) = p.series.get(at % p.series.len().max(1)) else { break };
        set(&[&format!("pie{}", at + 1), &format!("cScale{at}")], fill);
        set(&[&format!("cScaleLabel{at}"), &format!("cScaleInv{at}")], &p.text);
    }
    // Mermaid breaks a label after 120 pixels and makes no node narrower than that: names and titles are longer
    // than the one and shorter than the other. Its layout of choice (ELK) is not built in; dagre is.
    let mut sizes = Map::new();
    sizes.insert("wrappingWidth".into(), 220.into());
    sizes.insert("minNodeWidth".into(), 48.into());
    let mut out = Map::new();
    out.insert("layout".into(), "dagre".into());
    out.insert("flowchart".into(), Value::Object(sizes.clone()));
    out.insert("state".into(), Value::Object(sizes));
    out.insert("theme".into(), "base".into());
    out.insert("darkMode".into(), p.dark.into());
    out.insert("fontFamily".into(), p.font.clone().into());
    out.insert("themeVariables".into(), Value::Object(vars));
    Value::Object(out)
}

/// The number after `name="` in the first tag of an SVG.
fn root_numbers(svg: &str, name: &str) -> Option<Vec<f32>> {
    let tag = &svg[..svg.find('>')?];
    let from = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let value = &tag[from..from + tag[from..].find('"')?];
    value.split([' ', ',']).filter(|part| !part.is_empty()).map(|part| part.parse().ok()).collect()
}

/// The SVG as a plain renderer takes it. Mermaid leaves the width to the page (`width="100%"`): the picture is
/// given the size it was laid out at. And a label stands in a box as wide as its words were reckoned to be,
/// which cuts the ends off words that came out a little wider in the font actually found: the box no longer cuts.
fn plain(svg: &str, width: f32, height: f32) -> String {
    let end = svg.find('>').unwrap_or(0);
    let (tag, rest) = svg.split_at(end);
    let tag = if tag.contains(" width=\"100%\"") { tag.replacen(" width=\"100%\"", &format!(" width=\"{width}\" height=\"{height}\""), 1) } else { tag.to_string() };
    // The ground under the words on a line is half see-through in Mermaid's sheet: the line would run through the words.
    let rest = rest.replace("style=\"overflow:hidden;", "style=\"overflow:visible;").replace(".edgeLabel rect{opacity:0.5;", ".edgeLabel rect{opacity:1;");
    format!("{tag}{rest}")
}

/// The diagram `source` describes, in the colours of `palette`. An error is a sentence for whoever wrote the
/// source: what Mermaid did not understand, and where.
pub fn draw(source: &str, palette: &Palette) -> Result<Drawing, String> {
    let source = source.trim();
    if source.is_empty() { return Err("空的图".into()) }
    if source.len() > MAX_SOURCE { return Err(format!("图的源码太长（{} 字节，最多 {MAX_SOURCE}）", source.len())) }
    // A port of a large JavaScript library, fed whatever a model wrote: a panic in it is one diagram not drawn.
    let drawn = std::panic::catch_unwind(|| {
        let engine = MermaidEngine::new().with_site_config(SourcePresentationConfig::from_value(config(palette)));
        let request = SvgRequest { pipeline: Some(SvgPipeline::resvg_safe()), ..Default::default() };
        Renderer::new().with_presentation_engine(engine).render(RenderRequest::svg(source, OperationControl::new(), request))
    });
    let svg = match drawn {
        Ok(Ok(RenderOutput::Svg(Some(svg)))) => svg.svg().to_string(),
        Ok(Ok(_)) => return Err("不是 Mermaid 认识的图".into()),
        Ok(Err(err)) => return Err(err.to_string()),
        Err(_) => return Err("画这张图时出错了".into()),
    };
    let size = root_numbers(&svg, "viewBox").filter(|numbers| numbers.len() == 4).map(|numbers| (numbers[2], numbers[3]));
    let Some((width, height)) = size.filter(|(width, height)| *width > 0. && *height > 0. && width.is_finite() && height.is_finite()) else { return Err("图没有大小".into()) };
    Ok(Drawing { svg: plain(&svg, width, height), width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTHORS: &str = "graph LR\n  A[\"张伟 (中科院遗传所)\"] -->|共同通讯 3 篇| B[李娜]\n  A --> C[Dolf Weijers]\n  subgraph 瓦赫宁根大学\n    C\n    E[Ben Scheres]\n  end\n  E -.-> A\n  B ==> F((单细胞图谱))";

    #[test]
    fn a_graph_of_authors_is_drawn_in_the_palette() {
        let drawn = draw(AUTHORS, &Palette::light()).unwrap();
        assert!(drawn.width > 300. && drawn.height > 100., "{} x {}", drawn.width, drawn.height);
        // Wider than tall: `LR` was heeded. A name with its institute stays on one line.
        assert!(drawn.width > drawn.height);
        for label in ["张伟 (中科院遗传所)", "李娜", "Dolf Weijers", "Ben Scheres", "瓦赫宁根大学", "共同通讯 3 篇", "单细胞图谱"] {
            assert!(drawn.svg.contains(label), "{label}");
        }
        // The size is the picture's own, the labels are not cut, and the colours are the palette's.
        let tag = &drawn.svg[..drawn.svg.find('>').unwrap()];
        assert!(tag.contains(&format!("width=\"{}\" height=\"{}\"", drawn.width, drawn.height)) && !tag.contains("100%"), "{tag}");
        assert!(!drawn.svg.contains("overflow:hidden") && !drawn.svg.contains(".edgeLabel rect{opacity:0.5") && drawn.svg.contains(".edgeLabel rect{opacity:1;"));
        assert!(drawn.svg.contains("#F7F7F5") && drawn.svg.contains("#1F2329") && drawn.svg.contains("Arial"));
        // Nothing a plain SVG renderer leaves out.
        assert!(!drawn.svg.contains("<foreignObject"));

        // Another palette, another picture of the same size.
        let dark = Palette { dark: true, ground: "#222326".into(), panel: "#2A2B2F".into(), text: "#E6E7EA".into(), ..Palette::light() };
        let night = draw(AUTHORS, &dark).unwrap();
        assert!(night.svg.contains("#2A2B2F") && night.svg.contains("#E6E7EA") && !night.svg.contains("#F7F7F5"));
        assert_eq!((night.width, night.height), (drawn.width, drawn.height));
    }

    #[test]
    fn the_kinds_an_assistant_writes() {
        let kinds = [
            ("flowchart TD\n  样本 --> 建库 --> 测序\n  测序 --> 聚类{聚类}\n  聚类 -->|已知标记| 注释", "注释"),
            ("sequenceDiagram\n  participant 读者\n  participant 助手\n  读者->>助手: 这些作者有什么联系？\n  Note over 助手: 按合著归组\n  助手-->>读者: 一张关系图", "按合著归组"),
            ("mindmap\n  root((单细胞转录组))\n    平台\n      10x Genomics\n    物种\n      拟南芥", "10x Genomics"),
            ("pie title 文献按年份\n  \"2021\" : 4\n  \"2022\" : 9", "文献按年份"),
            ("timeline\n  title 根尖单细胞研究\n  2019 : 首批拟南芥根图谱\n  2023 : 空间转录组", "空间转录组"),
            ("stateDiagram-v2\n  [*] --> 待筛\n  待筛 --> 纳入: 相关\n  纳入 --> [*]", "待筛"),
            ("classDiagram\n  class Paper {\n    +String title\n  }\n  Paper --> Author : 作者", "Author"),
            ("erDiagram\n  PAPER ||--o{ AUTHOR : has", "AUTHOR"),
            ("gantt\n  title 写作计划\n  dateFormat YYYY-MM-DD\n  初稿 :a1, 2026-01-01, 30d\n  修改 :after a1, 20d", "初稿"),
        ];
        for (source, label) in kinds {
            let drawn = draw(source, &Palette::light()).unwrap_or_else(|err| panic!("{source}: {err}"));
            assert!(drawn.svg.contains(label) && drawn.width > 0. && drawn.height > 0., "{source}");
        }
        // The fills of a pie are the palette's series.
        let pie = draw("pie\n  \"a\" : 1\n  \"b\" : 2", &Palette::light()).unwrap();
        assert!(pie.svg.contains("#C1D3F5") && pie.svg.contains("#C0DCCD"));
    }

    #[test]
    fn what_is_not_a_diagram_says_why() {
        // A line a model left unfinished, plain words, nothing, and far too much.
        let err = draw("graph LR\n  A --> \n  ][ what", &Palette::light()).unwrap_err();
        assert!(!err.is_empty());
        assert!(draw("这不是一张图", &Palette::light()).is_err());
        assert_eq!(draw("  \n", &Palette::light()).unwrap_err(), "空的图");
        let long = format!("graph LR\n{}", "  A --> B\n".repeat(MAX_SOURCE / 8));
        assert!(draw(&long, &Palette::light()).unwrap_err().contains("太长"));
        assert!(is_mermaid("mermaid") && is_mermaid("Mermaid") && !is_mermaid("python") && !is_mermaid(""));
    }
}
