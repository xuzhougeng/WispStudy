//! What the assistant changed in the section on show. In 助手写 the
//! conversation writes the manuscript (`write_agent.rs`); what it changed
//! in answer to the last question is lit here: a tint runs over the new
//! words and settles, and a bar in the left margin stays on their lines.
//! Under the manuscript the changes are listed, in either mode of 写作:
//! each can be gone to, taken back alone or with the rest, and each is one
//! step for the editor's Undo as well. Nothing is asked of a model here:
//! where the person writes, the assistant is not told to change the text.

use crate::app::{Cmd, Workspace};
use crate::i18n::t;
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use std::ops::Range;
use std::time::Instant;
use wisp_core::revise;

/// How long the tint takes to settle once it has run over a change, in seconds.
const SETTLE: f32 = 1.2;

/// The assistant's changes to the section on show.
#[derive(Default)]
pub struct Revising {
    pub run: Option<Run>,
    /// The manuscript's text as the places of the changes were last worked out for.
    seen: String,
}

/// What the assistant changed in a section in answer to one question.
pub struct Run {
    pub section: String,
    pub made: Vec<Made>,
    /// The change being looked at.
    pub at: Option<usize>,
}

/// A change made to the manuscript.
pub struct Made {
    /// Where its words stand now; none once they were changed by hand.
    pub range: Option<Range<usize>>,
    pub old: String,
    pub new: String,
    /// The words to light up, as bytes of `new`.
    lit: Vec<Range<usize>>,
    /// When it landed or was last gone to: the tint runs from then.
    since: Instant,
    pub undone: bool,
}

/// What the list of changes can be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    /// Show a change in the manuscript.
    Go(usize),
    /// The next change (true) or the one before.
    Step(bool),
    Undo(usize),
    UndoAll,
    /// Keep the changes: the list is done with them.
    Keep,
}

impl Revising {
    /// Show the changes the assistant made to `section`: `text` is what the section reads now, and each change
    /// the bytes of it that are new with the words that stood there before.
    pub(crate) fn show(&mut self, section: &str, text: &str, changes: Vec<(Range<usize>, String)>) {
        let now = Instant::now();
        let made = changes.into_iter().map(|(range, old)| {
            let new = text[range.clone()].to_string();
            // A section written from nothing is all new: its bar in the margin says so, and no tint lies over all of it.
            let lit = if old.trim().is_empty() && new.trim().len() == text.trim().len() { vec![] } else { revise::added(&old, &new) };
            Made { range: Some(range), lit, old, new, since: now, undone: false }
        }).collect();
        self.seen = text.to_string();
        self.run = Some(Run { section: section.into(), made, at: None });
    }

    /// The person went on to another section: the changes were to this one.
    pub fn leave(&mut self) {
        self.run = None;
    }

    /// The manuscript now reads `text`: the changes keep their places in it.
    fn sync(&mut self, text: &str) {
        let Some(run) = self.run.as_mut() else { return };
        if self.seen == text { return }
        for made in &mut run.made { made.range = made.range.take().and_then(|range| revise::follow(&range, &self.seen, text)) }
        self.seen = text.to_string();
    }
}

impl Made {
    /// The stretches of the manuscript to tint for this change at `now`, and whether the tint is still moving.
    fn tints(&self, now: Instant, pointed: bool) -> (Vec<(Range<usize>, Hsla)>, bool) {
        let Some(range) = self.range.as_ref().filter(|_| !self.undone) else { return (vec![], false) };
        let dark = crate::theme::dark();
        let (strong, rest) = (if dark { 0.5 } else { 0.4 }, if pointed { if dark { 0.25 } else { 0.17 } } else if dark { 0.2 } else { 0.13 });
        let total: usize = self.lit.iter().map(|lit| lit.len()).sum();
        let (run, since) = (run_secs(total), now.duration_since(self.since).as_secs_f32());
        let place = |lit: &Range<usize>| range.start + lit.start..range.start + lit.end;
        if since < run {
            // The tint runs over the new words from their start, slowing as it gets there.
            let mut left = (ease(since / run) * total as f32) as usize;
            let mut out = vec![];
            for lit in &self.lit {
                if left == 0 { break }
                let reach = self.new.floor_char_boundary(lit.start + left.min(lit.len()));
                left = left.saturating_sub(lit.len());
                if reach > lit.start { out.push((place(&(lit.start..reach)), Theme::accent().opacity(strong))) }
            }
            return (out, true);
        }
        let settled = ((since - run) / SETTLE).min(1.);
        let tint = Theme::accent().opacity(strong + (rest - strong) * ease(settled));
        (self.lit.iter().map(|lit| (place(lit), tint)).collect(), settled < 1.)
    }

    /// How strongly its bar in the margin shows at `now`.
    fn bar(&self, now: Instant, pointed: bool) -> f32 {
        let since = now.duration_since(self.since).as_secs_f32();
        let rest = if pointed { 0.95 } else { 0.5 };
        let landed = (since / (run_secs(self.lit.iter().map(|lit| lit.len()).sum()) + SETTLE)).min(1.);
        1. + (rest - 1.) * ease(landed)
    }
}

/// How long the tint takes to run over a change of this many bytes: longer for more words, never long.
fn run_secs(bytes: usize) -> f32 {
    (bytes as f32 * 0.004).clamp(0.4, 1.3)
}

/// Fast at first, slow at the end.
fn ease(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// A text in one line, cut to `most` characters.
fn clipped(text: &str, most: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= most { line } else { format!("{}…", line.chars().take(most).collect::<String>()) }
}

/// A change in one line for the list, with how its pieces are drawn: words taken out struck, then what came;
/// words put in alone after a plus; and where most of a sentence stayed, the sentence around its new words, those shaded.
fn line(made: &Made) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let struck = HighlightStyle { color: Some(Theme::danger()), strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: None }), ..Default::default() };
    let quiet = HighlightStyle { color: Some(Theme::text_tertiary()), ..Default::default() };
    let (old, new) = (made.old.trim(), made.new.trim());
    if new.is_empty() {
        let went = clipped(old, 110);
        return (went.clone(), vec![(0..went.len(), struck)]);
    }
    // Words put after a sentence that stays, or at the end of the section.
    let after = made.new.strip_prefix(made.old.trim_end()).filter(|_| !old.is_empty());
    if old.is_empty() || after.is_some() {
        let plus = "＋ ";
        return (format!("{plus}{}", clipped(after.unwrap_or(new), 110)), vec![(0..plus.len(), quiet)]);
    }
    let whole = made.lit.is_empty() || (made.lit.len() == 1 && made.lit[0].len() >= new.len());
    if whole {
        let (went, arrow, came) = (clipped(old, 36), "  →  ", clipped(new, 90));
        return (format!("{went}{arrow}{came}"), vec![(0..went.len(), struck), (went.len()..went.len() + arrow.len(), quiet)]);
    }
    // From a few words before the first new one.
    let first = made.lit[0].start;
    let mut cut = made.new[..first].char_indices().rev().nth(23).map_or(0, |(at, _)| at);
    if cut > 0 { cut = made.new[cut..first].char_indices().find(|(_, c)| c.is_whitespace()).map_or(cut, |(at, c)| cut + at + c.len_utf8()) }
    let end = made.new[cut..].char_indices().nth(110).map_or(made.new.len(), |(at, _)| cut + at);
    let lead = if cut > 0 { "…" } else { "" };
    let text = format!("{lead}{}{}", made.new[cut..end].replace('\n', " "), if end < made.new.len() { "…" } else { "" });
    let shade = HighlightStyle { background_color: Some(Theme::accent().opacity(if crate::theme::dark() { 0.38 } else { 0.24 })), ..Default::default() };
    let marks = made.lit.iter().filter(|lit| lit.start < end && lit.end > cut).map(|lit| (lead.len() + lit.start.max(cut) - cut..lead.len() + lit.end.min(end) - cut, shade)).collect();
    (text, marks)
}

impl Workspace {
    pub(crate) fn run_revise(&mut self, what: Do, cx: &mut Context<Self>) {
        match what {
            Do::Go(n) => self.revise_go(n, cx),
            Do::Step(on) => {
                let Some(run) = self.lib.as_ref().and_then(|lib| lib.writing.revise.run.as_ref()) else { return };
                // Past the changes that can still be shown: one taken back has nothing to light.
                let there: Vec<usize> = run.made.iter().enumerate().filter(|(_, made)| made.range.is_some() && !made.undone).map(|(n, _)| n).collect();
                let Some(now) = run.at.and_then(|at| there.iter().position(|n| *n == at)) else { return if let Some(first) = there.first() { self.revise_go(*first, cx) } };
                self.revise_go(there[if on { (now + 1) % there.len() } else { (now + there.len() - 1) % there.len() }], cx);
            }
            Do::Undo(n) => self.revise_undo(Some(n), cx),
            Do::UndoAll => self.revise_undo(None, cx),
            Do::Keep => {
                if let Some(lib) = self.lib.as_mut() {
                    // Changes made by the conversation that writes are not lit again when the section is come back to.
                    if let Some(run) = lib.writing.revise.run.as_ref() { lib.writing.agent.changed.remove(&run.section); }
                    lib.writing.revise.leave();
                }
            }
        }
        cx.notify();
    }

    /// Bring byte `at` of the manuscript into view, when it is not.
    fn revise_reveal(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let writing = &lib.writing;
        let Some(top) = writing.editor.read(cx).top_of(at) else { return };
        let (seen, height) = (-writing.editor_scroll.offset().y, writing.editor_scroll.bounds().size.height);
        // A few lines above it stay in view, and room under it for what is written there.
        if top < seen || top + px(120.) > seen + height { writing.editor_scroll.set_offset(point(px(0.), -(top - px(70.)).max(px(0.)))) }
    }

    /// Show a change where it stands, its tint running again.
    fn revise_go(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(run) = self.lib.as_mut().and_then(|lib| lib.writing.revise.run.as_mut()) else { return };
        let Some(made) = run.made.get_mut(n) else { return };
        let Some(at) = made.range.as_ref().map(|range| range.start) else { return };
        made.since = Instant::now();
        run.at = Some(n);
        self.revise_reveal(at, cx);
        cx.notify();
    }

    /// Take a change back, or all of them from the last to the first: the words that were there return.
    fn revise_undo(&mut self, which: Option<usize>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let writing = &mut lib.writing;
        let text = writing.editor.read(cx).text().to_string();
        writing.revise.sync(&text);
        let Some(run) = writing.revise.run.as_ref() else { return };
        let order: Vec<usize> = which.map_or_else(|| (0..run.made.len()).rev().collect(), |n| vec![n]);
        let (mut back, mut first) = (0, None);
        for n in order {
            let Some((range, old)) = writing.revise.run.as_ref().and_then(|run| run.made.get(n)).filter(|made| !made.undone).and_then(|made| Some((made.range.clone()?, made.old.clone()))) else { continue };
            let before = writing.editor.read(cx).text().len();
            writing.editor.update(cx, |input, cx| input.replace_apart(range.clone(), &old, cx));
            let after = writing.editor.read(cx).text().to_string();
            writing.revise.sync(&after);
            // Its old words stand where its new ones did.
            let Some(made) = writing.revise.run.as_mut().and_then(|run| run.made.get_mut(n)) else { continue };
            (made.undone, made.range) = (true, Some(range.start..range.start + (after.len() + range.len() - before)));
            (back, first) = (back + 1, Some(range.start));
        }
        if let Some(at) = first { self.revise_reveal(at, cx) }
        // All taken back: the list has nothing left to say.
        if which.is_none() && let Some(lib) = self.lib.as_mut() {
            lib.writing.revise.leave();
            if back > 0 { self.say(tn!("已撤销助手的 {n} 处改动", "Took back the assistant's {n}", back, "change"), cx) }
        }
        cx.notify();
    }

    /// What the editor is to draw for the assistant's changes at `now`: the stretches to tint, the lines to put
    /// a bar beside (the stretch and how strongly), and whether any of it is still moving.
    #[allow(clippy::type_complexity)]
    pub(crate) fn revise_marks(&mut self, text: &str, now: Instant) -> (Vec<(Range<usize>, Hsla)>, Vec<(Range<usize>, f32)>, bool) {
        let Some(lib) = self.lib.as_mut() else { return (vec![], vec![], false) };
        let writing = &mut lib.writing;
        writing.revise.sync(text);
        let Some(run) = writing.revise.run.as_ref().filter(|run| run.section == writing.section_id) else { return (vec![], vec![], false) };
        let (mut tints, mut bars, mut moving) = (vec![], vec![], false);
        for (n, made) in run.made.iter().enumerate() {
            let pointed = run.at == Some(n);
            let (lit, still) = made.tints(now, pointed);
            tints.extend(lit);
            moving |= still;
            // The bar stands beside the lines of the new words, not beside all that was put there again.
            if let Some(range) = made.range.as_ref().filter(|_| !made.undone) {
                let lines = made.lit.first().zip(made.lit.last()).map_or(range.clone(), |(first, last)| range.start + first.start..range.start + last.end);
                bars.push((lines, made.bar(now, pointed)));
            }
        }
        (tints, bars, moving)
    }

    /// The bars in the manuscript's left margin, by where the editor last drew those bytes.
    pub(crate) fn render_revise_bars(&self, bars: &[(Range<usize>, f32)], cx: &App) -> Vec<AnyElement> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let editor = lib.writing.editor.read(cx);
        bars.iter()
            .filter_map(|(range, strength)| {
                let (top, bottom) = (editor.top_of(range.start)?, editor.top_of(range.end)?);
                // The text's top padding, then from the first line's top to the last line's foot.
                Some(div().absolute().left(px(6.)).top(px(14.) + top + px(2.)).w(px(3.)).h((bottom - top).max(px(0.)) + px(22.)).rounded(px(1.5)).bg(Theme::accent().opacity(*strength)).into_any_element())
            })
            .collect()
    }

    /// Under the manuscript, in a frame of its own: how many changes the assistant made to the section on show,
    /// each in a line to go to or take back, and the way to keep or take back all of them.
    pub(crate) fn render_revise_run(&self, run: &Run, cx: &mut Context<Self>) -> Div {
        let small = |id: &'static str, name: &'static str| ui::ghost_button(id, None, Some(name.into())).h(text_px(22.)).px(px(6.)).text_size(text_px(12.));
        let (count, standing) = (run.made.len(), run.made.iter().filter(|made| !made.undone && made.range.is_some()).count());
        let place = run.at.filter(|at| run.made.get(*at).is_some_and(|made| made.range.is_some())).map(|at| at + 1);
        let rows: Vec<AnyElement> = run
            .made
            .iter()
            .enumerate()
            .map(|(n, made)| {
                let gone = made.undone || made.range.is_none();
                let (line, marks) = line(made);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(4.))
                    .rounded(px(6.))
                    .when(run.at == Some(n) && !gone, |d| d.bg(Theme::accent_soft()))
                    .child(
                        div()
                            .id(("revise-change", n))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(8.))
                            .h(text_px(26.))
                            .px(px(4.))
                            .rounded(px(6.))
                            .when(made.range.is_some(), |d| d.cursor_pointer().hover(|s| s.bg(Theme::hover())).on_click(self.on(Cmd::Revise(Do::Go(n)), cx)))
                            .child(div().flex_none().w(px(14.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child((n + 1).to_string()))
                            .child(div().flex_1().min_w_0().truncate().text_size(text_px(12.)).text_color(if gone { Theme::text_tertiary() } else { Theme::text() }).child(StyledText::new(line).with_highlights(marks))),
                    )
                    .child(if made.undone {
                        div().flex_none().px(px(6.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("已撤销")).into_any_element()
                    } else if made.range.is_none() {
                        div().flex_none().px(px(6.)).text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(t("后来改过")).into_any_element()
                    } else {
                        ui::ghost_button(("revise-undo", n), None, Some(t("撤销").into())).h(text_px(22.)).px(px(6.)).text_size(text_px(11.)).on_click(self.on(Cmd::Revise(Do::Undo(n)), cx)).into_any_element()
                    })
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(5.))
            .mx(px(16.))
            .mb(px(8.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .border_1()
            .border_color(Theme::line())
            .bg(Theme::bg_body())
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(4.))
                    .min_h(px(22.))
                    .child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_secondary()).child(tn!("助手在这一节改了 {n} 处", "The assistant made {n} in this section", count, "change")))
                    .child(div().flex_1())
                    .when(standing > 1, |d| {
                        d.child(ui::ghost_button("revise-prev", Some("chevron-up"), None).size(px(22.)).min_w(px(22.)).on_click(self.on(Cmd::Revise(Do::Step(false)), cx)))
                            .child(div().flex_none().text_size(text_px(11.)).text_color(Theme::text_tertiary()).child(format!("{}/{count}", place.map_or("–".to_string(), |at| at.to_string()))))
                            .child(ui::ghost_button("revise-next", Some("chevron-down"), None).size(px(22.)).min_w(px(22.)).on_click(self.on(Cmd::Revise(Do::Step(true)), cx)))
                    })
                    .when(standing > 0, |d| d.child(small("revise-undo-all", t("全部撤销")).on_click(self.on(Cmd::Revise(Do::UndoAll), cx))))
                    // Nothing left standing: the list can only be put away.
                    .child(ui::primary_button("revise-keep", if standing > 0 { t("保留") } else { t("收起") }).h(text_px(22.)).px(px(9.)).text_size(text_px(12.)).on_click(self.on(Cmd::Revise(Do::Keep), cx))),
            )
            .when(!rows.is_empty(), |d| d.child(div().id("revise-changes").flex().flex_col().max_h(px(108.)).overflow_y_scroll().children(rows)))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{clipped, ease, run_secs, Made};
    use std::time::{Duration, Instant};

    fn made(new: &str, lit: std::ops::Range<usize>, since: Instant) -> Made {
        Made { range: Some(10..10 + new.len()), old: String::new(), new: new.into(), lit: Vec::from([lit]), since, undone: false }
    }

    #[test]
    fn the_tint_runs_over_a_change_and_settles() {
        let start = Instant::now();
        let change = made("根系生长很快", 0..18, start);
        let run = run_secs(18);
        // On its way: part of the words, at a letter's edge, and still moving.
        let (lit, moving) = change.tints(start + Duration::from_secs_f32(run * 0.2), false);
        assert!(moving && lit.len() == 1 && lit[0].0.start == 10 && lit[0].0.end > 10 && lit[0].0.end < 28 && (lit[0].0.end - 10) % 3 == 0);
        // Over them all, fading to rest.
        let (lit, moving) = change.tints(start + Duration::from_secs_f32(run + 0.3), false);
        assert!(moving && lit[0].0 == (10..28));
        let strong = lit[0].1.a;
        let (lit, moving) = change.tints(start + Duration::from_secs(10), false);
        assert!(!moving && lit[0].0 == (10..28) && lit[0].1.a < strong);
        // The one being looked at rests stronger, and its bar too.
        assert!(change.tints(start + Duration::from_secs(10), true).0[0].1.a > lit[0].1.a);
        assert!(change.bar(start + Duration::from_secs(10), true) > change.bar(start + Duration::from_secs(10), false));
    }

    #[test]
    fn a_change_taken_back_or_changed_by_hand_is_not_lit() {
        let start = Instant::now();
        let mut change = made("abc", 0..3, start);
        change.undone = true;
        assert_eq!(change.tints(start, false), (vec![], false));
        change.undone = false;
        change.range = None;
        assert_eq!(change.tints(start, false), (vec![], false));
    }

    #[test]
    fn small_helpers() {
        assert_eq!(clipped("a  b\n\nc", 10), "a b c");
        assert_eq!(clipped("根系生长很快", 3), "根系生…");
        assert!(ease(0.) == 0. && ease(1.) == 1. && ease(0.5) > 0.5 && ease(2.) == 1.);
        assert!(run_secs(1) == 0.4 && run_secs(100_000) == 1.3);
    }
}
