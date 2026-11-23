//! A diagram in an answer: the Mermaid source of a ```` ```mermaid ````
//! block drawn as a picture where the block stands. `wisp_core::mermaid`
//! makes the SVG in the window's colours; here it becomes pixels (resvg,
//! off the main thread) and an element as wide as the column allows. On
//! the picture, on hover: its own size in place of the fitted one (a large
//! graph scrolls sideways then), a button that copies it as an image on
//! white, for a slide or a manuscript, and a switch to its source and back.

use crate::i18n::t;
use crate::theme::{self, Theme};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use wisp_core::mermaid::{self, Palette};

/// How many pixels a unit of the diagram is drawn with on screen: enough for a display of double density.
const SCREEN_SCALE: f32 = 2.;

/// The same for a copy that leaves the window: it may be set large on a slide.
const COPY_SCALE: f32 = 3.;

/// The most pixels along one edge of a picture: a graph of hundreds of nodes is laid out ten thousand units wide,
/// and its picture is not to take a hundred megabytes.
const MAX_EDGE: f32 = 4096.;

/// The tallest a diagram stands in an answer before it is asked for at its own size: a tall graph is not to push
/// what is said about it out of sight, and a dozen names one under the other are still to be read (14 px words
/// are not made much smaller by it).
const FITTED_HEIGHT: f32 = 520.;

/// From this width on a diagram may have been made smaller to fit the column: then it is offered at its own size.
const MAY_BE_FITTED: f32 = 480.;

/// The most diagrams kept drawn. Each is some tens of kilobytes of SVG; a change of theme draws them all anew.
const KEPT: usize = 96;

/// A diagram drawn for the window: its SVG and the size it was laid out at.
pub struct Drawn {
    key: u64,
    svg: Arc<str>,
    /// The colour it stands on.
    ground: [u8; 3],
    pub width: f32,
    pub height: f32,
}

/// What a button on a diagram asks of the window.
pub enum Ask {
    /// The picture of this source, to the clipboard.
    CopyPicture(String),
}

thread_local! {
    /// What each source came to, by the source and the palette it was drawn in.
    static DRAWN: RefCell<HashMap<u64, Result<Rc<Drawn>, String>>> = RefCell::new(HashMap::new());
    /// The diagrams the person asked to see as source, by their source.
    static AS_SOURCE: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    /// Those asked for at their own size instead of fitted to the column: of this one place, not a habit.
    static OWN_SIZE: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    /// Who hears what a diagram's button asks: the workspace, once it is there.
    static ASKED: RefCell<Option<Rc<dyn Fn(Ask, &mut Window, &mut App)>>> = const { RefCell::new(None) };
}

/// The fonts a diagram's words are set in. Read once, when the first diagram is drawn, on the thread that draws it.
static FONTS: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = resvg::usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    Arc::new(fonts)
});

/// Who is told what a diagram's button asks.
pub fn on_ask(heard: impl Fn(Ask, &mut Window, &mut App) + 'static) {
    ASKED.with(|asked| *asked.borrow_mut() = Some(Rc::new(heard)));
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// A colour as `#RRGGBB`, what shows through it taken from `ground`.
fn hex(color: Hsla, ground: Hsla) -> String {
    let (over, under) = (Rgba::from(color), Rgba::from(ground));
    let mix = |over_part: f32, under_part: f32| ((over_part * over.a + under_part * (1. - over.a)).clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}", mix(over.r, under.r), mix(over.g, under.g), mix(over.b, under.b))
}

/// The font families a diagram asks for, first found first: the person's own choices, then one of the platform
/// that has Chinese as well as Latin letters, so that a label in both is set in one face.
fn families() -> String {
    let chosen = theme::ui_font();
    let mut names: Vec<String> = vec![];
    if chosen.family.as_ref() != theme::UI_FONT { names.push(chosen.family.to_string()) }
    if let Some(fallbacks) = &chosen.fallbacks { names.extend(fallbacks.fallback_list().iter().cloned()) }
    let platform: &[&str] = if cfg!(target_os = "macos") { &["PingFang SC", "Helvetica Neue"] } else if cfg!(target_os = "windows") { &["Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI"] } else { &["Noto Sans CJK SC", "Noto Sans"] };
    names.extend(platform.iter().map(|name| name.to_string()));
    let quoted: Vec<String> = names.iter().filter(|name| !name.contains('"')).map(|name| format!("\"{name}\"")).collect();
    format!("{}, sans-serif", quoted.join(", "))
}

/// The window's colours as a diagram takes them: nodes on the sunken ground with the stronger line around them,
/// and for what is counted or branched the library colours, thinned so that the text on them stays the text colour.
pub fn palette() -> Palette {
    let ground = Theme::bg_body();
    let solid = |color: Hsla| hex(color, ground);
    Palette {
        dark: theme::dark(),
        ground: solid(ground),
        panel: solid(Theme::bg_sunken()),
        text: solid(Theme::text()),
        secondary: solid(Theme::text_secondary()),
        line: solid(Theme::line()),
        line_strong: solid(Theme::line_strong()),
        note: solid(Theme::accent_soft()),
        series: theme::ACCENTS.iter().map(|accent| solid(accent.strong().opacity(0.3))).collect(),
        font: families(),
    }
}

/// The diagram of `source` in the window's colours now, or why there is none. Drawn once and kept: this is
/// asked for in every frame the answer is drawn.
pub fn drawn(source: &str) -> Result<Rc<Drawn>, String> {
    let palette = palette();
    let key = hash_of((source, &palette));
    if let Some(kept) = DRAWN.with(|kept| kept.borrow().get(&key).cloned()) { return kept }
    let result = mermaid::draw(source, &palette).map(|drawing| Rc::new(Drawn { key, svg: drawing.svg.into(), ground: rgb_of(&palette.ground), width: drawing.width, height: drawing.height }));
    DRAWN.with(|kept| {
        let mut kept = kept.borrow_mut();
        if kept.len() >= KEPT { kept.clear() }
        kept.insert(key, result.clone());
    });
    result
}

/// Whether the person asked to see this diagram as its source.
pub fn as_source(source: &str) -> bool {
    AS_SOURCE.with(|shown| shown.borrow().contains(&hash_of(source)))
}

/// Puts a diagram into one of the sets above, or takes it out again.
fn toggle(set: &'static std::thread::LocalKey<RefCell<HashSet<u64>>>, source: &str) {
    let key = hash_of(source);
    set.with(|set| {
        let mut set = set.borrow_mut();
        if !set.remove(&key) { set.insert(key); }
    });
}

/// The three parts of a `#RRGGBB`.
fn rgb_of(hex: &str) -> [u8; 3] {
    let part = |at: usize| hex.get(at..at + 2).and_then(|part| u8::from_str_radix(part, 16).ok()).unwrap_or(255);
    [part(1), part(3), part(5)]
}

/// An SVG as pixels: `scale` pixels to a unit, straight (not premultiplied) RGBA. `opaque` paints `ground`
/// under it; otherwise nothing is under it, and where nothing is drawn the pixels are `ground` seen through
/// entirely. Not black seen through entirely: the window draws a picture smaller by mixing neighbours, and the
/// edge of every white box would mix with that black into a grey line.
fn pixels(svg: &str, scale: f32, ground: [u8; 3], opaque: bool) -> Result<image::RgbaImage, String> {
    let options = resvg::usvg::Options { fontdb: FONTS.clone(), ..Default::default() };
    let tree = resvg::usvg::Tree::from_str(svg, &options).map_err(|err| err.to_string())?;
    let size = tree.size();
    // A diagram of hundreds of nodes is drawn smaller rather than not at all.
    let scale = scale.min(MAX_EDGE / size.width().max(size.height()).max(1.));
    let (width, height) = ((size.width() * scale).ceil().max(1.) as u32, (size.height() * scale).ceil().max(1.) as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or("图太大了")?;
    if opaque { pixmap.fill(resvg::tiny_skia::Color::from_rgba8(ground[0], ground[1], ground[2], 255)) }
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut bytes = Vec::with_capacity(pixmap.pixels().len() * 4);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        if color.alpha() == 0 { bytes.extend_from_slice(&[ground[0], ground[1], ground[2], 0]) } else { bytes.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]) }
    }
    image::RgbaImage::from_raw(width, height, bytes).ok_or_else(|| "图太大了".to_string())
}

/// The diagram of `source` as a PNG on white, in the colours of a light page whatever the window's are: what
/// is pasted into a slide or a manuscript.
pub fn png(source: &str) -> Result<Vec<u8>, String> {
    let palette = Palette { font: families(), ..Palette::light() };
    let drawing = mermaid::draw(source, &palette)?;
    let picture = pixels(&drawing.svg, COPY_SCALE, [255, 255, 255], true)?;
    let mut out = vec![];
    picture.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|err| err.to_string())?;
    Ok(out)
}

/// What the picture of a diagram is loaded by: the drawing's key stands for its SVG.
#[derive(Clone)]
struct Sheet {
    key: u64,
    svg: Arc<str>,
    ground: [u8; 3],
}

impl Hash for Sheet {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

/// The pixels of a diagram, made off the main thread and kept by the window like any picture it loads.
struct Picture;

impl Asset for Picture {
    type Source = Sheet;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    // Not an `async fn`: its future would hold on to `_cx`, and the trait wants one that outlives the call.
    #[allow(clippy::manual_async_fn)]
    fn load(source: Self::Source, _cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        async move {
            let mut picture = pixels(&source.svg, SCREEN_SCALE, source.ground, false).map_err(|err| ImageCacheError::Other(Arc::new(anyhow::anyhow!(err))))?;
            // The window draws BGRA.
            for pixel in picture.pixels_mut() { pixel.0.swap(0, 2) }
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(picture)])))
        }
    }
}

/// The buttons that come with a diagram, at its top right while the pointer is over it (`group`): the switch
/// between picture and source, lit while the source shows; beside the picture the copy, and for one that may
/// have been made smaller (`own_size` says whether it is at its own size now) the way to its own size and back.
fn actions(id: &str, group: &SharedString, source: &str, showing_source: bool, own_size: Option<bool>) -> Div {
    let (toggled, copied, sized) = (source.to_string(), source.to_string(), source.to_string());
    let size = own_size.map(|own| {
        ui::icon_action(SharedString::from(format!("{id}-size")), if own { "zoom-out" } else { "zoom-in" }, if own { Theme::accent() } else { Theme::text_secondary() }, if own { t("适应宽度") } else { t("原始大小") })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, _| {
                toggle(&OWN_SIZE, &sized);
                window.refresh();
            })
    });
    let switch = ui::icon_action(SharedString::from(format!("{id}-source")), "code", if showing_source { Theme::accent() } else { Theme::text_secondary() }, if showing_source { t("看图") } else { t("看源码") })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, _| {
            toggle(&AS_SOURCE, &toggled);
            window.refresh();
        });
    let copy = ui::icon_action(SharedString::from(format!("{id}-copy")), "copy", Theme::text_secondary(), t("复制图片"))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            if let Some(asked) = ASKED.with(|asked| asked.borrow().clone()) { asked(Ask::CopyPicture(copied.clone()), window, cx) }
        });
    div()
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(8.))
        .border_1()
        .border_color(Theme::line())
        .bg(Theme::bg_body())
        .cursor_default()
        .invisible()
        .group_hover(group.clone(), |s| s.visible())
        .children(size)
        .when(!showing_source, |d| d.child(copy))
        .child(switch)
}

/// The picture of a diagram: at the size it was laid out for, made smaller where the column is narrower than
/// that or the diagram taller than an answer has room for; at its own size when asked, scrolling sideways.
pub fn picture(id: &str, source: &str, drawn: &Rc<Drawn>) -> AnyElement {
    let group = SharedString::from(format!("{id}-group"));
    let sheet = Sheet { key: drawn.key, svg: drawn.svg.clone(), ground: drawn.ground };
    let loaded = ImageSource::Custom(Arc::new(move |window, cx| window.use_asset::<Picture>(&sheet, cx)));
    let picture = img(loaded).object_fit(ObjectFit::Contain).with_fallback(|| div().text_size(theme::text_px(12.5)).text_color(Theme::text_tertiary()).child(t("这张图没画出来")).into_any_element());
    let ratio = drawn.width / drawn.height;
    let may_be_fitted = drawn.width > MAY_BE_FITTED || drawn.height > FITTED_HEIGHT;
    let own_size = may_be_fitted && OWN_SIZE.with(|own| own.borrow().contains(&hash_of(source)));
    let shown = if own_size {
        div().id(SharedString::from(format!("{id}-own"))).w_full().overflow_x_scroll().child(picture.flex_none().w(px(drawn.width)).h(px(drawn.height))).into_any_element()
    } else {
        let mut frame = div().w_full().max_w(px(drawn.width.min(FITTED_HEIGHT * ratio)));
        frame.style().aspect_ratio = Some(ratio);
        frame.child(picture.size_full()).into_any_element()
    };
    div().group(group.clone()).relative().w_full().cursor_default().child(shown).child(actions(id, &group, source, false, may_be_fitted.then_some(own_size))).into_any_element()
}

/// A diagram's source as it is shown in place of the picture: `code` is the block of code as any other is
/// drawn, with the switch back to the picture on it, or under it why there is no picture.
pub fn source(id: &str, source: &str, code: AnyElement, failed: Option<&str>) -> AnyElement {
    let group = SharedString::from(format!("{id}-group"));
    div()
        .group(group.clone())
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(code)
        .when_some(failed, |d, why| d.child(div().text_size(theme::text_px(12.)).line_height(theme::body_line(18.)).text_color(Theme::text_tertiary()).child(tf!("这张图画不出来：{why}", "This diagram cannot be drawn: {why}"))))
        .when(failed.is_none(), |d| d.child(actions(id, &group, source, true, None)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{hex, pixels, png, rgb_of};
    use gpui::{hsla, rgb};
    use wisp_core::mermaid::{self, Palette};

    #[test]
    fn colours_as_hex() {
        assert_eq!(hex(rgb(0x2F6BDE).into(), rgb(0xFFFFFF).into()), "#2F6BDE");
        // Half of black over white.
        assert_eq!(hex(hsla(0., 0., 0., 0.5), rgb(0xFFFFFF).into()), "#808080");
        assert_eq!((rgb_of("#2F6BDE"), rgb_of("nonsense")), ([0x2F, 0x6B, 0xDE], [255, 255, 255]));
    }

    #[test]
    fn a_diagram_becomes_pixels() {
        let source = "graph LR\n  A[张伟] -->|合著| B[Dolf Weijers]";
        let drawing = mermaid::draw(source, &Palette::light()).unwrap();
        let picture = pixels(&drawing.svg, 2., [0x22, 0x23, 0x26], false).unwrap();
        assert_eq!((picture.width(), picture.height()), ((drawing.width * 2.).ceil() as u32, (drawing.height * 2.).ceil() as u32));
        // Where nothing is drawn, the ground seen through entirely; the node's fill where a node is; and ink somewhere: the words and the lines.
        assert_eq!(picture.get_pixel(0, 0).0, [0x22, 0x23, 0x26, 0]);
        assert!(picture.pixels().any(|pixel| pixel.0 == [0xF7, 0xF7, 0xF5, 0xFF]));
        assert!(picture.pixels().any(|pixel| pixel.0[3] == 0xFF && pixel.0[0] < 0x60));

        // The copy is a PNG on white, three pixels to a unit.
        let copy = image::load_from_memory(&png(source).unwrap()).unwrap().to_rgba8();
        assert_eq!(copy.width(), (drawing.width * 3.).ceil() as u32);
        assert_eq!(copy.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert!(png("这不是图").is_err());
    }
}
