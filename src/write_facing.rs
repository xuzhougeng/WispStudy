//! 译文对照 in 写作: what is on show with each paragraph's translation beside
//! it, so someone writing in English sees what it says in Chinese, and the
//! other way round. It is the languages icon right of 预览's eye in the
//! manuscript's bar (`Cmd::WriteFacing`, `Writing::facing`): on, the page is
//! two halves, the text at the left as the preview draws it and its
//! translation at the right, block by block at the same height (左右对照);
//! or, as the person has it (逐段交替, `Settings::write_facing_interleaved`),
//! and in a column too narrow for two halves, each translation under its
//! paragraph. Turned on while a section is written, it is that section in its
//! place, the caret's paragraph in view (`Writing::facing_section`, `Reveal`);
//! from 预览, the whole manuscript.
//!
//! What is translated and how it is asked is `wisp_core::bilingual`: a block of
//! a section at a time, its citations as placeholders. Each block is kept in
//! the library's `translations` under its words and the instructions, as every
//! translation is (`translate.rs`), so a manuscript read again asks for
//! nothing, and after a change only the paragraphs changed are asked for.
//! Nothing here writes the manuscript: it is a way of looking at it.

use crate::app::{Cmd, MenuItem, MenuKind, Workspace};
use crate::i18n::{label, t};
use crate::manuscripts::READ_SIZE;
use crate::theme::{self, Theme, text_px};
use crate::translate::Translated;
use crate::ui;
use crate::write_seek::Holder;
use gpui::{prelude::FluentBuilder, *};
use std::cell::Cell;
use std::rc::Rc;
use wisp_core::bilingual::{self, Kind};
use wisp_core::glossary;
use wisp_core::manuscript::{self, LANGUAGES};
use wisp_core::settings::Task;
use wisp_core::store::translation_key;
use wisp_core::structure::SECTIONS;

/// Room between the two halves; the line between them runs down its middle.
pub(crate) const GUTTER: f32 = 48.;
/// The narrowest a half is read at: in less, a translation stands under its paragraph instead of beside it.
const HALF_MIN: f32 = 300.;
/// Room at each side of the page, as the preview has it.
pub(crate) const PAD: f32 = 28.;

/// Whether a page `width` wide has room for two halves of `HALF_MIN` side by side.
pub(crate) fn halves_fit(width: f32) -> bool {
    let room = (width - 2. * PAD).max(1.).min(2. * f32::from(theme::manuscript_width()) + GUTTER);
    room >= 2. * HALF_MIN + GUTTER
}

/// How the page of a manuscript and its translation is laid out in `width` (翻译's too): whether the halves stand
/// side by side (only where `beside` wants it and there is room), how wide a half is, and the whole page. Not side
/// by side, the page is one column at the reading width, each translation under its paragraph.
pub(crate) fn measures(width: f32, beside: bool) -> (bool, f32, f32) {
    // The widths are given in pixels: wrapped text under widths that are shares is measured at the wrong one.
    let (measure, room) = (f32::from(theme::manuscript_width()), (width - 2. * PAD).max(1.));
    let both = room.min(2. * measure + GUTTER);
    let side = beside && halves_fit(width);
    let half = if side { (both - GUTTER) / 2. } else { room.min(measure) };
    (side, half, if side { both } else { half })
}

/// Frames a paragraph to keep in view may take to be drawn before it is given up.
const REVEAL_FRAMES: u8 = 4;
/// Room over the paragraph kept in view.
const REVEAL_ABOVE: f32 = 80.;

/// The paragraph of the section read beside its translation to keep in view: the one the caret was in when it was
/// turned on. Translations coming in above it make the rows taller, so it is kept in view as they come, until the
/// reader scrolls.
pub struct Reveal {
    section: String,
    block: usize,
    /// Its top in the page, as last drawn.
    top: Rc<Cell<Option<f32>>>,
    /// The offset last given the page: a scroll that is not it is the reader's.
    given: Option<f32>,
    frames: u8,
}

/// How the translation of one block stands.
enum Beside {
    /// The block says nothing to translate.
    Nothing,
    /// Not asked for yet: others are in flight.
    Waiting,
    Busy,
    Done(String),
    Failed(String),
}

/// How far the translation of the manuscript on show has come.
#[derive(Default)]
struct Tally {
    asked: usize,
    done: usize,
    failed: usize,
    /// Why the first of those failed.
    error: String,
}

/// What stands in a translation's place while there is none: a spinner while it is asked for, a mark where it
/// failed. Why it failed is said once, in the head of the translation's half, where 重试 is: the same sentence
/// under every paragraph would be a page of it. The mark says which paragraphs, and its tooltip why.
fn standing(id: String, beside: &Beside) -> Option<AnyElement> {
    match beside {
        Beside::Busy => Some(div().flex().items_center().h(theme::body_line(24.)).child(ui::spinner(SharedString::from(id), 13., Theme::text_tertiary())).into_any_element()),
        Beside::Failed(error) => Some(div().flex().child(div().id(SharedString::from(id)).text_size(text_px(12.)).line_height(theme::body_line(24.)).text_color(Theme::danger()).tooltip(ui::tip(ui::one_line(error))).child(t("没有译成"))).into_any_element()),
        _ => None,
    }
}

/// A block and its translation: side by side, each `half` wide, or the translation under it in a column that
/// wide. `foot` sets the two by their lower edges: a picture's caption is under it.
fn pair(side: bool, half: f32, foot: bool, left: AnyElement, right: Option<AnyElement>) -> AnyElement {
    if side {
        div().flex().flex_none().gap(px(GUTTER)).when(foot, |d| d.items_end()).when(!foot, |d| d.items_start()).child(div().flex_none().w(px(half)).child(left)).child(div().flex_none().w(px(half)).children(right)).into_any_element()
    } else {
        div().flex().flex_col().flex_none().gap(px(6.)).w(px(half)).child(left).children(right.map(|right| div().text_color(Theme::text_secondary()).child(right))).into_any_element()
    }
}

/// A block's translation with a quiet mark in the margin at its right where it lacks what its source has
/// (`glossary::check`), the reasons in the mark's tooltip.
fn checked(id: String, translation: AnyElement, lacks: &[glossary::Lack]) -> AnyElement {
    if lacks.is_empty() { return translation }
    let mark = div().id(SharedString::from(id)).absolute().top(px(5.)).right(px(-20.)).child(ui::icon("triangle-alert", 12., Theme::warning())).tooltip(ui::tip(crate::write_assist::lacks_said(lacks)));
    div().relative().child(translation).child(mark).into_any_element()
}

/// A block's Markdown drawn as the preview draws it.
fn page(id: String, text: &str) -> AnyElement {
    crate::markdown::render_page(&id, &manuscript::section_markdown(text, 1), READ_SIZE).into_any_element()
}

impl Workspace {
    pub(crate) fn run_write_facing(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        match cmd {
            Cmd::WriteFacing(on) => {
                // The pieces of the page change: what stands beside the words read goes, a comment written there kept first.
                self.read_letgo(cx);
                let Some(lib) = self.lib.as_mut() else { return };
                let writing = &mut lib.writing;
                if on {
                    // Asked for while a section is written, it is that section, the caret's paragraph in view; from 预览, the whole manuscript.
                    let entering = !writing.reading;
                    writing.face();
                    if entering {
                        let caret = writing.editor.read(cx).caret();
                        let block = writing.section().map_or(0, |s| bilingual::blocks(&s.body).iter().position(|b| caret <= b.range.end).unwrap_or(0));
                        let section = writing.section_id.clone();
                        self.write_facing_keep(&section, block);
                        self.write_snapshot("leave");
                    }
                } else {
                    // Put away over a section, the section is written again; over the whole manuscript, the preview stays.
                    if writing.facing_section() { writing.reading = false }
                    writing.facing = false;
                    writing.facing_reveal = None;
                }
            }
            Cmd::WriteFacingLanguage(code) => {
                self.settings.write_facing_language = code;
                self.save_settings(cx);
            }
            Cmd::WriteFacingInterleave(on) => {
                self.settings.write_facing_interleaved = on;
                self.save_settings(cx);
                // Chosen while 翻译's version is read alone, the primary comes back to stand where it was chosen to.
                if let Some(lib) = self.lib.as_mut() { lib.writing.translate.alone = false }
            }
            Cmd::WriteFacingRetry => {
                let failed: Vec<String> = self.facing_wanted().into_iter().map(|(_, _, key)| key).collect();
                if let Some(lib) = self.lib.as_mut() { lib.translations.retain(|key, state| !(matches!(state, Translated::Failed(_)) && failed.contains(key))) }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The page beside its translation drawn from its top, the block `block` of `section` brought into view.
    pub(crate) fn write_facing_keep(&mut self, section: &str, block: usize) {
        let Some(lib) = self.lib.as_mut() else { return };
        lib.writing.facing_reveal = (block > 0).then(|| Reveal { section: section.to_string(), block, top: Rc::default(), given: None, frames: REVEAL_FRAMES });
        lib.citing.read_scroll.set_offset(point(px(0.), px(0.)));
    }

    /// Before the section beside its translation is drawn: the paragraph to keep in view brought there, again while
    /// the rows above it grow, until the reader scrolls or it is not drawn within a few frames.
    pub(crate) fn write_facing_reveal(&mut self, window: &mut Window) {
        let Some(lib) = self.lib.as_mut() else { return };
        if !lib.writing.facing_section() { lib.writing.facing_reveal = None }
        let Some(reveal) = lib.writing.facing_reveal.as_mut() else { return };
        let scroll = &lib.citing.read_scroll;
        let now = f32::from(scroll.offset().y);
        if reveal.given.is_some_and(|given| (given - now).abs() > 1.) {
            lib.writing.facing_reveal = None;
            return;
        }
        let Some(top) = reveal.top.get() else {
            match reveal.frames.checked_sub(1) {
                Some(left) => {
                    reveal.frames = left;
                    window.request_animation_frame();
                }
                None => lib.writing.facing_reveal = None,
            }
            return;
        };
        let wanted = -(top - REVEAL_ABOVE).max(0.);
        if reveal.given != Some(wanted) {
            scroll.set_offset(point(px(0.), px(wanted)));
            reveal.given = Some(wanted);
            window.request_animation_frame();
        }
    }

    /// A row of the section beside its translation, with what records its top while it is the paragraph to keep in view.
    pub(crate) fn facing_revealed(&self, section: &str, block: usize, row: AnyElement) -> AnyElement {
        let Some(lib) = self.lib.as_ref() else { return row };
        let Some(reveal) = lib.writing.facing_reveal.as_ref().filter(|reveal| reveal.section == section && reveal.block == block) else { return row };
        let (top, scroll) = (reveal.top.clone(), lib.citing.read_scroll.clone());
        let mark = canvas(|_, _, _| (), move |bounds, _, window, _| {
            let (view, offset) = (scroll.bounds(), scroll.offset());
            let at = f32::from(bounds.origin.y - view.origin.y - offset.y);
            // Moved since it was last drawn (a translation above it came): brought into view again in the next frame.
            if top.get().is_none_or(|was| (was - at).abs() > 0.5) {
                top.set(Some(at));
                window.request_animation_frame();
            }
        });
        div().relative().child(row).child(mark.absolute().top_0().left_0().size_full()).into_any_element()
    }

    /// Whether the manuscript on show is read beside its translation.
    pub(crate) fn write_facing_shown(&self) -> bool {
        self.lib.as_ref().is_some_and(|lib| lib.writing.reading && lib.writing.facing && lib.writing.current().is_some())
    }

    /// The language the translation is in, a code of `manuscript::LANGUAGES`: the one chosen in the head of the
    /// translation's half, else the interface's, and never the one the manuscript is written in.
    pub(crate) fn write_facing_language(&self) -> &'static str {
        let written = self.lib.as_ref().map_or(manuscript::DEFAULT_LANGUAGE, |lib| lib.writing.language());
        bilingual::language(&self.settings.write_facing_language, written, if crate::i18n::is_english() { "en" } else { "zh" })
    }

    /// The languages to read the manuscript beside: every one but its own.
    pub(crate) fn facing_language_menu(&self) -> Vec<MenuItem> {
        let written = self.lib.as_ref().map_or(manuscript::DEFAULT_LANGUAGE, |lib| lib.writing.language());
        let on = self.write_facing_language();
        LANGUAGES.iter().filter(|(code, _)| *code != written).map(|(code, name)| MenuItem { label: format!("{} ({code})", t(name)).into(), checked: *code == on, note: None, cmd: Cmd::WriteFacingLanguage(code.to_string()) }).collect()
    }

    /// Whether the manuscript on show is read beside its translation in two halves (左右对照), as the sheet under the
    /// manuscript's bar is made for: not while each translation is under its paragraph, nor 翻译's version alone.
    pub(crate) fn write_facing_halves(&self) -> bool {
        self.write_facing_shown() && !self.settings.write_facing_interleaved && !(self.translate_shown().is_some() && self.lib.as_ref().is_some_and(|lib| lib.writing.translate.alone))
    }

    /// Where the translation stands (design rule 21): beside the switch of 译稿 in the manuscript's bar while it is
    /// on, two icons that picture the two ways, the one in force lit, a click on the other changes it (a habit, kept
    /// from manuscript to manuscript). In a column too narrow for two halves the translation is under its paragraph
    /// whichever is chosen: 逐段交替 is lit and 左右对照, faded, says why. While 翻译's version is read alone, neither is
    /// in force, and either brings the manuscript back beside it.
    pub(crate) fn render_facing_arrangement(&self, cx: &mut Context<Self>) -> Div {
        let Some(lib) = self.lib.as_ref() else { return div() };
        let room = lib.writing.facing_room.get();
        let alone = self.translate_shown().is_some() && lib.writing.translate.alone;
        let under = self.settings.write_facing_interleaved || !room;
        let way = |id: &'static str, icon: &'static str, on: bool, faded: bool, tip: SharedString, interleaved: bool, cx: &mut Context<Self>| {
            let color = if on { Theme::accent() } else if faded { Theme::text_tertiary() } else { Theme::text_secondary() };
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(text_px(24.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(on, |d| d.bg(Theme::accent_soft()))
                .when(!on, |d| d.hover(|s| s.bg(Theme::hover())))
                .on_click(self.on(Cmd::WriteFacingInterleave(interleaved), cx))
                .tooltip(ui::tip(tip))
                .child(ui::icon(icon, 14., color))
        };
        let beside: SharedString = if room { t("左右对照：原文在左，译文在右，一段对一段").into() } else { t("左右对照：这里放不下两栏，窗口宽一些才能左右对照").into() };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(1.))
            .p(px(1.))
            .rounded(px(6.))
            .border_1()
            .border_color(Theme::line())
            .child(way("write-facing-beside", "columns-2", !alone && !under, !room, beside, false, cx))
            .child(way("write-facing-under", "rows-2", !alone && under, false, t("逐段交替：每段原文下面紧跟着它的译文").into(), true, cx))
    }

    /// The glossary's pairs from the manuscript's language into the translation's, and the manuscript's requirement:
    /// what a text's instructions are made with (`bilingual::prompt_for`).
    fn facing_rules(&self) -> (Vec<(String, String)>, String) {
        let Some(lib) = self.lib.as_ref() else { return (vec![], String::new()) };
        (glossary::pairs(&lib.glossary.terms, lib.writing.language(), self.write_facing_language()), lib.requirement())
    }

    /// What of the manuscript on show is translated, each text with its instructions and the key its translation is
    /// kept under: the title, each section's name, and each block that says something (`bilingual::blocks`), in the
    /// order read; over a section, only what it holds. A text's instructions carry the glossary's terms it holds
    /// (`bilingual::prompt_for`).
    fn facing_wanted(&self) -> Vec<(String, String, String)> {
        let Some(writing) = self.lib.as_ref().map(|lib| &lib.writing) else { return vec![] };
        let Some(manuscript) = writing.current() else { return vec![] };
        let code = self.write_facing_language();
        let (pairs, requirement) = self.facing_rules();
        let mut wanted: Vec<String> = vec![];
        if writing.facing_holds("") && !manuscript.title.trim().is_empty() { wanted.push(manuscript.title.trim().to_string()) }
        for section in &manuscript.sections[writing.facing_sections()] {
            if !section.name.trim().is_empty() { wanted.push(section.name.trim().to_string()) }
            wanted.extend(bilingual::blocks(&section.body).into_iter().map(|block| block.sent).filter(|sent| !sent.is_empty()));
        }
        wanted
            .into_iter()
            .map(|text| {
                let prompt = bilingual::prompt_for(code, &text, &pairs, &requirement);
                let key = translation_key(&text, &prompt);
                (text, prompt, key)
            })
            .collect()
    }

    /// Ask for the translations still missing, as many at a time as a batch of translations has in flight
    /// (设置 › AI › 任务). Called before the manuscript is drawn beside its translation: an answer draws a frame,
    /// and that frame asks for the next ones. What failed is asked again only by 重试.
    pub(crate) fn write_facing_tend(&mut self, cx: &mut Context<Self>) {
        let wanted = self.facing_wanted();
        let profile = self.settings.profile_for_or_default(Task::Translate);
        let workers = self.settings.translate_workers.max(1);
        let state = |this: &Self, key: &str| this.lib.as_ref().and_then(|lib| lib.translations.get(key)).map(|state| matches!(state, Translated::Busy));
        let mut busy = wanted.iter().filter(|(_, _, key)| state(self, key) == Some(true)).count();
        for (text, prompt, key) in &wanted {
            if busy >= workers { break }
            if state(self, key).is_some() { continue }
            // Kept by an earlier look at this manuscript: read, not asked for.
            self.translate_text(text, &profile, prompt, false, false, cx);
            if state(self, key) == Some(true) { busy += 1 }
        }
    }

    /// The whole manuscript read through, or the section on show, each block with its translation: beside it where
    /// the column has room for two halves, under it where not or where the person has it so. `width` is the room the
    /// column has for it.
    pub(crate) fn render_manuscript_facing(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let Some(manuscript) = lib.writing.current() else { return div().into_any_element() };
        // Over a section: it and its subsections, without the title and the reference list, which are the whole manuscript's.
        let (within, whole_shown) = (lib.writing.facing_sections(), !lib.writing.facing_section());
        let code = self.write_facing_language();
        let (pairs, requirement) = self.facing_rules();
        let (side, half, whole) = measures(width, !self.settings.write_facing_interleaved);
        let mut tally = Tally::default();
        let mut look = |sent: &str| -> Beside {
            if sent.trim().is_empty() { return Beside::Nothing }
            tally.asked += 1;
            match lib.translations.get(&translation_key(sent, &bilingual::prompt_for(code, sent, &pairs, &requirement))) {
                None => Beside::Waiting,
                Some(Translated::Busy) => Beside::Busy,
                Some(Translated::Done(text)) => {
                    tally.done += 1;
                    Beside::Done(text.clone())
                }
                Some(Translated::Failed(error)) => {
                    tally.failed += 1;
                    if tally.error.is_empty() { tally.error = error.clone() }
                    Beside::Failed(error.clone())
                }
            }
        };
        // A name: the manuscript's title, a section's.
        let named = |size: f32, line: f32, text: String| div().text_size(text_px(size)).line_height(text_px(line)).font_weight(FontWeight::SEMIBOLD).child(text);

        let title = whole_shown.then(|| {
            let beside = look(&manuscript.title);
            let right = match &beside {
                Beside::Done(text) => Some(named(24., 32., ui::one_line(text)).into_any_element()),
                other => standing("write-face-title".into(), other),
            };
            pair(side, half, false, named(24., 32., crate::i18n::manuscript_title(manuscript).to_string()).into_any_element(), right)
        });
        let mut sections: Vec<AnyElement> = vec![];
        for (n, section) in manuscript.sections.iter().enumerate().filter(|(n, _)| within.contains(n)) {
            let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
            let read = |within: std::ops::Range<usize>| wisp_core::cite::read_as(&section.body, labels, within);
            // The section's name leads back to writing it, as in the preview.
            let heading = div()
                .id(("write-face-heading", n))
                .flex()
                .items_baseline()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(self.on(Cmd::WriteSection(section.id.clone()), cx))
                .child(named(20., 28., section.name.clone()).text_color(Theme::text()).hover(|s| s.text_color(Theme::accent())))
                .when(section.kind != "other", |d| d.child(div().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(label(&SECTIONS, &section.kind).to_string())));
            let beside = look(&section.name);
            let translated = match &beside {
                Beside::Done(text) => Some(named(20., 28., ui::one_line(text)).into_any_element()),
                other => standing(format!("write-face-name-{n}"), other),
            };
            let mut rows = vec![pair(side, half, false, heading.into_any_element(), translated)];
            let blocks = bilingual::blocks(&section.body);
            if blocks.is_empty() { rows.push(pair(side, half, false, div().text_size(text_px(13.)).text_color(Theme::text_tertiary()).child(t("这一节还没有写。")).into_any_element(), None)) }
            let pictures = manuscript::picture_lines(&section.body);
            for (b, block) in blocks.iter().enumerate() {
                let id = format!("write-face-{n}-{b}");
                let beside = look(&block.sent);
                match block.kind {
                    Kind::Picture => {
                        // The picture over its caption at the left; at the right the caption alone, at the caption's height.
                        let Some(picture) = pictures.iter().find(|picture| picture.line == block.range) else { continue };
                        let caption = section.body[picture.line.clone()].find(picture.caption.as_str()).filter(|_| !picture.caption.is_empty()).map(|start| read(picture.line.start + start..picture.line.start + start + picture.caption.len()));
                        let shown = manuscript::PictureLine { caption: caption.unwrap_or_else(|| picture.caption.clone()), ..picture.clone() };
                        let right = match &beside {
                            Beside::Done(text) => Some(div().w_full().text_size(text_px(13.)).text_color(Theme::text_secondary()).child(crate::markdown::render(&format!("{id}-t"), &bilingual::restored(text, &section.body, block, labels), None, None)).into_any_element()),
                            other => standing(format!("{id}-w"), other),
                        };
                        let drawn = self.seek_hold(Holder::Words(section.id.clone(), picture.line.clone()), self.render_manuscript_picture(format!("{id}-p"), &shown, half, window));
                        rows.push(self.facing_revealed(&section.id, b, pair(side, half, true, drawn, right)));
                    }
                    Kind::Words | Kind::Kept => {
                        let right = match (block.kind, &beside) {
                            // Code and rules read the same in any language: beside themselves, and not said twice in one column.
                            (Kind::Kept, _) => side.then(|| page(format!("{id}-t"), &section.body[block.range.clone()])),
                            // Looked over with no model: a term of the glossary not used, a number lost, is a mark at its right.
                            (_, Beside::Done(text)) => Some(checked(format!("{id}-c"), page(format!("{id}-t"), &bilingual::restored(text, &section.body, block, labels)), &glossary::check(&block.sent, text, &glossary::occurring(&block.sent, &pairs)))),
                            (_, other) => standing(format!("{id}-w"), other),
                        };
                        // The original is the preview's: selected, copied and commented on as it is (write_read.rs).
                        let original = self.render_read_piece(section, n, b, block.range.clone(), &manuscript::section_markdown(&read(block.range.clone()), 1), true, window, cx);
                        rows.push(self.facing_revealed(&section.id, b, pair(side, half, false, original, right)));
                    }
                }
            }
            sections.push(div().flex().flex_col().flex_none().gap(theme::paragraph_gap(10.)).text_size(text_px(READ_SIZE)).children(rows).into_any_element());
        }
        let references = whole_shown.then(|| self.render_references_read(cx)).flatten().map(|list| pair(side, half, false, list, None));

        // The line between the halves, from the head down the page.
        let divider = || div().absolute().top_0().bottom_0().left(px(half + GUTTER / 2.)).w(px(1.)).bg(Theme::line());
        let small = |text: &str| div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(text.to_string());
        // What the translation's half says of itself, in its top left corner: what it is, in which language (a menu), and how far it has come.
        let status: Option<AnyElement> = if tally.done + tally.failed < tally.asked {
            Some(div().flex().flex_none().items_center().gap(px(5.)).child(ui::spinner("write-facing-busy", 12., Theme::text_tertiary())).child(small(&tf!("正在翻译 {} / {}", "Translating {} / {}", tally.done, tally.asked))).into_any_element())
        } else if tally.failed > 0 {
            let said = tn!("{n} 段没有译成：{}", "{n} not translated: {}", tally.failed, "paragraph", ui::one_line(&tally.error));
            Some(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap(px(4.))
                    .child(div().id("write-facing-failed").min_w_0().truncate().text_size(text_px(11.)).text_color(Theme::danger()).tooltip(ui::tip(said.clone())).child(said))
                    .child(ui::ghost_button("write-facing-retry", None, Some(t("重试").into())).flex_none().h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).on_click(self.on(Cmd::WriteFacingRetry, cx)))
                    .into_any_element(),
            )
        } else {
            None
        };
        let language = ui::ghost_button("write-facing-language", None, None).flex_none().h(text_px(22.)).px(px(6.)).gap(px(4.)).text_size(text_px(12.)).child(label(&LANGUAGES, code).to_string()).child(ui::icon("chevron-down", 11., Theme::text_tertiary())).on_click(self.on_menu(MenuKind::WriteFacingLanguage, cx));
        let failed = tally.failed > 0 && tally.done + tally.failed >= tally.asked;
        // 翻译 grows out of here (write_translate.rs): what is read beside the manuscript kept as a version of it, editable and following it.
        let name = label(&LANGUAGES, code);
        let keep = ui::ghost_button("write-facing-keep", None, Some(t("保留成译稿").into())).flex_none().h(text_px(22.)).px(px(6.)).text_size(text_px(12.)).tooltip(ui::tip(tf!("保留成{name}译稿：每段可以改，原文改了它跟着变", "Keep as a {name} version: every paragraph editable, following the manuscript as it changes"))).on_click(self.on(Cmd::Translate(crate::write_translate::Do::Language(code.to_string())), cx));
        let translation = div().flex().flex_none().items_center().gap(px(4.)).w(px(half)).child(small(t("译文"))).child(language).child(keep).when(!failed, |d| d.child(div().flex_1())).children(status);
        let head = div().flex().flex_none().justify_center().px(px(PAD)).child(
            div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(GUTTER))
                .w(px(whole))
                .h(text_px(30.))
                .when(side, |d| d.child(div().flex().flex_none().items_center().w(px(half)).child(small(t("原文")))).child(divider()))
                .child(translation),
        );
        let sheet = div()
            .id("write-facing-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&lib.citing.read_scroll)
            // No frame, like the preview: one line under it, where the text scrolls away.
            .border_b_1()
            .border_color(Theme::line())
            .child(div().flex().justify_center().px(px(PAD)).pb(px(24.)).child(div().relative().flex().flex_col().flex_none().gap(px(30.)).w(px(whole)).pt(px(12.)).text_color(Theme::text()).when(side, |d| d.child(divider())).children(title).children(sections).children(references)));
        div().flex().flex_col().flex_1().min_h_0().child(head).child(sheet).into_any_element()
    }
}
