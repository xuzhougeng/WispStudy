//! 资料库: what a manuscript draws on besides the library's papers, which
//! for now is pictures. It is a row of the outline, over 参考文献, and opens
//! a dialog over the manuscript: the figures of 图片库 (`figures.rs`), to
//! search and to narrow to those the person brought in, those cut from
//! papers, those a model drew, and those the manuscript already shows. A
//! picture of the person's own is brought in here (上传图片, a file dropped on
//! the dialog or on the manuscript, a picture pasted into either) and kept
//! as a figure without a paper (`figure::UPLOADED`).
//!
//! A click on a card puts the figure into the manuscript: a line of its own
//! after the paragraph at the caret, `![caption](figures/G….png)`, which
//! the editor and 预览 draw as the picture over its caption and the exports
//! write as a figure (`manuscript::picture_lines`). A figure cut from a
//! paper cites that paper in its caption.

use crate::app::{Cmd, Modal, Workspace};
use crate::figures::Thumb;
use crate::i18n::t;
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use wisp_core::figure::{self, Figure};
use wisp_core::manuscript;

/// The narrowest a card is drawn, the room between cards, and the heights of a card's picture and of its words.
const CARD_MIN: f32 = 184.;
const GAP: f32 = 12.;
const PICTURE_H: f32 = 122.;
const WORDS_H: f32 = 58.;

/// The longest edge a manuscript's picture is drawn from, and how many are kept.
const SHOT_EDGE: u32 = 1800;
const KEEP_SHOTS: usize = 24;

/// The tallest a picture stands in the manuscript, and the room between it and its caption.
pub(crate) const SHOT_MOST: f32 = 360.;
pub(crate) const SHOT_GAP: f32 = 8.;

/// The kinds of picture file that can be brought in, by the ends of their names.
const PICTURE_KINDS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

/// What the dialog does.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    Open,
    /// Ask which files to bring in.
    Upload,
    /// Bring these files in.
    Add(Vec<PathBuf>),
    /// Show only the figures of one kind.
    Only(Only),
    /// Put a figure into the manuscript, after the paragraph at the caret.
    Insert(String),
    /// Look at a figure and change what is written about it; closing that comes back here.
    Edit(String),
}

/// Which figures the dialog shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Only {
    #[default]
    All,
    /// Those the person brought in.
    Uploaded,
    /// Those cut from the library's papers.
    Papers,
    /// Those a model drew.
    Drawn,
    /// Those the manuscript on show has.
    Used,
}

impl Only {
    fn has(self, figure: &Figure, used: &HashSet<&str>) -> bool {
        match self {
            Only::All => true,
            Only::Uploaded => figure.uploaded(),
            Only::Papers => !figure.paper_id.is_empty(),
            Only::Drawn => figure.drawn(),
            Only::Used => used.contains(figure.local_path.as_str()),
        }
    }
}

/// How a picture of the manuscript stands.
pub(crate) enum Shot {
    Ready(Arc<RenderImage>),
    Loading,
    /// Its file is gone or cannot be read.
    Broken,
}

/// The dialog's state, kept while the library is open so it comes back as it was left, and the pictures the
/// manuscript on show draws.
pub struct Shelf {
    pub search: Entity<TextInput>,
    pub only: Only,
    pub scroll: UniformListScrollHandle,
    /// Files being read and brought in.
    adding: usize,
    /// The figures brought in since the dialog was last opened: said on their cards.
    fresh: HashSet<String>,
    /// The dialog was just opened: its search box gets the keyboard.
    opened: bool,
    /// The manuscript's pictures by their addresses, the least lately drawn first in `recent`.
    shots: HashMap<String, Arc<RenderImage>>,
    recent: VecDeque<String>,
    loading: HashSet<String>,
    broken: HashSet<String>,
}

impl Shelf {
    pub fn new(subscriptions: &mut Vec<Subscription>, cx: &mut Context<Workspace>) -> Shelf {
        let search = cx.new(|cx| {
            let mut input = TextInput::new("搜索图注、标签、备注、文件名，或所属文献…", cx);
            input.accepts_images = true;
            input
        });
        subscriptions.push(cx.subscribe(&search, |this, input, event: &InputEvent, cx| match event {
            InputEvent::Changed => {
                // Another set of cards: from its top.
                if let Some(lib) = this.lib.as_mut() { lib.write_figures.scroll = UniformListScrollHandle::new() }
                cx.notify();
            }
            // A picture pasted while the dialog is open is brought in.
            InputEvent::Images => {
                let pasted = input.update(cx, |input, _| input.take_images());
                this.write_figures_paste(pasted, false, cx);
            }
            _ => {}
        }));
        Shelf { search, only: Only::All, scroll: UniformListScrollHandle::new(), adding: 0, fresh: HashSet::new(), opened: false, shots: HashMap::new(), recent: VecDeque::new(), loading: HashSet::new(), broken: HashSet::new() }
    }

    /// A picture of the manuscript as it stands now, without reading anything.
    pub(crate) fn shot(&self, address: &str) -> Shot {
        match self.shots.get(address) {
            Some(image) => Shot::Ready(image.clone()),
            None if self.broken.contains(address) || !manuscript::library_picture(address) => Shot::Broken,
            None => Shot::Loading,
        }
    }
}

/// A picture to bring in: where it is from, and what it is called there.
enum Brought {
    File(PathBuf),
    Pasted(Vec<u8>),
}

/// A picture as the figure library keeps it: a PNG or a JPEG as it is, any other kind the app reads written as PNG
/// (a Word file and a slide take no other).
fn kept_bytes(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if matches!(figure::picture_kind(&bytes), Some("png" | "jpg")) && figure::picture_size(&bytes).is_some() { return Ok(bytes) }
    let picture = image::load_from_memory(&bytes).map_err(|_| t("不是能读的图片（支持 PNG、JPEG、WebP、GIF、BMP 和 TIFF）").to_string())?;
    let mut out = vec![];
    picture.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|err| err.to_string())?;
    Ok(out)
}

/// A picture that is not of a kind kept, written as a PNG: for the pictures a Markdown file brings along
/// (`manuscript::read_pictures`). `None` for what is no picture that can be read.
pub(crate) fn as_kept(bytes: &[u8]) -> Option<Vec<u8>> {
    kept_bytes(bytes.to_vec()).ok()
}

/// How many cards stand in a row `width` wide, and how wide each is.
fn grid(width: f32) -> (usize, f32) {
    let inner = (width - 40.).max(CARD_MIN);
    let columns = (((inner + GAP) / (CARD_MIN + GAP)).floor() as usize).max(1);
    (columns, (inner - GAP * (columns - 1) as f32) / columns as f32)
}

/// The caption a figure is put into a manuscript with: its legend, citing `key` when it is from a paper,
/// before the stop the legend ends with.
fn caption_of(legend: &str, key: Option<&str>) -> String {
    let legend = legend.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(key) = key else { return legend };
    let cite = format!("[@{key}]");
    let Some(stop) = legend.chars().next_back().filter(|c| matches!(c, '.' | '。')) else { return if legend.is_empty() { cite } else { format!("{legend} {cite}") } };
    format!("{} {cite}{stop}", legend[..legend.len() - stop.len_utf8()].trim_end())
}

/// The size a picture of `natural` pixels stands at in a column `width` wide, where a pixel of the screen is
/// `density` of the picture's: whole, never wider than the column or taller than `SHOT_MOST`, never larger than it is.
pub(crate) fn shot_size(natural: (f32, f32), width: f32, density: f32) -> (f32, f32) {
    crate::figures::fit((natural.0 / density, natural.1 / density), (width.max(1.), SHOT_MOST))
}

impl Workspace {
    pub(crate) fn run_write_figures(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Open => self.write_figures_open(cx),
            Do::Upload => {
                let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some(t("上传").into()), extensions: Some(PICTURE_KINDS.iter().map(|kind| (*kind).into()).collect()) });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picked.await { this.update(cx, |this, cx| this.write_figures_add(paths, false, cx)).ok(); }
                })
                .detach();
            }
            Do::Add(paths) => self.write_figures_add(paths, false, cx),
            Do::Only(only) => {
                let Some(lib) = self.lib.as_mut() else { return };
                // The kind chosen already, asked for again, shows all.
                lib.write_figures.only = if lib.write_figures.only == only { Only::All } else { only };
                lib.write_figures.scroll = UniformListScrollHandle::new();
            }
            Do::Insert(id) => self.write_figures_insert(&id, true, window, cx),
            Do::Edit(id) => {
                self.run(Cmd::OpenFigure(id), window, cx);
                if let Some(Modal::Figure(form)) = self.modal.as_mut() { form.back = true }
            }
        }
        cx.notify();
    }

    pub(crate) fn write_figures_open(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.writing.current().is_none() { return }
        (lib.write_figures.opened, lib.write_figures.scroll) = (true, UniformListScrollHandle::new());
        lib.write_figures.fresh.clear();
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        self.modal = Some(Modal::WriteFigures);
        cx.notify();
    }

    /// Whether these are all pictures by their names: what is dropped on the manuscript is then brought in, not imported as papers.
    pub(crate) fn pictures_by_name(paths: &[PathBuf]) -> bool {
        !paths.is_empty() && paths.iter().all(|path| path.extension().and_then(|e| e.to_str()).is_some_and(|e| PICTURE_KINDS.contains(&e.to_ascii_lowercase().as_str())))
    }

    /// Bring picture files into the figure library as the person's own; with `insert`, each also goes into the manuscript.
    pub(crate) fn write_figures_add(&mut self, paths: Vec<PathBuf>, insert: bool, cx: &mut Context<Self>) {
        self.write_figures_bring(paths.into_iter().map(Brought::File).collect(), insert, cx)
    }

    /// The same for pictures pasted.
    pub(crate) fn write_figures_paste(&mut self, pasted: Vec<Image>, insert: bool, cx: &mut Context<Self>) {
        self.write_figures_bring(pasted.into_iter().map(|image| Brought::Pasted(image.bytes)).collect(), insert, cx)
    }

    fn write_figures_bring(&mut self, brought: Vec<Brought>, insert: bool, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if brought.is_empty() { return }
        let library = lib.info.id.clone();
        lib.write_figures.adding += brought.len();
        cx.notify();
        // Read and, where the kind is not one kept, written anew off the main thread.
        let work = cx.background_executor().spawn(async move {
            brought
                .into_iter()
                .map(|one| {
                    let (name, bytes) = match one {
                        Brought::File(path) => {
                            let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                            let bytes = std::fs::read(&path).map_err(|err| tf!("{name}：读不了（{err}）", "{name}: cannot be read ({err})"));
                            (name, bytes)
                        }
                        Brought::Pasted(bytes) => (String::new(), Ok(bytes)),
                    };
                    let said = if name.is_empty() { t("粘贴的图片").to_string() } else { name.clone() };
                    (name, bytes.and_then(|bytes| kept_bytes(bytes).map_err(|err| format!("{said}：{err}"))))
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let read = work.await;
            this.update(cx, |this, cx| {
                let count = read.len();
                let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
                lib.write_figures.adding = lib.write_figures.adding.saturating_sub(count);
                let (mut kept, mut failed): (Vec<String>, Vec<String>) = (vec![], vec![]);
                // A picture the library holds already, to the byte, is not kept twice: it is the one meant.
                let mut known = 0;
                this.change(cx, |lib| {
                    for (name, bytes) in read {
                        let added = bytes.and_then(|bytes| {
                            let held = lib.gallery.items.iter().find(|figure| figure.size_bytes == bytes.len() as u64 && std::fs::read(lib.store.figure_path(figure)).is_ok_and(|kept| kept == bytes));
                            if let Some(held) = held {
                                known += 1;
                                return Ok(held.id.clone());
                            }
                            let mut figure = Figure { origin: figure::UPLOADED.into(), file_name: name.clone(), ..Default::default() };
                            lib.store.add_figure(&mut figure, &bytes).map(|_| figure.id).map_err(|err| if name.is_empty() { format!("{err:#}") } else { format!("{name}：{err:#}") })
                        });
                        match added {
                            Ok(id) => kept.push(id),
                            Err(err) => failed.push(err),
                        }
                    }
                    Ok(())
                });
                if let Some(lib) = this.lib.as_mut() && !kept.is_empty() {
                    // What was just brought in is on show: no words searched for, and a kind that has it.
                    lib.write_figures.fresh.extend(kept.iter().cloned());
                    if !matches!(lib.write_figures.only, Only::All | Only::Uploaded) { lib.write_figures.only = Only::All }
                    lib.write_figures.search.update(cx, |input, cx| if !input.text().is_empty() { input.set_text("", cx) });
                    lib.write_figures.scroll = UniformListScrollHandle::new();
                }
                if insert {
                    // In the order they were brought: each after the one before it.
                    for id in &kept { this.write_figures_put(id, cx); }
                }
                match (kept.len(), failed.first()) {
                    (0, Some(first)) => this.fail(if failed.len() > 1 { tf!("{first}；另有 {} 个也没有放进来", "{first}; {} more were not brought in either", failed.len() - 1) } else { first.clone() }, cx),
                    (n, Some(first)) => this.fail(tf!("放进了 {n} 张；{} 个没有放进来：{first}", "{n} brought in; {} were not: {first}", failed.len()), cx),
                    (n, None) if insert => this.say(tf!("{n} 张图片已放进稿件，也收进了资料库。点图片下面的字写图注。", "{n} pictures put into the manuscript and kept in Materials. Click the words under a picture to write its caption."), cx),
                    (n, None) if known == n => this.say(if n == 1 { t("这张图片已经在资料库里了。").to_string() } else { tf!("这 {n} 张图片已经在资料库里了。", "These {n} pictures are in Materials already.") }, cx),
                    (n, None) if known > 0 => this.say(tf!("已放进资料库 {} 张，另外 {known} 张原来就在。点一下放进稿件。", "{} brought into Materials; {known} were there already. Click one to put it into the manuscript.", n - known), cx),
                    (n, None) => this.say(tf!("已放进资料库 {n} 张，点一下放进稿件。", "{n} brought into Materials. Click one to put it into the manuscript."), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The addresses of the pictures the manuscript on show has.
    pub(crate) fn write_figures_used(&self) -> Vec<String> {
        self.lib.as_ref().and_then(|lib| lib.writing.current()).map(manuscript::pictures_used).unwrap_or_default()
    }

    /// Put a figure into the section being written, on a line of its own after the paragraph at the caret.
    /// Where it went, in the editor's text: the line, and whether it has a caption.
    fn write_figures_put(&mut self, id: &str, cx: &mut Context<Self>) -> Option<(Range<usize>, bool)> {
        let figure = self.lib.as_ref()?.gallery.figure(id)?.clone();
        // One cut from a paper says so where it stands: its caption cites the paper.
        let key = if figure.paper_id.is_empty() { None } else { self.cite_key_kept(&figure.paper_id, cx) };
        let caption = caption_of(&figure.legend, key.as_deref());
        let line = manuscript::picture_markdown(&caption, &figure.local_path);
        let lib = self.lib.as_mut()?;
        if lib.writing.section_id.is_empty() { return None }
        // Read through, the manuscript is not written in: the section is brought back to be.
        lib.writing.reading = false;
        let placed = lib.writing.editor.update(cx, |input, cx| {
            let (at, words, lead) = manuscript::place_picture(input.text(), input.caret(), &line);
            input.replace_range(at..at, &words, cx);
            let start = at + lead;
            // With no caption yet the caret waits between the brackets; with one, after the line.
            let caret = if caption.is_empty() { start + 2 } else { start + line.len() };
            input.select_range(caret..caret, cx);
            start..start + line.len()
        });
        Some((placed, !caption.is_empty()))
    }

    fn write_figures_insert(&mut self, id: &str, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, captioned)) = self.write_figures_put(id, cx) else { return self.fail(t("没有放进去：先在大纲里选一节。"), cx) };
        if close { self.run(Cmd::CloseModal, window, cx) }
        if let Some(lib) = self.lib.as_ref() { lib.writing.editor.read(cx).focus(window) }
        self.say(if captioned { t("图片已放进稿件，图注在它下面，可以直接改。") } else { t("图片已放进稿件，在方括号里写图注。") }, cx);
    }

    /// Pictures pasted into the manuscript, or files dropped on it: kept in the figure library and put in at the caret.
    pub(crate) fn manuscript_pasted(&mut self, pasted: Vec<Image>, cx: &mut Context<Self>) {
        self.write_figures_paste(pasted, true, cx)
    }

    // ---- the manuscript's pictures

    /// The pictures at these addresses as far as they are read; the reading of the others is started, a few at a time.
    pub(crate) fn manuscript_shots(&mut self, addresses: &[String], cx: &mut Context<Self>) -> Vec<(String, Arc<RenderImage>)> {
        let Some(lib) = self.lib.as_mut() else { return vec![] };
        let mut ready = vec![];
        for address in addresses {
            let held = &mut lib.write_figures;
            if let Some(image) = held.shots.get(address).cloned() {
                if held.recent.back() != Some(address) {
                    held.recent.retain(|other| other != address);
                    held.recent.push_back(address.clone());
                }
                ready.push((address.clone(), image));
                continue;
            }
            if held.broken.contains(address) || held.loading.contains(address) || held.loading.len() >= 3 || !manuscript::library_picture(address) { continue }
            held.loading.insert(address.clone());
            let path = lib.store.root().join(address.replace('/', std::path::MAIN_SEPARATOR_STR));
            let (library, address) = (lib.info.id.clone(), address.clone());
            let work = cx.background_executor().spawn(async move { std::fs::read(&path).ok().and_then(|bytes| crate::figures::pixels(&bytes, SHOT_EDGE)) });
            cx.spawn(async move |this, cx| {
                let picture = work.await;
                this.update(cx, |this, cx| {
                    let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
                    let held = &mut lib.write_figures;
                    held.loading.remove(&address);
                    match picture {
                        Some(picture) => {
                            held.shots.insert(address.clone(), Arc::new(RenderImage::new(vec![image::Frame::new(picture)])));
                            held.recent.push_back(address);
                            while held.recent.len() > KEEP_SHOTS {
                                if let Some(old) = held.recent.pop_front() && let Some(image) = held.shots.remove(&old) { cx.drop_image(image, None) }
                            }
                        }
                        None => {
                            held.broken.insert(address);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        ready
    }

    /// A picture where the manuscript is read through: the picture in the middle of the column, and its caption under it.
    pub(crate) fn render_manuscript_picture(&self, id: String, picture: &manuscript::PictureLine, width: f32, window: &Window) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return div().into_any_element() };
        let drawn = match lib.write_figures.shot(&picture.address) {
            Shot::Ready(image) => {
                let natural = image.size(0);
                let (w, h) = shot_size((natural.width.0 as f32, natural.height.0 as f32), width, window.scale_factor().min(2.));
                img(image).w(px(w)).h(px(h)).max_w_full().into_any_element()
            }
            Shot::Loading => div().flex().items_center().justify_center().w_full().h(px(120.)).rounded(px(6.)).bg(Theme::bg_sunken()).child(ui::spinner(SharedString::from(format!("{id}-loading")), 16., Theme::text_tertiary())).into_any_element(),
            Shot::Broken => div().flex().items_center().gap(px(8.)).px(px(12.)).h(px(44.)).rounded(px(6.)).bg(Theme::bg_sunken()).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(ui::icon("image-broken", 16., Theme::text_placeholder())).child(tf!("图片不在了：{}", "Picture missing: {}", picture.address)).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(SHOT_GAP))
            .child(drawn)
            .when(!picture.caption.trim().is_empty(), |d| d.child(div().w_full().text_size(text_px(13.)).text_color(Theme::text_secondary()).child(crate::markdown::render(&id, &picture.caption, None, None))))
            .into_any_element()
    }

    // ---- the dialog

    /// The figures on show, as places in the gallery's list.
    fn write_figures_shown(&self, cx: &App) -> Vec<usize> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let used = self.write_figures_used();
        let used: HashSet<&str> = used.iter().map(String::as_str).collect();
        let words = figure::search_words(lib.write_figures.search.read(cx).text());
        let by_id: HashMap<&str, &wisp_core::model::Paper> = if words.is_empty() { HashMap::new() } else { lib.papers.iter().map(|paper| (paper.id.as_str(), paper)).collect() };
        (0..lib.gallery.items.len()).filter(|&at| {
            let figure = &lib.gallery.items[at];
            lib.write_figures.only.has(figure, &used) && figure.matches(by_id.get(figure.paper_id.as_str()).copied(), &words)
        }).collect()
    }

    pub(crate) fn render_write_figures_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        // Just opened: the keyboard is the search box's, so words typed look and a picture pasted is brought in.
        if let Some(lib) = self.lib.as_mut() && std::mem::take(&mut lib.write_figures.opened) { lib.write_figures.search.read(cx).focus(window) }
        let shown = Rc::new(self.write_figures_shown(cx));
        let lib = self.lib.as_ref()?;
        let held = &lib.write_figures;
        let viewport = window.viewport_size();
        let (width, height) = ((f32::from(viewport.width) * 0.92).min(940.), (f32::from(viewport.height) * 0.88).min(680.));
        let (columns, card) = grid(width);
        let used = self.write_figures_used();
        let used_set: HashSet<&str> = used.iter().map(String::as_str).collect();
        let items = &lib.gallery.items;
        let count = |only: Only| items.iter().filter(|figure| only.has(figure, &used_set)).count();
        let chip = |id: &'static str, label: String, only: Only, this: &Self, cx: &mut Context<Self>| {
            let chosen = held.only == only;
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .h(text_px(26.))
                .px(px(10.))
                .rounded(px(13.))
                .border_1()
                .cursor_pointer()
                .text_size(text_px(12.))
                .when(chosen, |d| d.bg(Theme::accent_soft()).border_color(Theme::accent_soft()).text_color(Theme::accent()))
                .when(!chosen, |d| d.border_color(Theme::line_strong()).text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                .on_click(this.on(Cmd::WriteFigures(Do::Only(only)), cx))
                .child(label)
        };
        // A kind is offered while there are figures of it: a number says there is something there.
        let mut chips = vec![chip("write-figures-all", tf!("全部 {}", "All {}", items.len()), Only::All, self, cx).into_any_element()];
        for (id, only, zh, en) in [("write-figures-uploaded", Only::Uploaded, "我上传的", "Uploaded"), ("write-figures-papers", Only::Papers, "文献里收藏的", "From papers"), ("write-figures-drawn", Only::Drawn, "模型生成的", "Drawn by a model"), ("write-figures-used", Only::Used, "本稿已用", "In this manuscript")] {
            let n = count(only);
            if n > 0 || held.only == only { chips.push(chip(id, format!("{} {n}", if crate::i18n::is_english() { en } else { zh }), only, self, cx).into_any_element()) }
        }
        let body = if items.is_empty() {
            ui::empty_state("image", t("资料库里还没有图片"), t("点「上传图片」把自己的图放进来，也可以把图片文件拖到这里，或者直接粘贴。在文献的 PDF 上截图收藏的图，也会出现在这里。")).into_any_element()
        } else if shown.is_empty() {
            ui::empty_state("search", t("没有匹配的图片"), t("换一个搜索词，或者点「全部」看所有图片。")).into_any_element()
        } else {
            let rows = shown.len().div_ceil(columns);
            let shown = shown.clone();
            uniform_list("write-figures", rows, cx.processor(move |this, range: Range<usize>, _, cx| this.render_write_figure_rows(range, &shown, columns, card, cx))).track_scroll(held.scroll.clone()).flex_1().into_any_element()
        };
        let focused = held.search.focus_handle(cx).is_focused(window);
        let adding = held.adding;
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .child(div().flex().flex_1().min_w_0().items_center().gap(px(8.)).h(text_px(34.)).px(px(10.)).rounded(px(8.)).bg(Theme::bg_sunken()).border_1().border_color(if focused { Theme::accent() } else { Theme::bg_sunken() }).child(ui::icon("search", 15., Theme::text_tertiary())).child(held.search.clone()))
            .when(adding > 0, |d| d.child(ui::spinner("write-figures-adding", 14., Theme::accent())))
            .child(ui::primary_button("write-figures-upload", t("上传图片")).flex_none().h(text_px(34.)).on_click(self.on(Cmd::WriteFigures(Do::Upload), cx)));
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .px(px(20.))
            .py(px(14.))
            .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("点一张图，把它放进稿件光标所在的段落后面 · 图片文件可以拖进来，也可以粘贴")))
            .child(ui::secondary_button("modal-cancel", crate::i18n::tc("dialog.close", "关闭")).flex_none().on_click(self.on(Cmd::CloseModal, cx)));
        Some(
            ui::dialog(width)
                .h(px(height))
                // Picture files dropped on the dialog are brought in; they are not the library's to import as papers.
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.write_figures_add(paths.paths().to_vec(), false, cx)))
                .child(ui::dialog_title(t("资料库")))
                .child(head)
                .child(div().id("write-figures-kinds").flex().flex_wrap().flex_none().items_center().gap(px(6.)).px(px(20.)).pt(px(10.)).children(chips))
                .child(div().flex().flex_col().flex_1().min_h_0().pt(px(GAP)).child(body))
                .child(footer),
        )
    }

    fn render_write_figure_rows(&mut self, range: Range<usize>, shown: &[usize], columns: usize, card: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let used = self.write_figures_used();
        range
            .map(|row| {
                let cards: Vec<AnyElement> = shown.iter().skip(row * columns).take(columns).map(|&at| self.render_write_figure_card(at, card, &used, cx)).collect();
                div().flex().flex_none().gap(px(GAP)).w_full().h(px(PICTURE_H + GAP) + text_px(WORDS_H)).px(px(20.)).pb(px(GAP)).children(cards).into_any_element()
            })
            .collect()
    }

    fn render_write_figure_card(&mut self, at: usize, width: f32, used: &[String], cx: &mut Context<Self>) -> AnyElement {
        let Some(figure) = self.lib.as_ref().and_then(|lib| lib.gallery.items.get(at)).cloned() else { return div().into_any_element() };
        let thumb = self.figure_thumb(&figure.id, cx);
        let lib = self.lib.as_ref().unwrap();
        let picture = match thumb {
            Thumb::Ready(image) => {
                let natural = image.size(0);
                let (w, h) = crate::figures::fit((natural.width.0 as f32, natural.height.0 as f32), (width - 14., PICTURE_H - 12.));
                img(image).w(px(w)).h(px(h)).into_any_element()
            }
            Thumb::Loading => div().into_any_element(),
            Thumb::Broken => ui::icon("image-broken", 22., Theme::text_placeholder()).into_any_element(),
        };
        // What it is called: what is written about it, else the file it was, else where it is from.
        let source = if figure.uploaded() { t("我上传的").to_string() } else if figure.drawn() { t("模型生成").to_string() } else { crate::figures::source_line(lib.paper(&figure.paper_id)) };
        let (head, below) = if !figure.headline().is_empty() {
            (ui::one_line(figure.headline()), source)
        } else if !figure.file_name.is_empty() {
            (figure.file_name.clone(), source)
        } else {
            (source, t("还没有图注").to_string())
        };
        let in_use = used.contains(&figure.local_path);
        let fresh = lib.write_figures.fresh.contains(&figure.id);
        let group = SharedString::from(format!("write-figure-{}", figure.id));
        let badge = |text: &'static str, color: Hsla| div().flex().flex_none().items_center().gap(px(3.)).h(text_px(18.)).px(px(6.)).rounded(px(9.)).bg(Theme::bg_body()).border_1().border_color(color.opacity(0.4)).text_size(text_px(10.)).text_color(color).child(text);
        div()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .h(px(PICTURE_H) + text_px(WORDS_H))
            .rounded(px(10.))
            .border_1()
            .border_color(Theme::line())
            .bg(Theme::bg_body())
            .overflow_hidden()
            .cursor_pointer()
            .hover(|s| s.border_color(Theme::accent()))
            .on_click(self.on(Cmd::WriteFigures(Do::Insert(figure.id.clone())), cx))
            .child(div().flex().flex_none().items_center().justify_center().h(px(PICTURE_H)).bg(gpui::white()).border_b_1().border_color(Theme::line()).child(picture))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .justify_center()
                    .gap(px(2.))
                    .px(px(10.))
                    .child(div().truncate().text_size(text_px(13.)).font_weight(FontWeight::MEDIUM).child(head))
                    .child(div().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(ui::one_line(&below))),
            )
            // What was done with it: the manuscript has it already, or it was just brought in.
            .child(div().absolute().top(px(6.)).left(px(6.)).flex().gap(px(4.)).when(in_use, |d| d.child(badge(t("已用"), Theme::success()).child(ui::icon("check", 10., Theme::success())))).when(fresh, |d| d.child(badge(t("刚上传"), Theme::accent()))))
            // Its words are changed in the figure's own dialog, which comes back here.
            .child(
                div().absolute().top(px(5.)).right(px(5.)).opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                    ui::icon_action(SharedString::from(format!("write-figure-edit-{}", figure.id)), "pen-line", Theme::text_secondary(), t("查看大图，改图注、标签和备注")).bg(Theme::bg_body()).on_click({
                        let edit = self.on(Cmd::WriteFigures(Do::Edit(figure.id.clone())), cx);
                        move |event, window, cx| {
                            // The card under it is not clicked too.
                            cx.stop_propagation();
                            edit(event, window, cx)
                        }
                    }),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{CARD_MIN, GAP, Only, Workspace, caption_of, grid, kept_bytes, shot_size};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use wisp_core::figure::{Figure, UPLOADED, picture_kind};

    #[test]
    fn a_caption_cites_the_paper_its_figure_is_from() {
        assert_eq!(caption_of("WOX5 marks the\nquiescent  centre.", Some("Zhang2021")), "WOX5 marks the quiescent centre [@Zhang2021].");
        assert_eq!(caption_of("根尖干细胞微环境。", Some("Li2020")), "根尖干细胞微环境 [@Li2020]。");
        assert_eq!(caption_of("No stop at its end", Some("Zhang2021")), "No stop at its end [@Zhang2021]");
        assert_eq!(caption_of("", Some("Zhang2021")), "[@Zhang2021]");
        // The person's own figure cites nothing.
        assert_eq!((caption_of(" Root length. ", None), caption_of("", None)), ("Root length.".to_string(), String::new()));
    }

    #[test]
    fn figures_are_told_apart_by_where_they_are_from() {
        let own = Figure { origin: UPLOADED.into(), local_path: "figures/G-1.png".into(), ..Default::default() };
        let cut = Figure { paper_id: "P-1".into(), local_path: "figures/G-2.png".into(), ..Default::default() };
        let drawn = Figure { local_path: "figures/G-3.png".into(), ..Default::default() };
        let used: HashSet<&str> = ["figures/G-2.png"].into_iter().collect();
        let kinds = |figure: &Figure| [Only::All, Only::Uploaded, Only::Papers, Only::Drawn, Only::Used].map(|only| only.has(figure, &used));
        assert_eq!(kinds(&own), [true, true, false, false, false]);
        assert_eq!(kinds(&cut), [true, false, true, false, true]);
        assert_eq!(kinds(&drawn), [true, false, false, true, false]);
    }

    #[test]
    fn pictures_brought_in_are_kept_as_png_or_jpeg() {
        let encoded = |format: image::ImageFormat| {
            let mut out = vec![];
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 20, image::Rgb([200, 40, 10]))).write_to(&mut std::io::Cursor::new(&mut out), format).unwrap();
            out
        };
        // A PNG and a JPEG stay as they are, to the byte; any other kind becomes a PNG of the same picture.
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            assert_eq!(kept_bytes(encoded(format)).unwrap(), encoded(format));
        }
        for format in [image::ImageFormat::WebP, image::ImageFormat::Bmp, image::ImageFormat::Tiff, image::ImageFormat::Gif] {
            let kept = kept_bytes(encoded(format)).unwrap();
            assert_eq!((picture_kind(&kept), image::load_from_memory(&kept).unwrap().width()), (Some("png"), 40), "{format:?}");
        }
        assert!(kept_bytes(b"%PDF-1.7 not a picture".to_vec()).is_err());
        let names = |names: &[&str]| names.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert!(Workspace::pictures_by_name(&names(&["a.png", "b.JPG", "c.tiff"])));
        assert!(!Workspace::pictures_by_name(&names(&["a.png", "paper.pdf"])) && !Workspace::pictures_by_name(&[]));
    }

    #[test]
    fn cards_and_pictures_fit_their_room() {
        for width in [300., 640., 940.] {
            let (columns, card) = grid(width);
            assert!(card >= CARD_MIN - 0.01 && ((card * columns as f32 + GAP * (columns - 1) as f32) - (width - 40.).max(CARD_MIN)).abs() < 0.01, "{width}");
        }
        assert_eq!(grid(940.).0, 4);
        // A picture stands whole in the column: at its own size on a dense screen, never wider or taller than the room.
        assert_eq!(shot_size((800., 400.), 700., 2.), (400., 200.));
        assert_eq!(shot_size((2800., 1400.), 700., 2.), (700., 350.));
        assert_eq!(shot_size((1000., 3000.), 700., 1.), (120., 360.));
    }
}
