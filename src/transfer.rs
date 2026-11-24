//! 移到文献库: a paper of the library (or conversation) on show moved into another library, chosen in a dialog
//! (`Modal::MoveTo`, from the paper's right-click menu). The engine's `Store::take_paper` does the moving; here
//! the window opens the library it went to on the paper, read as it was. A conversation left with no paper and
//! no question goes the way a removed conversation goes (its folder to `removed/`).

use crate::app::{Cmd, Modal, Talk, Workspace};
use crate::i18n::t;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::store::Store;

impl Workspace {
    /// The libraries a paper of the open library can go to: every other one, not the conversations.
    pub(crate) fn move_targets(&self) -> Vec<LibraryInfo> {
        let here = self.lib.as_ref().map(|lib| lib.info.id.clone()).unwrap_or_default();
        libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().filter(|info| info.id != here).cloned().collect()
    }

    pub(crate) fn ask_move_paper(&mut self, paper: String, cx: &mut Context<Self>) {
        if self.move_targets().is_empty() { return self.fail(t("还没有别的文献库可以移过去。"), cx) }
        self.modal_error.clear();
        self.modal = Some(Modal::MoveTo { paper, to: None });
        cx.notify();
    }

    pub(crate) fn move_paper(&mut self, paper: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.libraries.iter().find(|info| info.id == to).cloned() else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.answering() { return self.modal_error = t("这次回答结束后再移。").into() }
        // Where its PDF was read to is written first, so that it goes along.
        lib.keep_places(cx);
        let (reading, tab) = (lib.reading && lib.selected.as_deref() == Some(paper.as_str()), lib.tab);
        let taken = Store::open(&target.dir).and_then(|mut store| store.take_paper(&mut lib.store, &paper));
        let taken = match taken {
            Ok(taken) => taken,
            Err(err) => return self.modal_error = format!("{err:#}"),
        };
        self.modal = None;
        let from = lib.info.clone();
        // A conversation that held only this paper, where nothing was asked, has nothing left to go back to.
        let empty = from.draft && lib.store.list_papers().is_ok_and(|papers| papers.is_empty()) && lib.chat(Talk::Library).messages.is_empty() && lib.store.list_conversations("").is_ok_and(|kept| kept.iter().all(|c| c.questions == 0));
        self.close_library(cx);
        if empty && let Err(err) = libraries::remove(&self.root, &from) { self.fail(format!("{err:#}"), cx) }
        self.libraries = libraries::list(&self.root);
        self.open_library(&to, window, cx);
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != to || lib.paper(&taken.paper).is_none()) { return }
        self.select(Some(taken.paper.clone()), window, cx);
        self.run(Cmd::SetTab(tab), window, cx);
        if let Some(lib) = self.lib.as_mut() { lib.reading = reading }
        let mut said = if taken.joined { tf!("「{}」里已有这篇：文件和标注并到了那一条上。", "“{}” already had this paper: its files and annotations were added to it.", target.name) } else { tf!("已移到「{}」。", "Moved to “{}”.", target.name) };
        if !empty { said += &tf!("原来的那条在「{}」的回收站里。", " The original is in the trash of “{}”.", from.name) }
        if !taken.fields_left.is_empty() { said += &tf!("这里没有词条「{}」，它的内容没有带过来。", " This library has no field “{}”; its values were not brought.", taken.fields_left.join("」「")) }
        self.say(said, cx);
    }

    pub(crate) fn render_move_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::MoveTo { paper, to }) = &self.modal else { return None };
        let title = self.lib.as_ref().and_then(|lib| lib.paper(paper)).map(|p| p.title.clone()).unwrap_or_default();
        let draft = self.lib.as_ref().is_some_and(|lib| lib.info.draft);
        let rows = self.move_targets().into_iter().enumerate().map(|(i, info)| {
            let chosen = to.as_deref() == Some(info.id.as_str());
            let accent = theme::accent_of(&info.color);
            div()
                .id(("move-to", i))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(chosen, |d| d.bg(accent.soft()))
                .when(!chosen, |d| d.hover(|s| s.bg(Theme::hover())))
                .child(div().flex_none().size(px(10.)).rounded_full().bg(accent.strong()))
                .child(div().flex_1().min_w_0().font_weight(if chosen { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(ui::one_line(&info.name)))
                .child(div().flex_none().text_size(text_px(12.)).text_color(Theme::text_tertiary()).child(tn!("{n} 篇", "{n}", info.paper_count, "paper")))
                .on_click(self.on(Cmd::ChooseMoveTarget(info.id.clone()), cx))
        });
        let note = if draft { t("PDF、标注、读到的位置和关于它的对话都一起过去；这段对话里不再有它。") } else { t("PDF、标注、读到的位置和关于它的对话都一起过去，收录状态回到待判断；原来的那条留在本库的回收站里。") };
        Some(
            ui::dialog(460.)
                .child(ui::dialog_title(t("移到文献库")))
                .child(
                    div().id("modal-body").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(10.)).px(px(20.)).py(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).line_height(text_px(21.)).child(title))
                        .child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(note))
                        .child(div().flex().flex_col().gap(px(2.)).children(rows))
                        .child(ui::error_line(&self.modal_error)),
                )
                .child(
                    div().flex().flex_none().justify_end().gap(px(8.)).px(px(20.)).py(px(14.))
                        .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
                        .child(ui::primary_button("modal-ok", t("移过去").to_string()).when(to.is_none(), |d| d.opacity(0.4)).when(to.is_some(), |d| d.on_click(self.on(Cmd::SubmitModal, cx)))),
                ),
        )
    }
}
