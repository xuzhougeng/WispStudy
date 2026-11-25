//! 方法库 and 导言库: passages kept from the library's papers in their own
//! words (`wisp_core::excerpt`), of two kinds: experimental methods, and
//! introductions (how a paper leads into its question). Both stand in 收藏
//! beside the starred papers and the figures (`render_collection_switch`),
//! each a page of its own drawn by the same code with the kind as a
//! parameter (`Page::Methods`, `Page::Intros`). A passage is kept from the
//! PDF: selected there, 收藏为方法 or 收藏为导言 opens its dialog with the
//! paper's words as they were selected, for the person to name, tag and
//! annotate. The page lists them as rows, to be searched and narrowed to a
//! tag, so passages of a kind stand together and can be compared; a row
//! opens the dialog, which leads back to the page the words stand on.
//!
//! A method is linked to figures of 图片库: what it produced. The links are
//! set in the method's dialog (关联图片: the figures as small pictures, and a
//! list to add one from, the paper's own first); a figure's dialog names the
//! methods linked to it, each a way to that method. An introduction has no
//! figures. 用这段提问 hands a passage to the library's conversation, as a
//! passage selected in a PDF is.

use crate::app::{Cmd, MenuItem, MenuKind, Modal, Page, Talk, Workspace};
use crate::chat_state::{ChatQuote, MAX_QUOTES};
use crate::figures::{Thumb, fit, source_line};
use crate::i18n::{t, tc};
use crate::input::{InputEvent, TextInput};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use wisp_core::excerpt::{self, Excerpt, INTRO, METHOD};
use wisp_core::figure::{self, Figure};
use wisp_core::model::{Paper, split_list};

/// The height of a row of the page, and the size of a linked figure's picture there and in the dialog.
const ROW_H: f32 = 112.;
const ROW_THUMB: (f32, f32) = (64., 48.);
const DIALOG_THUMB: (f32, f32) = (88., 66.);

/// How many linked figures a row shows; the rest are counted.
const ROW_FIGURES: usize = 3;

/// The width of the dialog, and of its fields.
const DIALOG_W: f32 = 620.;
const WIDE: f32 = DIALOG_W - 40.;

/// How many figures the list to link one from shows at a time: the search finds the others.
const PICK_SHOWN: usize = 40;

/// Everything the two pages and the dialog do, as one command (`Cmd::Excerpt`).
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// Only the passages of a kind with this tag; the tag chosen already shows all of them.
    Tag(String, String),
    /// A passage's dialog.
    Open(String),
    /// The dialog of a passage of a kind the person writes down, with no paper.
    New(String),
    /// Open the PDF a passage was selected in, at its page.
    Source(String),
    /// Hand a passage to the library's conversation.
    Ask(String),
    AskRemove(String),
    Remove(String),
    /// Add one of the tags on offer to the dialog's tags.
    AddTag(String),
    /// Open or close the dialog's list of figures to link.
    Pick,
    /// Link a figure in the dialog, or let it go.
    Link(String),
    Unlink(String),
}

/// The search, the tag chosen and the place scrolled to of one kind's page.
pub struct Shelf {
    pub search: Entity<TextInput>,
    /// Only those with this tag; empty for all of them.
    pub tag: String,
    pub scroll: UniformListScrollHandle,
}

/// The passages of the open library, of both kinds, and what the pages need to show them.
pub struct Excerpts {
    /// Every passage, the newest first.
    pub items: Vec<Excerpt>,
    /// Every link, as (passage, figure), each passage's in the person's order (`Store::excerpt_links`).
    pub links: Vec<(String, String)>,
    pub methods: Shelf,
    pub intros: Shelf,
}

impl Excerpts {
    pub fn new(subscriptions: &mut Vec<Subscription>, cx: &mut Context<Workspace>) -> Excerpts {
        let mut shelf = |kind: &'static str, cx: &mut Context<Workspace>| {
            let search = cx.new(|cx| TextInput::new(if kind == METHOD { "搜索名称、原文、标签、备注，或所属文献…" } else { "搜索引入方式、原文、标签、备注，或所属文献…" }, cx));
            subscriptions.push(cx.subscribe(&search, move |this, _, event: &InputEvent, cx| {
                if *event != InputEvent::Changed { return }
                if let Some(lib) = this.lib.as_mut() { lib.excerpts.shelf_mut(kind).scroll = UniformListScrollHandle::new() }
                cx.notify();
            }));
            Shelf { search, tag: String::new(), scroll: UniformListScrollHandle::new() }
        };
        let methods = shelf(METHOD, cx);
        let intros = shelf(INTRO, cx);
        Excerpts { items: vec![], links: vec![], methods, intros }
    }

    pub fn shelf(&self, kind: &str) -> &Shelf {
        if kind == INTRO { &self.intros } else { &self.methods }
    }

    pub fn shelf_mut(&mut self, kind: &str) -> &mut Shelf {
        if kind == INTRO { &mut self.intros } else { &mut self.methods }
    }

    pub fn excerpt(&self, id: &str) -> Option<&Excerpt> {
        self.items.iter().find(|excerpt| excerpt.id == id)
    }

    /// How many of a kind there are.
    pub fn count(&self, kind: &str) -> usize {
        self.items.iter().filter(|excerpt| excerpt.kind == kind).count()
    }

    /// The figures a passage is linked to, in the person's order.
    pub fn figures_of(&self, id: &str) -> Vec<String> {
        self.links.iter().filter(|(excerpt, _)| excerpt == id).map(|(_, figure)| figure.clone()).collect()
    }

    /// The methods listed that are linked to a figure, the newest first.
    pub fn methods_of(&self, figure: &str) -> Vec<&Excerpt> {
        self.items.iter().filter(|excerpt| excerpt.kind == METHOD && self.links.iter().any(|(e, f)| *e == excerpt.id && f == figure)).collect()
    }

    /// The passages of a kind on show, as places in `items`: those with the tag chosen that say every word searched for.
    fn shown(&self, kind: &str, papers: &[Paper], query: &str) -> Vec<usize> {
        let words = figure::search_words(query);
        let tag = &self.shelf(kind).tag;
        let by_id: HashMap<&str, &Paper> = if words.is_empty() { HashMap::new() } else { papers.iter().map(|paper| (paper.id.as_str(), paper)).collect() };
        (0..self.items.len())
            .filter(|&at| {
                let excerpt = &self.items[at];
                excerpt.kind == kind && (tag.is_empty() || excerpt.tags.contains(tag)) && excerpt.matches(by_id.get(excerpt.paper_id.as_str()).copied(), &words)
            })
            .collect()
    }
}

/// A passage as the dialog holds it: one of the library, or one just selected that is not kept yet.
pub struct ExcerptForm {
    /// The passage looked at; `None` for a new one.
    pub id: Option<String>,
    /// `METHOD` or `INTRO`.
    pub kind: String,
    pub paper_id: String,
    resource_id: String,
    pub page: usize,
    pub name: Entity<TextInput>,
    pub text: Entity<TextInput>,
    pub tags: Entity<TextInput>,
    pub notes: Entity<TextInput>,
    /// The figures linked, in order: kept with the rest. A method's only.
    pub figures: Vec<String>,
    /// The list of figures to link one from is open, and what is searched in it.
    pub picking: bool,
    pub find: Entity<TextInput>,
    /// Not drawn yet: with the first frame the keyboard goes to the name.
    fresh: bool,
}

/// The order the list to link a figure from shows them in: the passage's paper's own first, then the rest as the
/// library lists them (the newest first); none linked already, only those that say every word searched for.
fn pick_order(figures: &[Figure], paper_id: &str, linked: &[String], papers: &HashMap<&str, &Paper>, words: &[String]) -> Vec<usize> {
    let fits = |figure: &Figure| !linked.contains(&figure.id) && figure.matches(papers.get(figure.paper_id.as_str()).copied(), words);
    let own = (0..figures.len()).filter(|&at| !paper_id.is_empty() && figures[at].paper_id == paper_id && fits(&figures[at]));
    let rest = (0..figures.len()).filter(|&at| (paper_id.is_empty() || figures[at].paper_id != paper_id) && fits(&figures[at]));
    own.chain(rest).collect()
}

/// What a passage's words are cut to in a row: one line's worth of text, its breaks as spaces.
fn row_text(excerpt: &Excerpt) -> String {
    ui::one_line(&excerpt.text).chars().take(400).collect()
}

impl Workspace {
    // ---- commands

    pub(crate) fn run_excerpts(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Tag(kind, tag) => {
                let Some(lib) = self.lib.as_mut() else { return };
                let shelf = lib.excerpts.shelf_mut(&kind);
                shelf.tag = if shelf.tag == tag { String::new() } else { tag };
                shelf.scroll = UniformListScrollHandle::new();
            }
            Do::Open(id) => self.excerpt_dialog(&id, cx),
            Do::New(kind) => {
                if self.lib.is_none() { return }
                let form = self.excerpt_form(&Excerpt { kind, ..Default::default() }, cx);
                self.show_excerpt_form(form, cx);
            }
            Do::Source(id) => self.excerpt_source(&id, window, cx),
            Do::Ask(id) => self.ask_excerpt(&id, window, cx),
            Do::AskRemove(id) => {
                let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(&id)) else { return };
                let method = excerpt.kind == METHOD;
                let name = if excerpt.headline().is_empty() { t(if method { "这个方法" } else { "这段导言" }).to_string() } else { format!("「{}」", excerpt.headline().chars().take(40).collect::<String>()) };
                let (title, message, action) = if method {
                    (tf!("把{name}移出方法库？", "Remove {name} from methods?"), t("它的名称、原文、标签、备注和关联的图片会一起删除。图片库里的图和 PDF 都不动。"), t("移出方法库"))
                } else {
                    (tf!("把{name}移出导言库？", "Remove {name} from introductions?"), t("它的名称、原文、标签和备注会一起删除。PDF 不动。"), t("移出导言库"))
                };
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                self.modal = Some(Modal::Confirm { title, message: message.into(), action: action.into(), cmd: Cmd::Excerpt(Do::Remove(id)) });
            }
            Do::Remove(id) => {
                let method = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(&id)).is_none_or(|excerpt| excerpt.kind == METHOD);
                self.run(Cmd::CloseModal, window, cx);
                if self.change(cx, |lib| lib.store.remove_excerpt(&id)) { self.say(if method { t("已移出方法库。") } else { t("已移出导言库。") }, cx) }
            }
            Do::AddTag(tag) => {
                let Some(Modal::Excerpt(form)) = self.modal.as_ref() else { return };
                form.tags.update(cx, |input, cx| {
                    let mut tags = split_list(input.text());
                    if !tags.contains(&tag) { tags.push(tag) }
                    input.replace_range(0..input.text().len(), &tags.join(", "), cx);
                });
            }
            Do::Pick => {
                let Some(Modal::Excerpt(form)) = self.modal.as_mut() else { return };
                form.picking = !form.picking;
                if form.picking { form.find.read(cx).focus(window) }
            }
            Do::Link(figure) => {
                let Some(Modal::Excerpt(form)) = self.modal.as_mut() else { return };
                if !form.figures.contains(&figure) { form.figures.push(figure) }
            }
            Do::Unlink(figure) => {
                let Some(Modal::Excerpt(form)) = self.modal.as_mut() else { return };
                form.figures.retain(|id| *id != figure);
            }
        }
        cx.notify();
    }

    fn excerpt_form(&mut self, from: &Excerpt, cx: &mut Context<Self>) -> ExcerptForm {
        let input = |placeholder: &'static str, lines: usize, text: &str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let input = TextInput::new(placeholder, cx);
                if lines > 1 { input.multiline(lines).with_text(text) } else { input.with_text(text) }
            })
        };
        let method = from.kind == METHOD;
        let figures = match self.lib.as_ref() {
            Some(lib) if !from.id.is_empty() => lib.excerpts.figures_of(&from.id),
            _ => vec![],
        };
        ExcerptForm {
            id: Some(from.id.clone()).filter(|id| !id.is_empty()),
            kind: from.kind.clone(),
            paper_id: from.paper_id.clone(),
            resource_id: from.resource_id.clone(),
            page: from.page,
            name: input(if method { "例如 Western blot、根尖原生质体分离" } else { "它怎样引入：例如 从临床问题切入、从方法的局限切入" }, 1, &from.name, cx),
            text: input(if method { "文献里的原文：步骤、试剂、参数" } else { "文献里的原文：它开篇和引入问题的那几句" }, 6, &from.text, cx),
            tags: input(if method { "用逗号隔开，例如 蛋白, 免疫印迹" } else { "用逗号隔开，例如 空白, 背景, 目的" }, 1, &from.tags.join(", "), cx),
            notes: input(if method { "为什么适合、改了哪些参数、依据是什么" } else { "想借鉴什么：怎样铺背景、怎样点出空白、怎样落到目的" }, 3, &from.notes, cx),
            figures,
            picking: false,
            find: input("搜索图号、图注、标签，或所属文献…", 1, "", cx),
            fresh: true,
        }
    }

    fn show_excerpt_form(&mut self, form: ExcerptForm, cx: &mut Context<Self>) {
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        // What is searched for in the list of figures is drawn as it is typed.
        self.modal_subscriptions.push(cx.subscribe(&form.find, |_, _, event: &InputEvent, cx| if *event == InputEvent::Changed { cx.notify() }));
        self.modal = Some(Modal::Excerpt(Box::new(form)));
        cx.notify();
    }

    /// A passage of the library, to look at and to change.
    fn excerpt_dialog(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(id)).cloned() else { return };
        let form = self.excerpt_form(&excerpt, cx);
        self.show_excerpt_form(form, cx);
    }

    /// 收藏为方法 / 收藏为导言: a passage selected on a 1-based page of a PDF of a paper, for the person to name.
    pub(crate) fn collect_excerpt(&mut self, kind: &str, paper_id: String, resource_id: String, page: usize, text: &str, cx: &mut Context<Self>) {
        if self.lib.is_none() { return }
        let excerpt = Excerpt { kind: kind.to_string(), paper_id, resource_id, page, text: text.trim().to_string(), ..Default::default() };
        let form = self.excerpt_form(&excerpt, cx);
        self.show_excerpt_form(form, cx);
    }

    /// Before the dialogs are drawn: a passage's dialog just opened gets the keyboard, in its name.
    pub(crate) fn tend_excerpt_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Excerpt(form)) = self.modal.as_mut() else { return };
        if !std::mem::take(&mut form.fresh) { return }
        if form.id.is_none() { form.name.read(cx).focus(window) }
    }

    /// The PDF a passage was selected in, at its page, the words lit as a citation's are.
    fn excerpt_source(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(id)).cloned() else { return };
        if excerpt.paper_id.is_empty() { return }
        self.run(Cmd::CloseModal, window, cx);
        if self.lib.as_ref().is_some_and(|lib| lib.paper(&excerpt.paper_id).is_none()) { return self.fail(t("这段原文的文献已不在库里。"), cx) }
        let attachment = self.lib.as_ref().and_then(|lib| lib.attachments(&excerpt.paper_id).iter().find(|f| f.id == excerpt.resource_id).map(|f| f.id.clone()));
        let quote = excerpt.text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_string();
        self.run(Cmd::GotoFile(excerpt.paper_id.clone(), attachment, (excerpt.page > 0).then_some(excerpt.page), quote), window, cx);
    }

    /// 用这段提问: the passage's words go over the library's question box, as a passage selected in a PDF does,
    /// and the box is begun with its name, for the question to say what to compare it with or what to change.
    fn ask_excerpt(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(id)).cloned() else { return };
        self.run(Cmd::CloseModal, window, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let held = !excerpt.paper_id.is_empty() && lib.paper(&excerpt.paper_id).is_some();
        let method = excerpt.kind == METHOD;
        let chat = lib.chat_mut(Talk::Library);
        let begun = if held {
            if chat.quotes.len() >= MAX_QUOTES { return self.fail(tf!("一个问题最多带 {MAX_QUOTES} 段原文。先发出去，或者去掉一段。", "A question can include at most {MAX_QUOTES} passages. Send it first or remove one."), cx) }
            let quote = ChatQuote { paper_id: excerpt.paper_id.clone(), file_id: excerpt.resource_id.clone(), page: excerpt.page.max(1), text: excerpt.text.clone() };
            if !chat.quotes.contains(&quote) { chat.quotes.push(quote) }
            if method { tf!("关于方法「{}」：", "About the method “{}”: ", excerpt.headline()) } else { tf!("关于这段导言「{}」：", "About the introduction “{}”: ", excerpt.headline()) }
        } else if method {
            // One written down without a paper has no page to quote: its words go into the box.
            tf!("关于我记下的方法「{}」：\n{}\n\n", "About the method I wrote down, “{}”:\n{}\n\n", excerpt.headline(), excerpt.text)
        } else {
            tf!("关于我记下的这段导言「{}」：\n{}\n\n", "About the introduction I wrote down, “{}”:\n{}\n\n", excerpt.headline(), excerpt.text)
        };
        chat.history = None;
        let input = chat.input.clone();
        // Begun as if typed: the caret after it, for the question to follow.
        input.update(cx, |input, cx| if input.text().trim().is_empty() { input.replace_text(0..input.text().len(), &begun, cx) });
        lib.focus_box = Some(Talk::Library);
        self.run(Cmd::SetPage(Page::Ask), window, cx);
    }

    pub(crate) fn submit_excerpt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Excerpt(form)) = self.modal.as_ref() else { return };
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let mut excerpt = Excerpt { id: form.id.clone().unwrap_or_default(), kind: form.kind.clone(), paper_id: form.paper_id.clone(), resource_id: form.resource_id.clone(), page: form.page, name: text(&form.name), text: text(&form.text), notes: text(&form.notes), tags: split_list(&text(&form.tags)), ..Default::default() };
        let (fresh, method) = (form.id.is_none(), form.kind == METHOD);
        // An introduction has no figures; a method keeps those its dialog links.
        let figures = if method { form.figures.clone() } else { vec![] };
        let mut failed = String::new();
        let kept = self.change(cx, |lib| {
            let result = (|| {
                if fresh {
                    lib.store.add_excerpt(&mut excerpt)?;
                } else {
                    let held = lib.store.get_excerpt(&excerpt.id)?;
                    excerpt = Excerpt { name: excerpt.name.clone(), text: excerpt.text.clone(), notes: excerpt.notes.clone(), tags: excerpt.tags.clone(), ..held };
                    lib.store.save_excerpt(&mut excerpt)?;
                }
                if method { lib.store.set_excerpt_figures(&excerpt.id, &figures) } else { Ok(()) }
            })();
            // Said in the dialog, which stays open with what was typed.
            if let Err(err) = &result { failed = format!("{err:#}") }
            Ok(())
        });
        if !failed.is_empty() {
            self.modal_error = failed;
            return cx.notify();
        }
        if !kept { return }
        self.run(Cmd::CloseModal, window, cx);
        if fresh { self.say(if method { t("已收藏到方法库，在左侧「收藏」里的「方法」下。") } else { t("已收藏到导言库，在左侧「收藏」里的「导言」下。") }, cx) }
    }

    // ---- the dialog

    pub(crate) fn render_excerpt_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::Excerpt(form)) = self.modal.as_ref() else { return None };
        let lib = self.lib.as_ref()?;
        let method = form.kind == METHOD;
        // The pictures wanted: those linked, and those the list to link from shows.
        let picks: Vec<String> = if method && form.picking {
            let papers: HashMap<&str, &Paper> = lib.papers.iter().map(|paper| (paper.id.as_str(), paper)).collect();
            let words = figure::search_words(form.find.read(cx).text());
            pick_order(&lib.gallery.items, &form.paper_id, &form.figures, &papers, &words).into_iter().take(PICK_SHOWN).map(|at| lib.gallery.items[at].id.clone()).collect()
        } else {
            vec![]
        };
        let linked: Vec<String> = if method { form.figures.iter().filter(|id| lib.gallery.figure(id).is_some()).cloned().collect() } else { vec![] };
        let wanted: Vec<String> = linked.iter().chain(&picks).cloned().collect();
        let thumbs: HashMap<String, Thumb> = wanted.into_iter().map(|id| { let thumb = self.figure_thumb(&id, cx); (id, thumb) }).collect();
        let Some(Modal::Excerpt(form)) = self.modal.as_ref() else { return None };
        let lib = self.lib.as_ref()?;
        let fresh = form.id.is_none();
        let paper = lib.paper(&form.paper_id);

        // Where its words are from, and the ways from a kept passage to its page and to the conversation.
        let mut where_from = if !form.paper_id.is_empty() { source_line(paper) } else if method { t("我记下的方法，不出自任何文献").to_string() } else { t("我记下的导言，不出自任何文献").to_string() };
        if form.page > 0 { where_from = tf!("第 {} 页 · {where_from}", "Page {} · {where_from}", form.page) }
        let tool = |id: &'static str, label: &'static str, cmd: Cmd, this: &Self, cx: &mut Context<Self>| ui::ghost_button(id, None, Some(label.into())).flex_none().h(text_px(26.)).text_size(text_px(12.)).on_click(this.on(cmd, cx));
        let source = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .w(px(WIDE))
            .text_size(text_px(12.))
            .text_color(Theme::text_secondary())
            .child(div().flex_1().min_w_0().truncate().child(where_from))
            .when_some(form.id.clone(), |d, id| {
                d.when(paper.is_some(), |d| d.child(tool("excerpt-source", t("打开原文"), Cmd::Excerpt(Do::Source(id.clone())), self, cx).text_color(Theme::accent())))
                    .child(tool("excerpt-ask", t("用这段提问"), Cmd::Excerpt(Do::Ask(id)), self, cx))
            });

        // The tags the library's passages of this kind already carry, to take with a click.
        let typed = split_list(form.tags.read(cx).text());
        let offered: Vec<AnyElement> = excerpt::tag_counts(&lib.excerpts.items, &form.kind)
            .into_iter()
            .filter(|(tag, _)| !typed.contains(tag))
            .take(10)
            .enumerate()
            .map(|(i, (tag, _))| {
                div()
                    .id(("excerpt-offer", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(text_px(22.))
                    .px(px(8.))
                    .rounded(px(11.))
                    .border_1()
                    .border_color(Theme::line_strong())
                    .text_size(text_px(12.))
                    .text_color(Theme::text_secondary())
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()).border_color(Theme::accent()))
                    .on_click(self.on(Cmd::Excerpt(Do::AddTag(tag.clone())), cx))
                    .child(format!("+ {tag}"))
                    .into_any_element()
            })
            .collect();

        // 关联图片, a method's only: the figures linked as small pictures, each with its ×; a click on one opens that figure.
        let picture = |thumb: Option<&Thumb>, room: (f32, f32)| match thumb {
            Some(Thumb::Ready(image)) => {
                let natural = image.size(0);
                let (w, h) = fit((natural.width.0 as f32, natural.height.0 as f32), (room.0 - 6., room.1 - 6.));
                img(image.clone()).w(px(w)).h(px(h)).into_any_element()
            }
            Some(Thumb::Broken) => ui::icon("image-broken", 18., Theme::text_placeholder()).into_any_element(),
            _ => div().into_any_element(),
        };
        let shelf: Vec<AnyElement> = linked
            .iter()
            .filter_map(|id| lib.gallery.figure(id))
            .map(|figure| {
                let headline = if figure.headline().is_empty() { t("没有图号").to_string() } else { figure.headline().chars().take(24).collect() };
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(px(3.))
                    .w(px(DIALOG_THUMB.0))
                    .child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id(SharedString::from(format!("excerpt-figure-{}", figure.id)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(DIALOG_THUMB.0))
                                    .h(px(DIALOG_THUMB.1))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(Theme::line())
                                    .bg(gpui::white())
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(Theme::accent()))
                                    .on_click(self.on(Cmd::OpenFigure(figure.id.clone()), cx))
                                    .child(picture(thumbs.get(&figure.id), DIALOG_THUMB)),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("excerpt-unlink-{}", figure.id)))
                                    .absolute()
                                    .top(px(-6.))
                                    .right(px(-6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(18.))
                                    .rounded(px(9.))
                                    .bg(Theme::bg_body())
                                    .border_1()
                                    .border_color(Theme::line_strong())
                                    .cursor_pointer()
                                    .hover(|s| s.border_color(Theme::danger()))
                                    .on_click(self.on(Cmd::Excerpt(Do::Unlink(figure.id.clone())), cx))
                                    .tooltip(ui::tip(t("取消关联")))
                                    .child(ui::icon("x", 11., Theme::text_secondary())),
                            ),
                    )
                    .child(div().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(headline))
                    .into_any_element()
            })
            .collect();
        let figures = method.then(|| {
            let add = ui::ghost_button("excerpt-pick", Some(if form.picking { "chevron-up" } else { "plus" }), Some(if form.picking { t("收起") } else { t("添加") }.into())).flex_none().h(text_px(26.)).text_size(text_px(12.)).on_click(self.on(Cmd::Excerpt(Do::Pick), cx));
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(WIDE))
                .child(div().flex().items_center().justify_between().h(text_px(26.)).child(div().text_size(text_px(12.)).text_color(Theme::text_secondary()).child(t("关联图片"))).child(add))
                .when(shelf.is_empty() && !form.picking, |d| d.child(div().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(t("这个方法做出来的图，例如它得到的条带图、统计图。"))))
                .when(!shelf.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(12.)).pt(px(6.)).children(shelf)))
        });

        // The list to link a figure from: the paper's own first, then the rest, searched by their words.
        let picker = (method && form.picking).then(|| {
            let focused = form.find.focus_handle(cx).is_focused(window);
            let rows: Vec<AnyElement> = picks
                .iter()
                .filter_map(|id| lib.gallery.figure(id))
                .map(|figure| {
                    let own = !form.paper_id.is_empty() && figure.paper_id == form.paper_id;
                    let from = if figure.uploaded() { t("我上传的").to_string() } else if figure.drawn() { t("模型生成").to_string() } else { source_line(lib.paper(&figure.paper_id)) };
                    let head = if figure.label.is_empty() { ui::one_line(&figure.legend) } else { figure.label.clone() };
                    div()
                        .id(SharedString::from(format!("excerpt-pick-{}", figure.id)))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(10.))
                        .w_full()
                        .h(text_px(48.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Theme::hover()))
                        .on_click(self.on(Cmd::Excerpt(Do::Link(figure.id.clone())), cx))
                        .child(div().flex().flex_none().items_center().justify_center().w(px(48.)).h(px(36.)).rounded(px(4.)).border_1().border_color(Theme::line()).bg(gpui::white()).overflow_hidden().child(picture(thumbs.get(&figure.id), (48., 36.))))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(if head.is_empty() { t("没有图号").to_string() } else { head }))
                                .child(div().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(if own { tf!("这篇文献的图 · {from}", "From this paper · {from}") } else { from })),
                        )
                        .into_any_element()
                })
                .collect();
            let empty = if lib.gallery.items.is_empty() { Some(t("图片库里还没有图片。在 PDF 上截图收藏以后，可以在这里关联。")) } else if rows.is_empty() { Some(t("没有匹配的图片。")) } else { None };
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(WIDE))
                .p(px(8.))
                .rounded(px(10.))
                .border_1()
                .border_color(Theme::line_strong())
                .child(div().flex().items_center().gap(px(8.)).h(text_px(30.)).px(px(8.)).rounded(px(7.)).bg(Theme::bg_sunken()).border_1().border_color(if focused { Theme::accent() } else { Theme::bg_sunken() }).child(ui::icon("search", 14., Theme::text_tertiary())).child(div().flex_1().min_w_0().child(form.find.clone())))
                .when_some(empty, |d, empty| d.child(div().px(px(8.)).py(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(empty)))
                .when(!rows.is_empty(), |d| d.child(div().id("excerpt-picks").flex().flex_col().max_h(px(232.)).overflow_y_scroll().children(rows)))
        });

        // A wrapping field under percentages is measured too narrow: each gets the dialog's width in pixels.
        let row = |label: &'static str, input: &Entity<TextInput>| ui::form_row(label, div().w(px(WIDE)).child(ui::field(input, window, cx)));
        // The paper's words can run to half a page: their field keeps a height and scrolls.
        let tall = |id: &'static str, input: &Entity<TextInput>, most: f32| {
            let focused = input.focus_handle(cx).is_focused(window);
            div().child(div().id(id).w(px(WIDE)).max_h(px(most)).overflow_y_scroll().px(px(10.)).py(px(8.)).rounded(px(8.)).border_1().border_color(if focused { Theme::accent() } else { Theme::line_strong() }).bg(Theme::bg_body()).line_height(text_px(20.)).child(div().w(px(WIDE - 22.)).child(input.clone())))
        };
        let hint = match (fresh, form.paper_id.is_empty(), method) {
            (true, false, true) => Some(t("原文是在 PDF 上选中的文字，不需要的部分可以删去。起一个名称，以后按名称和标签查找，和同类的方法放在一起比较。")),
            (true, false, false) => Some(t("原文是在 PDF 上选中的文字，不需要的部分可以删去。名称写它怎样引入，以后按引入方式和标签查找，和别的导言放在一起比较。")),
            (true, true, true) => Some(t("自己记下一个方法：步骤和参数写在原文里，为什么这样做、改了什么写在备注里。")),
            (true, true, false) => Some(t("自己记下一段导言：开篇的那几句写在原文里，想借鉴的写在备注里。")),
            _ => None,
        };
        let body = div()
            .id("excerpt-fields")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(12.))
            .px(px(20.))
            .py(px(8.))
            .child(source)
            .when_some(hint, |d, hint| d.child(div().w(px(WIDE)).text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(hint)))
            .child(row(if method { t("名称") } else { t("引入方式") }, &form.name))
            .child(ui::form_row(t("原文"), tall("excerpt-text", &form.text, 220.)))
            .child(ui::form_row(t("标签"), div().w(px(WIDE)).child(ui::field(&form.tags, window, cx))).when(!offered.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(5.)).pt(px(3.)).w(px(WIDE)).children(offered))))
            .child(ui::form_row(t("备注"), tall("excerpt-notes", &form.notes, 130.)))
            .children(figures)
            .children(picker)
            .child(div().w(px(WIDE)).child(ui::error_line(&self.modal_error)));
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(20.))
            .py(px(14.))
            .when_some(form.id.clone(), |d, id| d.child(ui::ghost_button("excerpt-remove", Some("bin"), Some(t("移出").into())).text_color(Theme::danger()).on_click(self.on(Cmd::Excerpt(Do::AskRemove(id)), cx))))
            .child(div().flex_1())
            .child(ui::secondary_button("modal-cancel", if fresh { t("取消") } else { tc("dialog.close", "关闭") }).on_click(self.on(Cmd::CloseModal, cx)))
            .child(ui::primary_button("modal-ok", if fresh { tc("action.save_figure", "收藏") } else { t("保存") }).on_click(self.on(Cmd::SubmitModal, cx)));
        let title = ui::dialog_title(match (fresh, method) {
            (true, true) => t("收藏到方法库"),
            (true, false) => t("收藏到导言库"),
            (false, true) => t("方法"),
            (false, false) => t("导言"),
        });
        Some(ui::dialog(DIALOG_W).child(title).child(body).child(footer))
    }

    /// The methods linked to a figure, for its dialog: a quiet line of their names, each opening its method.
    pub(crate) fn render_figure_methods(&self, figure: &str, width: f32, cx: &mut Context<Self>) -> Option<Div> {
        let lib = self.lib.as_ref()?;
        let methods = lib.excerpts.methods_of(figure);
        if methods.is_empty() { return None }
        let names: Vec<AnyElement> = methods
            .iter()
            .map(|method| {
                div()
                    .id(SharedString::from(format!("figure-method-{}", method.id)))
                    .flex_none()
                    .max_w(px(width))
                    .truncate()
                    .text_color(Theme::accent())
                    .cursor_pointer()
                    .on_click(self.on(Cmd::Excerpt(Do::Open(method.id.clone())), cx))
                    .child(method.headline().to_string())
                    .into_any_element()
            })
            .collect();
        Some(ui::form_row(t("关联方法"), div().flex().flex_wrap().gap_x(px(12.)).gap_y(px(4.)).w(px(width)).text_size(text_px(12.)).children(names)))
    }

    // ---- the pages

    /// 方法库 or 导言库: the passages of a kind, as rows.
    pub fn render_excerpts(&mut self, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let excerpts = &lib.excerpts;
        let shelf = excerpts.shelf(kind);
        let method = kind == METHOD;
        let query = shelf.search.read(cx).text().to_string();
        let shown = Rc::new(excerpts.shown(kind, &lib.papers, &query));
        let (total, count) = (excerpts.count(kind), shown.len());

        let tags = excerpt::tag_counts(&excerpts.items, kind);
        let tag_chip = |id: ElementId, label: String, chosen: bool, cmd: Cmd, this: &Self, cx: &mut Context<Self>| {
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
                .on_click(this.on(cmd, cx))
                .child(label)
        };
        let mut chips = vec![tag_chip("excerpt-tag-all".into(), tf!("全部 {total}", "All {total}"), shelf.tag.is_empty(), Cmd::Excerpt(Do::Tag(kind.into(), shelf.tag.clone())), self, cx).into_any_element()];
        for (i, (tag, n)) in tags.iter().take(24).enumerate() {
            chips.push(tag_chip(("excerpt-tag", i).into(), format!("{tag} {n}"), *tag == shelf.tag, Cmd::Excerpt(Do::Tag(kind.into(), tag.clone())), self, cx).into_any_element());
        }

        let body = if total == 0 {
            if method {
                ui::empty_state("flask-conical", t("还没有收藏方法"), t("打开一篇文献的 PDF，选中它写实验方法的一段文字，点「收藏为方法」。原文和页码会一起记下，起个名称、加上标签和备注，以后可以按名称和标签找出同类的方法放在一起比较，也可以关联它做出来的图。也可以点右上角的「新建方法」自己记一个。")).into_any_element()
            } else {
                ui::empty_state("quote", t("还没有收藏导言"), t("打开一篇文献的 PDF，选中引言里它开篇、引入问题的几句，点「收藏为导言」。原文和页码会一起记下，用名称写下它怎样引入（例如 从临床问题切入），以后可以按引入方式和标签把别人的写法放在一起比较。也可以点右上角的「新建导言」自己记一段。")).into_any_element()
            }
        } else if count == 0 {
            if method { ui::empty_state("search", t("没有匹配的方法"), t("换一个搜索词，或者点「全部」看所有方法。")).into_any_element() } else { ui::empty_state("search", t("没有匹配的导言"), t("换一个搜索词，或者点「全部」看所有导言。")).into_any_element() }
        } else {
            let shown = shown.clone();
            uniform_list(if method { "methods" } else { "intros" }, count, cx.processor(move |this, range: Range<usize>, _, cx| this.render_excerpt_rows(range, &shown, cx))).track_scroll(shelf.scroll.clone()).flex_1().into_any_element()
        };
        let papers = shown.iter().map(|&at| excerpts.items[at].paper_id.as_str()).filter(|id| !id.is_empty()).collect::<std::collections::HashSet<_>>().len();
        let own = shown.iter().filter(|&&at| excerpts.items[at].paper_id.is_empty()).count();
        let focused = shelf.search.focus_handle(cx).is_focused(window);
        let status = if total == 0 {
            if method { t("方法库是空的").to_string() } else { t("导言库是空的").to_string() }
        } else if count != total {
            if method { tf!("显示 {count} / {total} 个", "Showing {count} / {total} methods") } else { tf!("显示 {count} / {total} 段", "Showing {count} / {total} introductions") }
        } else if own == 0 {
            if method { tf!("共 {total} 个，来自 {papers} 篇文献", "{total} methods from {papers} papers") } else { tf!("共 {total} 段，来自 {papers} 篇文献", "{total} introductions from {papers} papers") }
        } else if papers == 0 {
            if method { tf!("共 {total} 个，都是自己记下的", "{total} methods, all written down by me") } else { tf!("共 {total} 段，都是自己记下的", "{total} introductions, all written down by me") }
        } else if method {
            tf!("共 {total} 个：{} 个来自 {papers} 篇文献，{own} 个是自己记下的", "{total} methods: {} from {papers} papers, {own} written down by me", total - own)
        } else {
            tf!("共 {total} 段：{} 段来自 {papers} 篇文献，{own} 段是自己记下的", "{total} introductions: {} from {papers} papers, {own} written down by me", total - own)
        };
        let search = shelf.search.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                ui::column_head()
                    .gap(px(8.))
                    .child(self.render_collection_switch(cx))
                    .child(div().flex().flex_1().items_center().gap(px(8.)).h(text_px(34.)).px(px(10.)).rounded(px(8.)).bg(Theme::bg_sunken()).border_1().border_color(if focused { Theme::accent() } else { Theme::bg_sunken() }).child(ui::icon("search", 15., Theme::text_tertiary())).child(search))
                    .child(ui::ghost_button("excerpt-new", Some("plus"), Some(if method { t("新建方法") } else { t("新建导言") }.into())).flex_none().h(text_px(34.)).on_click(self.on(Cmd::Excerpt(Do::New(kind.into())), cx))),
            )
            .when(total > 0, |d| d.child(div().id("excerpt-tags").flex().flex_wrap().items_center().gap(px(6.)).px(px(16.)).pt(px(10.)).pb(px(6.)).children(chips)))
            .child(div().flex().flex_col().flex_1().min_h_0().child(body))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(text_px(30.))
                    .px(px(16.))
                    .border_t_1()
                    .border_color(Theme::line())
                    .text_size(text_px(12.))
                    .text_color(Theme::text_tertiary())
                    .gap(px(12.))
                    .child(div().flex_none().child(status))
                    .child(div().min_w_0().truncate().child(if method { t("点开方法查看和修改 · 右键更多操作 · 在 PDF 里选中一段文字点「收藏为方法」收藏新的") } else { t("点开导言查看和修改 · 右键更多操作 · 在 PDF 里选中一段文字点「收藏为导言」收藏新的") })),
            )
            .into_any_element()
    }

    fn render_excerpt_rows(&mut self, range: Range<usize>, shown: &[usize], cx: &mut Context<Self>) -> Vec<AnyElement> {
        range.filter_map(|at| shown.get(at).copied()).map(|at| self.render_excerpt_row(at, cx)).collect()
    }

    fn render_excerpt_row(&mut self, at: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.items.get(at)).cloned() else { return div().into_any_element() };
        let method = excerpt.kind == METHOD;
        let linked: Vec<String> = if method { self.lib.as_ref().map(|lib| lib.excerpts.figures_of(&excerpt.id).into_iter().filter(|id| lib.gallery.figure(id).is_some()).collect()).unwrap_or_default() } else { vec![] };
        let thumbs: Vec<(String, Thumb)> = linked.iter().take(ROW_FIGURES).map(|id| (id.clone(), self.figure_thumb(id, cx))).collect();
        let lib = self.lib.as_ref().unwrap();
        let mut source = if excerpt.paper_id.is_empty() { t("我记下的").to_string() } else { source_line(lib.paper(&excerpt.paper_id)) };
        if excerpt.page > 0 { source = tf!("第 {} 页 · {source}", "Page {} · {source}", excerpt.page) }
        let said = row_text(&excerpt);
        let name = if !excerpt.name.is_empty() { excerpt.name.clone() } else if method { t("未命名的方法").to_string() } else { t("未命名的导言").to_string() };
        let tags: Vec<Div> = excerpt.tags.iter().take(5).map(|tag| ui::chip(tag.clone(), Theme::text_secondary(), Theme::bg_sunken()).h(text_px(20.)).text_size(text_px(11.))).collect();
        let pictures: Vec<AnyElement> = thumbs
            .into_iter()
            .map(|(id, thumb)| {
                let picture = match thumb {
                    Thumb::Ready(image) => {
                        let natural = image.size(0);
                        let (w, h) = fit((natural.width.0 as f32, natural.height.0 as f32), (ROW_THUMB.0 - 6., ROW_THUMB.1 - 6.));
                        img(image).w(px(w)).h(px(h)).into_any_element()
                    }
                    Thumb::Loading => div().into_any_element(),
                    Thumb::Broken => ui::icon("image-broken", 16., Theme::text_placeholder()).into_any_element(),
                };
                div().id(SharedString::from(format!("excerpt-row-figure-{}-{id}", excerpt.id))).flex().flex_none().items_center().justify_center().w(px(ROW_THUMB.0)).h(px(ROW_THUMB.1)).rounded(px(5.)).border_1().border_color(Theme::line()).bg(gpui::white()).overflow_hidden().child(picture).into_any_element()
            })
            .collect();
        let more = linked.len().saturating_sub(ROW_FIGURES);
        div()
            .id(SharedString::from(format!("excerpt-{}", excerpt.id)))
            .flex()
            .items_center()
            .gap(px(14.))
            .w_full()
            .h(text_px(ROW_H))
            .px(px(16.))
            .border_b_1()
            .border_color(Theme::line())
            .cursor_pointer()
            .hover(|s| s.bg(Theme::hover()))
            .on_click(self.on(Cmd::Excerpt(Do::Open(excerpt.id.clone())), cx))
            .on_mouse_down(MouseButton::Right, self.on_context(MenuKind::Excerpt(excerpt.id.clone()), cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).text_color(if excerpt.name.is_empty() { Theme::text_tertiary() } else { Theme::text() }).child(name))
                    .child(div().truncate().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(source))
                    .child(div().h(text_px(36.)).overflow_hidden().text_size(text_px(12.)).line_height(text_px(18.)).line_clamp(2).text_color(if said.is_empty() { Theme::text_placeholder() } else { Theme::text_secondary() }).child(if said.is_empty() { t("还没有原文").to_string() } else { said }))
                    .child(div().flex().flex_none().gap(px(4.)).h(text_px(20.)).overflow_hidden().children(tags)),
            )
            .when(!pictures.is_empty(), |d| {
                d.child(div().flex().flex_none().items_center().gap(px(6.)).children(pictures).when(more > 0, |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(format!("+{more}")))))
            })
            .into_any_element()
    }

    /// What a right click on a row offers.
    pub(crate) fn excerpt_menu(&self, id: &str) -> (Vec<MenuItem>, Vec<usize>) {
        let item = |label: &str, cmd: Cmd| MenuItem { label: label.to_string().into(), checked: false, note: None, cmd };
        let Some(excerpt) = self.lib.as_ref().and_then(|lib| lib.excerpts.excerpt(id)) else { return (vec![], vec![]) };
        let id = id.to_string();
        let mut items = vec![item(t("查看与修改…"), Cmd::Excerpt(Do::Open(id.clone())))];
        if !excerpt.paper_id.is_empty() { items.push(item(&if excerpt.page > 0 { tf!("打开原文第 {} 页", "Open source at page {}", excerpt.page) } else { t("打开所属文献").to_string() }, Cmd::Excerpt(Do::Source(id.clone())))) }
        let mut breaks = vec![items.len()];
        items.push(item(t("用这段提问"), Cmd::Excerpt(Do::Ask(id.clone()))));
        if !excerpt.text.is_empty() { items.push(item(t("复制原文"), Cmd::CopyText(excerpt.text.clone()))) }
        breaks.push(items.len());
        items.push(item(if excerpt.kind == METHOD { t("移出方法库…") } else { t("移出导言库…") }, Cmd::Excerpt(Do::AskRemove(id))));
        (items, breaks)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{pick_order, row_text};
    use std::collections::HashMap;
    use wisp_core::excerpt::Excerpt;
    use wisp_core::figure::{Figure, search_words};
    use wisp_core::model::Paper;

    #[test]
    fn the_papers_own_figures_are_offered_first() {
        let figure = |id: &str, paper: &str, label: &str| Figure { id: id.into(), paper_id: paper.into(), label: label.into(), ..Default::default() };
        let figures = vec![figure("G-1", "P-2", "Fig. 1"), figure("G-2", "P-1", "Fig. 2 blot"), figure("G-3", "", "drawn"), figure("G-4", "P-1", "Fig. 4")];
        let papers = HashMap::new();
        assert_eq!(pick_order(&figures, "P-1", &[], &papers, &[]), vec![1, 3, 0, 2]);
        // Those linked already are not offered again; a search narrows them all.
        assert_eq!(pick_order(&figures, "P-1", &["G-4".to_string()], &papers, &[]), vec![1, 0, 2]);
        assert_eq!(pick_order(&figures, "P-1", &[], &papers, &search_words("blot")), vec![1]);
        // A method of no paper: the library's order.
        assert_eq!(pick_order(&figures, "", &[], &papers, &[]), vec![0, 1, 2, 3]);
        let paper = Paper { id: "P-2".into(), title: "Cytokinin".into(), ..Default::default() };
        let papers: HashMap<&str, &Paper> = [("P-2", &paper)].into_iter().collect();
        assert_eq!(pick_order(&figures, "P-1", &[], &papers, &search_words("cytokinin")), vec![0]);
    }

    #[test]
    fn a_rows_words_are_on_one_line() {
        let method = Excerpt { text: "Proteins were\nseparated   by SDS-PAGE.".into(), ..Default::default() };
        assert_eq!(row_text(&method), "Proteins were separated by SDS-PAGE.");
    }
}
