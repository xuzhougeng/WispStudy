//! 归入已有文献: a supplement, a figure file or a methods PDF taken in on its own, as a paper of its own, filed
//! under the paper it belongs to (`Modal::FileInto`, from the right-click menu of the list). The engine's
//! `Store::file_into` does the filing: the files become the paper's attachments with what was marked on them, and
//! the records they came with go to the trash. Here are the dialog that asks which paper, and the way back.

use crate::app::{Cmd, Modal, Workspace};
use crate::i18n::t;
use crate::input::InputEvent;
use crate::theme::{Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::filter;

/// How many papers the dialog draws: the box over them narrows a longer list.
const OFFERED: usize = 60;

/// A title short enough for a sentence of a toast.
fn brief(title: &str) -> String {
    if title.chars().count() <= 32 { return title.to_string() }
    format!("{}…", title.chars().take(30).collect::<String>().trim_end())
}

impl Workspace {
    /// Whether these papers can be filed under another: each has a file, and another paper is there to take them.
    pub(crate) fn can_file(&self, papers: &[String]) -> bool {
        let Some(lib) = &self.lib else { return false };
        lib.papers.len() > papers.len() && papers.iter().all(|id| lib.files.iter().any(|f| &f.paper_id == id))
    }

    /// The papers the dialog offers, in the order of the library (the newest first): every paper but those
    /// being filed, narrowed by what is typed into the dialog's box.
    pub(crate) fn file_targets(&self, cx: &App) -> Vec<String> {
        let (Some(lib), Some(Modal::FileInto { papers, query, .. })) = (&self.lib, &self.modal) else { return vec![] };
        let query = query.read(cx).text().to_string();
        lib.papers.iter().filter(|p| !papers.contains(&p.id) && filter::says(p, &query)).map(|p| p.id.clone()).collect()
    }

    pub(crate) fn ask_file_into(&mut self, papers: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_file(&papers) { return self.fail(t("要归入的文献得有文件，库里还得有另一篇文献。"), cx) }
        self.modal_subscriptions.clear();
        self.modal_error.clear();
        let query = self.modal_input("按题目、作者、年份查找", "", false, window, cx);
        // The papers offered follow what is typed; one chosen that is no longer among them is not chosen.
        let typed = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if *event != InputEvent::Changed { return }
            let offered = this.file_targets(cx);
            if let Some(Modal::FileInto { to, .. }) = this.modal.as_mut() && to.as_ref().is_some_and(|to| !offered.contains(to)) { *to = None }
            this.modal_error.clear();
            cx.notify();
        });
        self.modal_subscriptions.push(typed);
        query.read(cx).focus(window);
        self.modal = Some(Modal::FileInto { papers, query, to: None });
        cx.notify();
    }

    /// Enter with no paper chosen: the only one offered is chosen (a second Enter files), else the dialog says what it waits for.
    pub(crate) fn choose_only_file_target(&mut self, cx: &mut Context<Self>) {
        let mut offered = self.file_targets(cx);
        match (offered.pop(), offered.is_empty(), self.modal.as_mut()) {
            (Some(only), true, Some(Modal::FileInto { to, .. })) => *to = Some(only),
            _ => self.modal_error = t("选一篇文献。").into(),
        }
        cx.notify();
    }

    pub(crate) fn file_into(&mut self, papers: Vec<String>, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(title) = lib.paper(&to).map(|p| brief(&p.title)) else { return };
        // Where the files were read to is written first, so that it goes along with them.
        lib.keep_places(cx);
        let shown = lib.selected.as_ref().is_some_and(|id| papers.contains(id));
        let moved = match lib.store.file_into(&papers, &to) {
            Ok(moved) => moved,
            Err(err) => return self.modal_error = format!("{err:#}"),
        };
        self.modal = None;
        if !self.change(cx, |_| Ok(())) { return }
        if let Some(lib) = self.lib.as_mut() {
            // The paper they went to shows them, so that where they are now is seen.
            lib.unfolded.insert(to.clone());
            lib.lay_rows();
            lib.marked.clear();
        }
        // The record on show is in the trash now: the paper its file went to is shown instead.
        if shown { self.select(Some(to.clone()), window, cx) }
        if let Some(lib) = self.lib.as_mut() { lib.close_pdf() }
        self.ensure_pdf(cx);
        let said = if papers.len() == 1 {
            tn!("已把 {n} 个文件归入「{}」，原来的那条在回收站里。", "Filed {n} under “{}”; the record it came with is in the trash.", moved, "file", title)
        } else {
            tn!("已把 {n} 个文件归入「{}」，原来的 {} 条在回收站里。", "Filed {n} under “{}”; the {} records they came with are in the trash.", moved, "file", title, papers.len())
        };
        self.say_undoable(said, Cmd::UndoFiling(papers), cx);
        window.focus(&self.focus);
    }

    /// The toast's 撤销: the records come out of the trash and take their files back, as what they were.
    pub(crate) fn undo_filing(&mut self, papers: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.toast = None;
        let Some(lib) = self.lib.as_mut() else { return };
        lib.keep_places(cx);
        // The paper on show since the filing is the one the files went to, when a record of theirs was on show before.
        let shown = lib.selected.as_ref().is_some_and(|id| lib.files.iter().any(|f| &f.paper_id == id && lib.store.filed_from(&f.id).is_some_and(|from| papers.contains(&from))));
        let done = self.change(cx, |lib| {
            for id in &papers { lib.store.restore_paper(id)? }
            Ok(())
        });
        if !done { return }
        // A single record is shown again, as it was before it was filed.
        if let ([only], true) = (papers.as_slice(), shown) { self.select(Some(only.clone()), window, cx) }
        // A file being read may have gone back to its own record.
        if let Some(lib) = self.lib.as_mut() { lib.close_pdf() }
        self.ensure_pdf(cx);
        self.say(t("已撤销：文件回到了原来的条目上。"), cx);
    }

    pub(crate) fn render_file_into_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (Some(lib), Some(Modal::FileInto { papers, query, to })) = (&self.lib, &self.modal) else { return None };
        // What is being filed, by the names of the files: that is what tells one supplement from another.
        let files: Vec<&str> = lib.files.iter().filter(|f| papers.contains(&f.paper_id)).map(|f| f.title.as_str()).collect();
        let what = match files.as_slice() {
            [one] => one.to_string(),
            [first, ..] => tn!("{}等 {n} 个文件", "{} and {n} in all", files.len(), "file", first),
            [] => String::new(),
        };
        let offered = self.file_targets(cx);
        let more = offered.len().saturating_sub(OFFERED);
        let rows = offered.iter().take(OFFERED).enumerate().filter_map(|(i, id)| {
            let p = lib.paper(id)?;
            let chosen = to.as_deref() == Some(id.as_str());
            let mut meta = vec![];
            let authors = crate::i18n::short_authors(p);
            if !authors.is_empty() { meta.push(authors) }
            if let Some(year) = p.year { meta.push(year.to_string()) }
            if !p.journal.is_empty() { meta.push(p.journal.clone()) }
            Some(
                div()
                    .id(("file-to", i))
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w_full()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(chosen, |d| d.bg(Theme::accent_soft()))
                    .when(!chosen, |d| d.hover(|s| s.bg(Theme::hover())))
                    .child(div().font_weight(if chosen { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(ui::one_line(&p.title)))
                    .when(!meta.is_empty(), |d| d.child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(ui::one_line(&meta.join(" · ")))))
                    .on_click(self.on(Cmd::ChooseFileTarget(id.clone()), cx)),
            )
        });
        let none = offered.is_empty().then(|| div().px(px(10.)).py(px(14.)).text_color(Theme::text_tertiary()).child(t("没有匹配的文献")));
        let rest = (more > 0).then(|| div().px(px(10.)).py(px(6.)).text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(tf!("还有 {more} 篇，输入几个字找它。", "{more} more: type a few words to find one.")));
        Some(
            ui::dialog(560.)
                .child(ui::dialog_title(t("归入已有文献")))
                .child(
                    div().flex().flex_col().flex_none().gap(px(8.)).px(px(20.)).pt(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).line_height(text_px(21.)).child(what))
                        .child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(t("选它所属的文献。文件成为那篇文献的附件，标注、读到的位置和取出的图片都跟着；原来的这一条移到回收站，从回收站恢复时文件也回来。")))
                        .child(query.clone()),
                )
                .child(div().id("modal-body").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(2.)).px(px(10.)).py(px(8.)).children(rows).children(none).children(rest))
                .child(div().px(px(20.)).child(ui::error_line(&self.modal_error)))
                .child(
                    div().flex().flex_none().justify_end().gap(px(8.)).px(px(20.)).py(px(14.))
                        .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
                        .child(ui::primary_button("modal-ok", t("归入").to_string()).when(to.is_none(), |d| d.opacity(0.4)).when(to.is_some(), |d| d.on_click(self.on(Cmd::SubmitModal, cx)))),
                ),
        )
    }
}
