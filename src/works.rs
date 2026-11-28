//! What a library goes on with while another is on show (#275). A run of the assistant through a library's
//! papers (筛选, 提取, 查找全文, a batch of translations, a paper's whole translation), and the runs of its saved
//! searches, do not stop when the library is left: like a conversation that is answering, the library is kept
//! off screen (`Workspace::parked`) and its answers are handled through `Workspace::in_library` as if it were on show.
//!
//! `Library::works` is the one place that says which runs there are and how far each is; `Library::at_work`
//! is what keeps a library when it is left. A kind of run belongs here only once every callback of it goes
//! through `in_library`: one that still asks for the library on show would never hear its answers off
//! screen, and the library would be kept for good.
//!
//! What is seen of it: the mark on a library's card or row of the home page (`Workspace::off_screen_mark`),
//! an entry at the foot of the sidebar of the library on show for each run elsewhere (`render_elsewhere`),
//! a line said when a run ends off screen (`Work::ended`), and a row of the home page's bell (`bell.rs`).

use crate::app::{Cmd, Library, Workspace};
use crate::i18n::t;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};

/// The runs that go on off screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Screen,
    Extract,
    Fetch,
    /// Titles and abstracts of the list.
    Translate,
    /// A paper's full text, beside its pages.
    Facing,
    /// Saved searches run and imported (检索与导入), counted in records.
    Search,
    /// A manuscript's version in another language: its first pass, and the blocks that follow the primary (`write_translate.rs`).
    Version,
}

impl Kind {
    #[cfg(test)]
    pub(crate) const ALL: [Kind; 7] = [Kind::Screen, Kind::Extract, Kind::Fetch, Kind::Translate, Kind::Facing, Kind::Search, Kind::Version];

    /// The run's name, a key of `locales/en.json`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::Screen => "筛选",
            Kind::Extract => "提取词条",
            Kind::Fetch => "查找全文",
            Kind::Translate => "翻译",
            Kind::Facing => "翻译全文",
            Kind::Search => "运行检索",
            Kind::Version => "翻译稿件",
        }
    }

    /// What it counts, in Chinese: papers, the texts of a list, the paragraphs of a paper.
    fn unit(self) -> &'static str {
        match self {
            Kind::Screen | Kind::Extract | Kind::Fetch | Kind::Search => "篇",
            Kind::Translate => "条",
            Kind::Facing | Kind::Version => "段",
        }
    }
}

/// One run of a library: how far it is, and how it ended once it is no longer running.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Work {
    pub kind: Kind,
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    pub running: bool,
    pub error: String,
}

impl Work {
    /// On a card or a row: 「筛选 12 / 80」.
    pub(crate) fn brief(&self) -> String {
        format!("{} {} / {}", t(self.kind.label()), self.done, self.total)
    }

    /// What is said of a run that ended while its library `name` was off screen: that it is through, with what
    /// did not come of it; that it stopped, with why; or that it was stopped short.
    pub(crate) fn ended(&self, name: &str) -> String {
        let (label, unit, done, total, failed) = (t(self.kind.label()), self.kind.unit(), self.done, self.total, self.failed);
        if done < total && !self.error.is_empty() {
            let error = crate::i18n::reasons(&self.error);
            return tf!("「{name}」的{label}停了（{done} / {total}）：{error}", "{label} in “{name}” stopped at {done} of {total}: {error}");
        }
        if done < total { return tf!("「{name}」的{label}已停止：{done} / {total}", "{label} in “{name}” was stopped at {done} of {total}") }
        if failed > 0 { return tf!("「{name}」的{label}做完了：{total} {unit}，其中 {failed} {unit}没有成", "{label} in “{name}” is done: {total}, {failed} of them failed") }
        tf!("「{name}」的{label}做完了：{total} {unit}", "{label} in “{name}” is done: {total}")
    }

    /// The same without the library, under its name in the bell's list (`bell.rs`).
    pub(crate) fn outcome(&self) -> String {
        let (label, unit, done, total, failed) = (t(self.kind.label()), self.kind.unit(), self.done, self.total, self.failed);
        if done < total && !self.error.is_empty() {
            let error = crate::i18n::reasons(&self.error);
            return tf!("{label}停了（{done} / {total}）：{error}", "{label} stopped at {done} of {total}: {error}");
        }
        if done < total { return tf!("{label}已停止：{done} / {total}", "{label} was stopped at {done} of {total}") }
        if failed > 0 { return tf!("{label}做完了：{total} {unit}，其中 {failed} {unit}没有成", "{label} is done: {total}, {failed} of them failed") }
        tf!("{label}做完了：{total} {unit}", "{label} is done: {total}")
    }
}

impl Library {
    /// The runs there is anything to say of, those going on and those that have ended and are still on its pages.
    pub(crate) fn works(&self) -> Vec<Work> {
        let mut works = vec![];
        let screen = &self.screen;
        if screen.total > 0 || screen.running() { works.push(Work { kind: Kind::Screen, done: screen.done, total: screen.total, failed: screen.failed, running: screen.running(), error: screen.error.clone() }) }
        let extract = &self.extract;
        if extract.total > 0 || extract.running() { works.push(Work { kind: Kind::Extract, done: extract.done, total: extract.total, failed: extract.failed, running: extract.running(), error: extract.error.clone() }) }
        if let Some(run) = &self.fetching { works.push(Work { kind: Kind::Fetch, done: run.rows.len(), total: run.total, failed: run.count("failed"), running: run.running(), error: String::new() }) }
        for (kind, run) in [(Kind::Translate, &self.translation_run), (Kind::Facing, &self.facing_run)] {
            if let Some(run) = run { works.push(Work { kind, done: run.batch.done(), total: run.batch.total, failed: run.batch.failed, running: run.batch.running(), error: run.batch.error.clone() }) }
        }
        if let Some(batch) = self.writing.translate.run_batch() { works.push(Work { kind: Kind::Version, done: batch.done(), total: batch.total, failed: batch.failed, running: batch.running(), error: batch.error.clone() }) }
        // The searches going and those over since none was: one run.
        let searching = &self.searching;
        if !searching.going.is_empty() || searching.total > 0 {
            let (done, total) = searching.counts();
            works.push(Work { kind: Kind::Search, done, total, failed: 0, running: !searching.going.is_empty(), error: searching.error.clone() });
        }
        works
    }

    /// The runs going on right now.
    pub(crate) fn running(&self) -> Vec<Work> {
        self.works().into_iter().filter(|work| work.running).collect()
    }

    /// Whether leaving the library must not let go of it: a conversation is answering or waits for a yes, or a run is going on.
    pub(crate) fn at_work(&self) -> bool {
        self.answering() || self.works().iter().any(|work| work.running)
    }
}

impl Workspace {
    /// A library left while it was at work (`Workspace::parked`): a mark for its card or row that says what it goes on
    /// with. A yes it waits for comes first, since nothing moves until it is given; then an answer on its way; then the
    /// first of its runs with how far it is. None for one not at work off screen.
    pub(crate) fn off_screen_mark(&self, id: &str) -> Option<Div> {
        let lib = self.parked.iter().find(|lib| lib.info.id == id)?;
        let mark = SharedString::from(format!("off-screen-{id}"));
        if lib.answering() { return Some(Self::at_work_mark(mark, lib.waits_for_yes())) }
        let work = lib.running().into_iter().next()?;
        Some(div().flex().flex_none().items_center().gap(px(5.)).text_size(text_px(12.)).text_color(Theme::text_secondary()).child(ui::spinner(mark, 11., Theme::accent())).child(work.brief()))
    }

    /// At the foot of the sidebar, under the runs of the library on show: what the libraries left at work go on with,
    /// an entry each. The run is named before its library, so a narrow sidebar cuts the library's name, which the
    /// tooltip has whole, and not what is being done. A click goes to that library, where the run's own page has
    /// the rest (and its stop).
    pub(crate) fn render_elsewhere(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let blue = theme::accent_of("blue").strong();
        let mut entries = vec![];
        for lib in &self.parked {
            let (id, name) = (lib.info.id.clone(), ui::one_line(&lib.info.name));
            let entry = |n: usize, said: String, count: String, bar: Option<(usize, usize)>, waits: bool, this: &Self, cx: &mut Context<Self>| {
                let key = SharedString::from(format!("elsewhere-{id}-{n}"));
                div().id(key.clone()).flex().flex_none().flex_col().gap(px(7.)).px(px(8.)).py(px(10.)).rounded(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover()))
                    .tooltip(ui::tip(tf!("在「{name}」里，点击打开", "In “{name}”: click to open")))
                    .on_click(this.on(Cmd::OpenLibrary(id.clone()), cx))
                    .child(div().flex().items_center().gap(px(6.)).text_size(text_px(12.)).line_height(text_px(17.))
                        .child(if waits { ui::icon("info", 12., Theme::accent()).into_any_element() } else { ui::spinner(key, 12., blue).into_any_element() })
                        .child(div().flex_1().min_w_0().truncate().text_color(Theme::text_secondary()).child(format!("{said} · {name}")))
                        .child(div().flex_none().text_color(if waits { Theme::accent() } else { Theme::text_tertiary() }).child(count))
                        .child(ui::icon("chevron-right", 12., Theme::text_tertiary())))
                    .when_some(bar, |d, (done, total)| d.child(ui::progress_bar(total, &[(done, "", blue)])))
                    .into_any_element()
            };
            if lib.answering() {
                let waits = lib.waits_for_yes();
                entries.push(entry(0, t("助手").to_string(), t(if waits { "等你同意" } else { "回答中" }).to_string(), None, waits, self, cx));
            }
            for (n, work) in lib.running().into_iter().enumerate() {
                entries.push(entry(n + 1, t(work.kind.label()).to_string(), format!("{}/{}", work.done, work.total), Some((work.done, work.total)), false, self, cx));
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's own `test` attribute.
    use super::{Kind, Work};

    #[test]
    fn every_run_has_a_name_in_english() {
        let english: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!("../locales/en.json")).unwrap();
        for kind in Kind::ALL { assert!(english.contains_key(kind.label()), "{}", kind.label()) }
    }

    #[test]
    fn a_run_that_ended_off_screen_says_how() {
        let work = |done, failed, error: &str| Work { kind: Kind::Screen, done, total: 80, failed, running: false, error: error.into() };
        assert_eq!(work(80, 0, "").ended("根毛"), "「根毛」的筛选做完了：80 篇");
        assert_eq!(work(80, 3, "").ended("根毛"), "「根毛」的筛选做完了：80 篇，其中 3 篇没有成");
        // One paper's trouble along the way is not how the run ended.
        assert_eq!(work(80, 1, "「某篇」没有判断成").ended("根毛"), "「根毛」的筛选做完了：80 篇，其中 1 篇没有成");
        assert_eq!(work(12, 0, "").ended("根毛"), "「根毛」的筛选已停止：12 / 80");
        assert_eq!(work(12, 5, "连续 5 篇没有得到可用的回答，已停止。").ended("根毛"), "「根毛」的筛选停了（12 / 80）：连续 5 篇没有得到可用的回答，已停止。");
        assert_eq!(Work { running: true, ..work(12, 0, "") }.brief(), "筛选 12 / 80");
        // Under the library's name in the bell's list.
        assert_eq!(work(80, 3, "").outcome(), "筛选做完了：80 篇，其中 3 篇没有成");
        assert_eq!(work(12, 0, "").outcome(), "筛选已停止：12 / 80");
    }
}
