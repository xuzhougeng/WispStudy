//! The manuscript read through (预览, `Writing::reading`, and beside its translation, 译文对照) is a page to work on
//! and not only to look at: its words are selected as an answer's are (a drag, a double click for a word), copied,
//! commented on where they are read, and gone back to in the editor. The words a comment is about are tinted as in
//! the editor, and a click on them opens the comment's card beside them, with what the card in the panel does.
//!
//! What stands beside a stretch (the tools, a comment being written, a comment's card) goes where the bar of an
//! answer goes, under the line the stretch ends on (design rule 8, `chat::beside_selection`); a right click on the
//! words offers the same actions by the same names. What the preview draws is tied to the section's text by
//! `wisp_core::preview::Reading`: a comment keeps the words as they are written, the editor selects them there.

use crate::app::{Cmd, MenuKind, Workspace};
use crate::chat::Spot;
use crate::i18n::t;
use crate::manuscripts::READ_SIZE;
use crate::markdown::{Place, Pointer, Selecting};
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::*;
use std::ops::Range;
use std::rc::Rc;
use wisp_core::bilingual;
use wisp_core::cite::read_as;
use wisp_core::comment;
use wisp_core::manuscript::{self, PictureLine, Section};
use wisp_core::preview::Reading;

/// What stands beside a stretch of the manuscript read through.
#[derive(Clone, Debug, PartialEq)]
pub enum Beside {
    /// What is selected, and the tools for it.
    Tools,
    /// A comment being written on what is selected (`Commenting::drafting`).
    Writing,
    /// The card of the comment whose words were clicked: its id. The stretch is its words, not selected.
    Comment(String),
}

/// A stretch of one piece of the manuscript read through, and what stands beside it. A piece is a section's text
/// between two of its pictures in the preview, one block of it beside its translation: what the drawing's places
/// count in (`markdown::texts`).
#[derive(Clone, Debug)]
pub struct Pick {
    pub section: String,
    pub piece: usize,
    pub facing: bool,
    /// Where the pointer went down, and where it is: either may come first.
    anchor: Place,
    head: Place,
    /// The pointer still holds it.
    pub held: bool,
    pub beside: Beside,
}

impl Pick {
    /// From, to, in order.
    pub fn range(&self) -> (Place, Place) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    fn is(&self, section: &str, piece: usize, facing: bool) -> bool {
        self.section == section && self.piece == piece && self.facing == facing
    }

    /// Words are selected, and the tools for them are what stands beside.
    pub fn selects(&self) -> bool {
        self.beside == Beside::Tools && self.anchor != self.head
    }
}

/// The manuscript read through, as far as the pointer has to do with it.
#[derive(Default)]
pub struct Read {
    pub pick: Option<Pick>,
    /// Where the stretch was drawn in the last frame, and how big what stands beside it came out.
    spot: Option<Spot>,
    size: Option<Size<Pixels>>,
    /// The comment's field beside it is to have the keyboard once it is drawn (`read_tend`): a field not in the frame loses it.
    focus: bool,
}

impl Read {
    /// Ends a selection or a comment's card the pointer is not holding: whether one ended. A comment being written
    /// is the window's to keep or let go (`Workspace::read_letgo`).
    pub fn unpick(&mut self) -> bool {
        if !self.pick.as_ref().is_some_and(|pick| !pick.held && pick.beside != Beside::Writing) { return false }
        self.set(None);
        true
    }

    /// Another stretch, or none: where the last one was drawn and how big its company was is no guide to it.
    fn set(&mut self, pick: Option<Pick>) {
        (self.pick, self.spot, self.size, self.focus) = (pick, None, None, false);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// The pointer over a piece's words: the section, the piece, whether beside the translation, and where.
    Pointer(String, usize, bool, Pointer, Place),
    /// What is selected: copied as it reads; a comment written on it; the section written with it selected.
    Copy,
    Comment,
    Edit,
    /// The comment being written beside it: kept, or let go.
    Keep,
    Drop,
}

/// A section read through, as the preview draws it: its text in the stretches between its pictures, each with the
/// Markdown drawn for it (citations read as their labels, a heading of the section's `depth` levels under the page's
/// first, `section_markdown`), and each picture with its caption read so.
pub(crate) enum Part {
    Words(Range<usize>, String),
    Picture(PictureLine),
}

pub(crate) fn parts(body: &str, labels: &[(Range<usize>, String)], depth: u8) -> Vec<Part> {
    let read = |within: Range<usize>| read_as(body, labels, within);
    let mut parts = vec![];
    let mut from = 0;
    for picture in manuscript::picture_lines(body) {
        let before = manuscript::section_markdown(&read(from..picture.line.start), depth);
        if !before.is_empty() { parts.push(Part::Words(from..picture.line.start, before)) }
        let caption = body[picture.line.clone()].find(picture.caption.as_str()).filter(|_| !picture.caption.is_empty()).map(|start| read(picture.line.start + start..picture.line.start + start + picture.caption.len()));
        from = picture.line.end;
        parts.push(Part::Picture(PictureLine { caption: caption.unwrap_or_else(|| picture.caption.clone()), ..picture }));
    }
    let rest = manuscript::section_markdown(&read(from..body.len()), depth);
    if !rest.is_empty() { parts.push(Part::Words(from..body.len(), rest)) }
    parts
}

impl Workspace {
    pub(crate) fn run_read(&mut self, what: Do, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Do::Pointer(section, piece, facing, pointer, place) => self.read_pointer(section, piece, facing, pointer, place, window, cx),
            Do::Copy => {
                let Some(text) = self.read_picked_text() else { return };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.say(t("已复制"), cx);
            }
            Do::Comment => self.read_comment(window, cx),
            Do::Edit => self.read_edit(window, cx),
            Do::Keep | Do::Drop => {
                let said = self.lib.as_ref().is_some_and(|lib| !lib.writing.comments.draft.read(cx).text().trim().is_empty());
                if let Some(lib) = self.lib.as_mut() { lib.writing.read.set(None) }
                if what == Do::Keep && said { self.comment_save(cx) } else { self.comment_cancel(cx) }
                window.focus(&self.focus);
            }
        }
        cx.notify();
    }

    fn read_pointer(&mut self, section: String, piece: usize, facing: bool, pointer: Pointer, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        match pointer {
            Pointer::Down | Pointer::Word => {
                // A comment being written on the page is kept, or let go when it says nothing, before anything else is pointed at.
                self.read_letgo(cx);
                // One selection in the window, and the keyboard with the window: copying finds this one, not a text box.
                window.focus(&self.focus);
                self.picked = None;
                let Some(lib) = self.lib.as_mut() else { return };
                for talk in [crate::app::Talk::Library, crate::app::Talk::Paper, crate::app::Talk::Write] { lib.chat_mut(talk).picked = None }
                let pick = Pick { section, piece, facing, anchor: place, head: place, held: true, beside: Beside::Tools };
                if pointer == Pointer::Down { return lib.writing.read.set(Some(pick)) }
                // A word by the rule of every text read, done at once and not held: the hand's slip between press and release would cut it short.
                let Some((_, _, shown)) = self.read_piece(&pick.section, piece, facing) else { return };
                let word = crate::markdown::texts(&shown).get(place.0).map(|text| crate::select::word_at(text, place.1)).filter(|word| !word.is_empty() && crate::markdown::text_between(&shown, (place.0, word.start), (place.0, word.end)).trim() != "");
                let Some(lib) = self.lib.as_mut() else { return };
                lib.writing.read.set(word.map(|word| Pick { anchor: (place.0, word.start), head: (place.0, word.end), held: false, ..pick }));
            }
            Pointer::Drag => {
                let Some(pick) = self.lib.as_mut().and_then(|lib| lib.writing.read.pick.as_mut()).filter(|pick| pick.held && pick.is(&section, piece, facing)) else { return };
                pick.head = place;
            }
            Pointer::Up => {
                let Some(lib) = self.lib.as_mut() else { return };
                let Some(pick) = lib.writing.read.pick.as_mut().filter(|pick| pick.held) else { return };
                pick.held = false;
                if pick.anchor != pick.head { return }
                // A click selects nothing; on the words of a comment it opens the comment's card beside them.
                let (section, piece, facing, at) = (pick.section.clone(), pick.piece, pick.facing, pick.anchor);
                let hit = self.read_piece(&section, piece, facing).and_then(|(n, within, shown)| {
                    let lib = self.lib.as_ref()?;
                    let body = lib.writing.current()?.sections.get(n)?.body.clone();
                    self.read_marks(&section, &body, n, within, &shown).into_iter().filter(|(from, to, ..)| *from <= at && at < *to).min_by_key(|(from, to, ..)| (to.0 - from.0, to.1.abs_diff(from.1)))
                });
                let Some((from, to, _, id)) = hit else {
                    if let Some(lib) = self.lib.as_mut() { lib.writing.read.set(None) }
                    return;
                };
                // The card's actions work on the section being written: this one becomes it, the preview stays.
                self.read_attend(&section, cx);
                let Some(lib) = self.lib.as_mut() else { return };
                lib.writing.comments.focused = Some(id.clone());
                lib.writing.read.set(Some(Pick { section, piece, facing, anchor: from, head: to, held: false, beside: Beside::Comment(id) }));
            }
        }
    }

    /// Ends what stands beside a stretch of the manuscript read through. A comment being written there was written
    /// on the page, so a press elsewhere keeps it when it says something and lets it go when it says nothing (design
    /// rule 33). Whether there was something to end.
    pub(crate) fn read_letgo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(lib) = self.lib.as_mut() else { return false };
        let Some(pick) = lib.writing.read.pick.take() else { return false };
        lib.writing.read.set(None);
        if pick.beside == Beside::Writing {
            if lib.writing.comments.draft.read(cx).text().trim().is_empty() { self.comment_cancel(cx) } else { self.comment_save(cx) }
        }
        true
    }

    /// A comment being written beside the preview's words: its field's Esc keeps it as a press elsewhere does.
    pub(crate) fn read_writing(&self) -> bool {
        self.lib.as_ref().is_some_and(|lib| lib.writing.read.pick.as_ref().is_some_and(|pick| pick.beside == Beside::Writing))
    }

    /// The comment's card beside its words goes with the comment, and with its being dealt with.
    pub(crate) fn read_forget(&mut self, comment: &str) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.writing.read.pick.as_ref().is_some_and(|pick| pick.beside == Beside::Comment(comment.to_string())) { lib.writing.read.set(None) }
    }

    /// The section a piece is of becomes the section being written, while the manuscript is still read through:
    /// comments are written on it, and its comments' cards change it, through the editor.
    fn read_attend(&mut self, section: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref().filter(|lib| lib.writing.section_id != section) else { return };
        let manuscript = lib.writing.manuscript.clone();
        // `write_show` ends a comment being written on the section left: none is, here.
        let pick = self.lib.as_mut().and_then(|lib| lib.writing.read.pick.take());
        self.write_show(manuscript, section.to_string(), cx);
        if let Some(lib) = self.lib.as_mut() { lib.writing.read.pick = pick }
    }

    /// A piece of the manuscript read through: its section's place, the stretch of the section's text, and the
    /// Markdown drawn for it.
    fn read_piece(&self, section: &str, piece: usize, facing: bool) -> Option<(usize, Range<usize>, String)> {
        let lib = self.lib.as_ref()?;
        let manuscript = lib.writing.current()?;
        let n = manuscript.sections.iter().position(|s| s.id == section)?;
        let body = &manuscript.sections[n].body;
        let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
        if facing {
            let block = bilingual::blocks(body).into_iter().nth(piece).filter(|block| block.kind != bilingual::Kind::Picture)?;
            return Some((n, block.range.clone(), manuscript::section_markdown(&read_as(body, labels, block.range), 1)));
        }
        // As the manuscript read through draws it (`render_section_read` with the page's first level at 1).
        let depth = manuscript.sections[n].level.saturating_sub(1) + 1;
        parts(body, labels, depth).into_iter().filter_map(|part| match part { Part::Words(within, shown) => Some((within, shown)), Part::Picture(_) => None }).nth(piece).map(|(within, shown)| (n, within, shown))
    }

    /// The words selected in the manuscript read through, as they read.
    pub(crate) fn read_picked_text(&self) -> Option<String> {
        let pick = self.lib.as_ref()?.writing.read.pick.as_ref().filter(|pick| !matches!(pick.beside, Beside::Comment(_)))?;
        let (_, _, shown) = self.read_piece(&pick.section, pick.piece, pick.facing)?;
        let (from, to) = pick.range();
        Some(crate::markdown::text_between(&shown, from, to)).filter(|text| !text.trim().is_empty())
    }

    /// Where the words selected are written: their section, and its bytes.
    fn read_written(&self) -> Option<(String, Range<usize>)> {
        let lib = self.lib.as_ref()?;
        let pick = lib.writing.read.pick.as_ref().filter(|pick| pick.anchor != pick.head)?;
        let (n, within, shown) = self.read_piece(&pick.section, pick.piece, pick.facing)?;
        let section = lib.writing.current()?.sections.get(n)?;
        let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
        let (from, to) = pick.range();
        Some((section.id.clone(), Reading::new(&section.body, labels, within, &shown).written(from, to)?))
    }

    /// 批注 on what is selected: the comment is written beside it, on the words as the section has them. Of a
    /// selection over several paragraphs it is about the first, as in the editor.
    fn read_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, range)) = self.read_written() else { return self.say(t("选中的文字在稿件里找不到原文，回到这一节选中它再写批注。"), cx) };
        self.read_attend(&section, cx);
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(body) = lib.writing.current().and_then(|m| m.section(&section)).map(|s| s.body.clone()) else { return };
        let quote = body[range].lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_string();
        let writing = &mut lib.writing;
        writing.comments.drafting = Some(quote);
        writing.comments.draft.update(cx, |input, cx| input.set_text("", cx));
        // The same stretch: the card goes where the tools were, drawn in the next frame, where its field takes the keyboard.
        let Some(pick) = writing.read.pick.as_mut() else { return };
        (pick.held, pick.beside) = (false, Beside::Writing);
        (writing.read.size, writing.read.focus) = (None, true);
        window.focus(&self.focus);
    }

    /// Before the manuscript read through is drawn: the field of a comment begun there gets the keyboard, once the card
    /// it is in will be drawn; and a field put away with Enter gives the keyboard back to the window.
    pub(crate) fn read_tend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.writing.reading) else { return };
        let writing = &mut lib.writing;
        if writing.read.focus && writing.read.spot.is_some() {
            writing.read.focus = false;
            writing.comments.draft.read(cx).focus(window);
        } else if writing.comments.drafting.is_none() && writing.comments.draft.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus);
        }
    }

    /// 编辑这里: back to writing the section, the words selected there, in view, with the tools beside them.
    fn read_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, range)) = self.read_written() else { return self.say(t("选中的文字在稿件里找不到原文。"), cx) };
        let Some(lib) = self.lib.as_mut() else { return };
        lib.writing.read.set(None);
        (lib.writing.reading, lib.writing.facing) = (false, false);
        if lib.writing.section_id != section {
            let manuscript = lib.writing.manuscript.clone();
            self.write_show(manuscript, section, cx);
        }
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        writing.editor.update(cx, |input, cx| input.select_range(range.clone(), cx));
        writing.editor.read(cx).focus(window);
        writing.seek.reveal_at(range.start);
    }

    /// The words of the open comments of a section drawn in one piece of it: from, to, their tint, and the comment.
    fn read_marks(&self, section: &str, body: &str, n: usize, within: Range<usize>, shown: &str) -> Vec<(Place, Place, Hsla, String)> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let located: Vec<(Range<usize>, &comment::Comment)> = lib.writing.comments.list.iter()
            .filter(|c| c.section_id == section && c.state != comment::DONE)
            .filter_map(|c| Some((comment::locate(body, &c.quote, None)?, c)))
            .filter(|(range, _)| within.start <= range.start && range.end <= within.end)
            .collect();
        if located.is_empty() { return vec![] }
        let labels = lib.citing.read.whole.get(n).map(Vec::as_slice).unwrap_or(&[]);
        let reading = Reading::new(body, labels, within, shown);
        located.into_iter().filter_map(|(range, c)| {
            let (from, to) = reading.drawn(range)?;
            Some((from, to, if c.proposes() { Theme::note_strong() } else { Theme::note() }, c.id.clone()))
        }).collect()
    }

    /// A piece of the manuscript read through: `shown` (the Markdown drawn for `within` of the `n`-th section)
    /// drawn so that it can be selected, the words of its comments tinted, and whatever stands beside a stretch of it.
    pub(crate) fn render_read_piece(&self, section: &Section, n: usize, piece: usize, within: Range<usize>, shown: &str, facing: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let lib = self.lib.as_ref().unwrap();
        let marks = self.read_marks(&section.id, &section.body, n, within, shown);
        let read = &lib.writing.read;
        let pick = read.pick.as_ref().filter(|pick| pick.is(&section.id, piece, facing));
        let selected = pick.filter(|pick| !matches!(pick.beside, Beside::Comment(_))).map(Pick::range).filter(|(from, to)| from != to);
        let card = pick.and_then(|pick| match &pick.beside { Beside::Comment(id) => marks.iter().find(|mark| mark.3 == *id).map(|mark| (mark.0, mark.1)), _ => None });
        let me = cx.entity().downgrade();
        let (told, said) = (me.clone(), section.id.clone());
        let scroll = lib.citing.read_scroll.clone();
        let selecting = Selecting {
            selected,
            marks: marks.iter().map(|(from, to, tint, _)| (*from, *to, *tint)).collect(),
            spot: card,
            pointer: Rc::new(move |pointer, place, window, cx| {
                told.update(cx, |this, cx| this.run(Cmd::Read(Do::Pointer(said.clone(), piece, facing, pointer, place)), window, cx)).ok();
            }),
            drawn: {
                let measured = me.clone();
                Rc::new(move |drawn, window, cx| {
                    // In terms scrolling leaves alone: from the piece, and the piece in the scrolled page.
                    let (view, offset) = (scroll.bounds(), scroll.offset());
                    let spot = Spot { start: drawn.start - drawn.origin, end: drawn.end - drawn.origin, line: drawn.line, answer: drawn.origin - view.origin - offset, view: view.size };
                    let measured = measured.clone();
                    window.defer(cx, move |_, cx| {
                        measured.update(cx, |this, cx| {
                            let Some(read) = this.lib.as_mut().map(|lib| &mut lib.writing.read) else { return };
                            if read.spot != Some(spot) {
                                read.spot = Some(spot);
                                cx.notify();
                            }
                        }).ok();
                    });
                })
            },
        };
        let key = if facing { format!("write-face-{n}-{piece}-o") } else { format!("write-read-{n}-{piece}") };
        let page = crate::markdown::render_page_selecting(&key, shown, READ_SIZE, Some(selecting)).on_mouse_down(MouseButton::Right, self.on_context(MenuKind::ReadPiece(section.id.clone(), piece, facing), cx));
        // What stands beside the stretch, once it is let go and drawn; not under a menu or a dialog.
        let covered = self.menu.is_some() || self.modal.is_some() || self.palette.is_some();
        let Some(pick) = pick.filter(|pick| !pick.held && !covered && (selected.is_some() || card.is_some())) else { return page.into_any_element() };
        let guess = match pick.beside { Beside::Tools => size(px(250.), px(32.)), Beside::Writing => size(px(330.), px(120.)), Beside::Comment(_) => size(px(340.), px(170.)) };
        let Some((at, spot)) = read.spot.and_then(|spot| Some((spot.bar_at(lib.citing.read_scroll.offset(), read.size.unwrap_or(guess))?, spot))) else { return page.into_any_element() };
        let Some(inner) = self.render_read_beside(pick, window, cx) else { return page.into_any_element() };
        let measured = me;
        let beside = div()
            .id("read-beside")
            .absolute()
            .left(at.x)
            .top(at.y)
            .max_w(spot.view.width - px(12.))
            .block_mouse_except_scroll()
            // A press on what stands beside is not a press elsewhere, which would end it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // How big it came out: where it goes next frame, near an edge, goes by it.
            .child(canvas(move |bounds, window, cx| {
                let size = bounds.size;
                window.defer(cx, move |_, cx| {
                    measured.update(cx, |this, cx| {
                        let Some(read) = this.lib.as_mut().map(|lib| &mut lib.writing.read) else { return };
                        if read.size != Some(size) {
                            read.size = Some(size);
                            cx.notify();
                        }
                    }).ok();
                });
            }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .child(inner);
        page.relative().child(deferred(beside).with_priority(1)).into_any_element()
    }

    /// The tools for what is selected, the comment being written on it, or the card of a comment whose words were clicked.
    fn render_read_beside(&self, pick: &Pick, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lib = self.lib.as_ref()?;
        let frame = || div().rounded(px(8.)).bg(Theme::bg_body()).border_1().border_color(Theme::line_strong()).shadow(crate::theme::shadow_card());
        let button = |id: &'static str, icon: &'static str, name: &'static str| ui::ghost_button(id, Some(icon), Some(t(name).into())).h(text_px(24.)).text_size(text_px(12.));
        Some(match &pick.beside {
            // The same names as in the right click's menu, and 批注 by the icon it has in the editor's bar.
            Beside::Tools => frame()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(2.))
                .p(px(3.))
                .child(button("read-comment", "message-square", "批注").text_color(Theme::accent()).on_click(self.on(Cmd::Read(Do::Comment), cx)))
                .child(button("read-copy", "copy", "复制").on_click(self.on(Cmd::Read(Do::Copy), cx)))
                .child(button("read-edit", "pen-line", "编辑这里").tooltip(ui::tip(t("回到这一节写，这些字已经选中：润色、找句子这些工具就在它旁边"))).on_click(self.on(Cmd::Read(Do::Edit), cx)))
                .into_any_element(),
            Beside::Writing => {
                lib.writing.comments.drafting.as_ref()?;
                frame()
                    .flex()
                    .flex_col()
                    .w(px(320.))
                    .gap(px(8.))
                    .p(px(10.))
                    .child(div().w(px(298.)).child(ui::field(&lib.writing.comments.draft, window, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("点别处也会保存")))
                            .child(ui::ghost_button("read-comment-drop", None, Some(t("取消").into())).h(text_px(26.)).text_size(text_px(12.)).on_click(self.on(Cmd::Read(Do::Drop), cx)))
                            .child(ui::primary_button("read-comment-keep", t("保存")).h(text_px(26.)).px(px(10.)).text_size(text_px(12.)).on_click(self.on(Cmd::Read(Do::Keep), cx))),
                    )
                    .into_any_element()
            }
            Beside::Comment(id) => {
                let comment = lib.writing.comments.list.iter().find(|comment| comment.id == *id && comment.state != comment::DONE)?;
                div().rounded(px(10.)).shadow(crate::theme::shadow_card()).child(self.render_comment(comment, true, 360., cx)).into_any_element()
            }
        })
    }
}
