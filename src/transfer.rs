//! 移到文献库: one paper or several marked ones of the library (or conversation) on show moved into another library,
//! chosen in a dialog (`Modal::MoveTo`, from the right-click menu). The engine's `Store::take_papers` does the moving;
//! for one paper the window opens the library it went to on that paper, read as it was, while moving several from a
//! library stays in the list with a button on the toast to open the target. A conversation left with no paper and no
//! question goes the way a removed conversation goes (its folder to `removed/`).
//!
//! 复制到文献库 is the same dialog (`copy`) and `Store::copy_papers`: the other library gets what a move brings, this one
//! keeps the papers as they were, and the window stays where it is, with 打开 on the toast.
//!
//! Papers the library chosen holds already are read as it is chosen (`Store::held_papers`): the dialog names them and
//! asks what becomes of them, 追加 or 覆盖 (`Held`), 覆盖 being its first button.

use crate::app::{Cmd, Modal, Talk, Workspace};
use crate::context::Nav;
use crate::i18n::t;
use crate::theme::{self, Theme, text_px};
use crate::ui;
use gpui::{prelude::FluentBuilder, *};
use wisp_core::libraries::{self, LibraryInfo};
use wisp_core::store::Store;
use wisp_core::transfer::Held;

/// What the toast says when the one paper taken was held there already.
fn joined_one(library: &str, held: Held) -> String {
    match held {
        Held::Append => tf!("「{}」里已有这篇：文件和标注并到了那一条上。", "“{}” already had this paper: its files and annotations were added to it.", library),
        Held::Overwrite => tf!("「{}」里已有这篇：已用这一篇覆盖。", "“{}” already had this paper: it was overwritten with this one.", library),
    }
}

/// What the toast says when every paper taken was held there already.
fn joined_all(library: &str, count: usize, held: Held) -> String {
    match held {
        Held::Append => tf!("「{}」里已有这 {} 篇：文件和标注并到了原有的条目上。", "“{}” already had these {} papers: their files and annotations were added to them.", library, count),
        Held::Overwrite => tf!("「{}」里已有这 {} 篇：已用这边的覆盖。", "“{}” already had these {} papers: they were overwritten with these.", library, count),
    }
}

fn brief(title: &str) -> String {
    let line = ui::one_line(title);
    if line.chars().count() <= 36 { return line }
    format!("{}…", line.chars().take(34).collect::<String>().trim_end())
}

impl Workspace {
    /// The libraries a paper of the open library can go to: every other one, not the conversations.
    pub(crate) fn move_targets(&self) -> Vec<LibraryInfo> {
        let here = self.lib.as_ref().map(|lib| lib.info.id.clone()).unwrap_or_default();
        libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().filter(|info| info.id != here).cloned().collect()
    }

    pub(crate) fn ask_move_papers(&mut self, papers: Vec<String>, copy: bool, cx: &mut Context<Self>) {
        if papers.is_empty() { return }
        if self.move_targets().is_empty() { return self.fail(t(if copy { "还没有别的文献库可以复制过去。" } else { "还没有别的文献库可以移过去。" }), cx) }
        self.modal_error.clear();
        self.modal = Some(Modal::MoveTo { papers, to: None, copy, held: vec![] });
        cx.notify();
    }

    /// A library chosen in the dialog, and which of the papers it holds already, so that the dialog asks what becomes
    /// of them before anything is taken. One in its trash is said at once, as taking it would say it.
    pub(crate) fn choose_move_target(&mut self, id: String) {
        let Some(Modal::MoveTo { papers, copy, .. }) = &self.modal else { return };
        let found = match (self.libraries.iter().find(|info| info.id == id), self.lib.as_ref()) {
            (Some(target), Some(lib)) => Store::open(&target.dir).and_then(|store| store.held_papers(&lib.store, papers, *copy)),
            _ => Ok(vec![]),
        };
        self.modal_error.clear();
        let found = found.unwrap_or_else(|err| {
            self.modal_error = crate::i18n::phrase(&format!("{err:#}"));
            vec![]
        });
        if let Some(Modal::MoveTo { to, held, .. }) = self.modal.as_mut() { (*to, *held) = (Some(id), found) }
    }

    /// Enter with no library chosen: the only one offered is chosen (a second Enter moves), else the dialog says what it waits for.
    pub(crate) fn choose_only_move_target(&mut self, cx: &mut Context<Self>) {
        let mut offered = self.move_targets();
        match (offered.pop(), offered.is_empty()) {
            (Some(only), true) => self.choose_move_target(only.id),
            _ => self.modal_error = t("选一个文献库。").into(),
        }
        cx.notify();
    }

    /// Arrow keys and Enter in `Modal::MoveTo` walk the libraries offered.
    pub(crate) fn move_nav(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let targets = self.move_targets();
        let Some(Modal::MoveTo { to, .. }) = self.modal.as_ref() else { return false };
        if targets.is_empty() { return true }
        let at = to.as_deref().and_then(|id| targets.iter().position(|info| info.id == id));
        let last = targets.len() - 1;
        let next = match nav {
            Nav::Down => at.map_or(0, |at| (at + 1).min(last)),
            Nav::Up => at.map_or(last, |at| at.saturating_sub(1)),
            Nav::First => 0,
            Nav::Last => last,
            Nav::Open => {
                self.run(Cmd::SubmitModal, window, cx);
                return true;
            }
            _ => return true,
        };
        if at != Some(next) { self.choose_move_target(targets[next].id.clone()) }
        cx.notify();
        true
    }

    pub(crate) fn move_papers(&mut self, papers: Vec<String>, to: String, held: Held, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.libraries.iter().find(|info| info.id == to).cloned() else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.answering() { return self.modal_error = t("这次回答结束后再移。").into() }
        // Where their PDFs were read to is written first, so that it goes along.
        lib.keep_places(cx);
        let single = (papers.len() == 1).then(|| papers[0].as_str());
        let (reading, tab) = (single.is_some_and(|paper| lib.reading && lib.selected.as_deref() == Some(paper)), lib.tab);
        let taken = Store::open(&target.dir).and_then(|mut store| store.take_papers(&mut lib.store, &papers, held));
        let taken = match taken {
            Ok(taken) if !taken.is_empty() => taken,
            Ok(_) => return,
            Err(err) => return self.modal_error = crate::i18n::phrase(&format!("{err:#}")),
        };
        self.modal = None;
        let mut fields_left: Vec<String> = vec![];
        for field in taken.iter().flat_map(|t| &t.fields_left) {
            if !fields_left.contains(field) { fields_left.push(field.clone()) }
        }
        let from = lib.info.clone();
        // A conversation that held only these papers, where nothing was asked, has nothing left to go back to.
        let empty = from.draft && lib.store.list_papers().is_ok_and(|papers| papers.is_empty()) && lib.chat(Talk::Library).messages.is_empty() && lib.store.list_conversations("").is_ok_and(|kept| kept.iter().all(|c| c.questions == 0));
        if taken.len() == 1 || empty {
            self.close_library(cx);
            if empty && let Err(err) = libraries::remove(&self.root, &from) { self.fail(format!("{err:#}"), cx) }
            self.libraries = libraries::list(&self.root);
            self.open_library(&to, window, cx);
            if let [only] = taken.as_slice() {
                if self.lib.as_ref().is_none_or(|lib| lib.info.id != to || lib.paper(&only.paper).is_none()) { return }
                self.select(Some(only.paper.clone()), window, cx);
                self.run(Cmd::SetTab(tab), window, cx);
                if let Some(lib) = self.lib.as_mut() { lib.reading = reading }
                let mut said = if only.joined { joined_one(&target.name, held) } else { tf!("已移到「{}」。", "Moved to “{}”.", target.name) };
                if !empty { said += &tf!("原来的那条在「{}」的回收站里。", " The original is in the trash of “{}”.", from.name) }
                if !fields_left.is_empty() { said += &tf!("这里没有词条「{}」，它的内容没有带过来。", " This library has no field “{}”; its values were not brought.", fields_left.join("」「")) }
                return self.say(said, cx);
            }
        } else {
            if let Some(parked) = self.parked.iter_mut().find(|lib| lib.info.id == to) { let _ = parked.reload(); }
            self.change(cx, |_| Ok(()));
            self.libraries = libraries::list(&self.root);
            window.focus(&self.focus);
        }
        let count = taken.len();
        let joined = taken.iter().filter(|t| t.joined).count();
        let mut said = if joined == 0 {
            tn!("已把 {n} 篇文献移到「{}」。", "Moved {n} to “{}”.", count, "paper", target.name)
        } else if joined == count {
            joined_all(&target.name, count, held)
        } else if held == Held::Overwrite {
            tf!("已把 {} 篇文献移到「{}」，其中 {} 篇覆盖了已有的条目。", "Moved {} papers to “{}” ({} overwrote existing entries).", count, target.name, joined)
        } else {
            tf!("已把 {} 篇文献移到「{}」，其中 {} 篇并到了已有的条目上。", "Moved {} papers to “{}” ({} merged with existing entries).", count, target.name, joined)
        };
        if !empty {
            said += &if self.lib.as_ref().is_some_and(|lib| lib.info.id == from.id) {
                tf!("原来的条目在本库的回收站里。", " The originals are in this library's trash.")
            } else {
                tf!("原来的条目在「{}」的回收站里。", " The originals are in the trash of “{}”.", from.name)
            };
        }
        if !fields_left.is_empty() {
            said += &tf!("「{}」没有词条「{}」，内容没有带过来。", " “{}” has no field “{}”; its values were not brought.", target.name, fields_left.join("」「"));
        }
        if self.lib.as_ref().is_some_and(|lib| lib.info.id == from.id) {
            self.say_with(said, t("打开"), Cmd::OpenLibrary(to), cx);
        } else {
            self.say(said, cx);
        }
    }

    /// 复制到文献库: the papers copied into `to`. Nothing here changes, so the window stays where it is, and the toast
    /// opens the other library.
    pub(crate) fn copy_papers(&mut self, papers: Vec<String>, to: String, held: Held, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.libraries.iter().find(|info| info.id == to).cloned() else { return };
        let Some(lib) = self.lib.as_mut() else { return };
        // Where their PDFs were read to is written first, so that it goes along.
        lib.keep_places(cx);
        let taken = Store::open(&target.dir).and_then(|mut store| store.copy_papers(&lib.store, &papers, held));
        let taken = match taken {
            Ok(taken) if !taken.is_empty() => taken,
            Ok(_) => return,
            Err(err) => return self.modal_error = crate::i18n::phrase(&format!("{err:#}")),
        };
        self.modal = None;
        // A library at work off screen reads what came in, as for a move.
        if let Some(parked) = self.parked.iter_mut().find(|lib| lib.info.id == to) { let _ = parked.reload(); }
        self.libraries = libraries::list(&self.root);
        window.focus(&self.focus);
        let mut fields_left: Vec<String> = vec![];
        for field in taken.iter().flat_map(|t| &t.fields_left) {
            if !fields_left.contains(field) { fields_left.push(field.clone()) }
        }
        let count = taken.len();
        let joined = taken.iter().filter(|t| t.joined).count();
        let mut said = if joined == count {
            if count == 1 { joined_one(&target.name, held) } else { joined_all(&target.name, count, held) }
        } else if joined == 0 {
            if count == 1 { tf!("已复制到「{}」。", "Copied to “{}”.", target.name) } else { tn!("已把 {n} 篇文献复制到「{}」。", "Copied {n} to “{}”.", count, "paper", target.name) }
        } else if held == Held::Overwrite {
            tf!("已把 {} 篇文献复制到「{}」，其中 {} 篇覆盖了已有的条目。", "Copied {} papers to “{}” ({} overwrote existing entries).", count, target.name, joined)
        } else {
            tf!("已把 {} 篇文献复制到「{}」，其中 {} 篇并到了已有的条目上。", "Copied {} papers to “{}” ({} merged with existing entries).", count, target.name, joined)
        };
        if !fields_left.is_empty() {
            said += &tf!("「{}」没有词条「{}」，内容没有带过来。", " “{}” has no field “{}”; its values were not brought.", target.name, fields_left.join("」「"));
        }
        self.say_with(said, t("打开"), Cmd::OpenLibrary(to), cx);
    }

    pub(crate) fn render_move_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Modal::MoveTo { papers, to, copy, held }) = &self.modal else { return None };
        let titles: Vec<&str> = papers.iter().filter_map(|id| self.lib.as_ref()?.paper(id)).map(|p| p.title.as_str()).collect();
        let what = match titles.as_slice() {
            [one] => one.to_string(),
            [first, ..] => tn!("「{}」等 {n} 篇文献", "“{}” ({n} in all)", titles.len(), "paper", brief(first)),
            [] => String::new(),
        };
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
        let note = match (papers.len() > 1, draft, *copy) {
            (false, true, false) => t("PDF、标注、读到的位置和关于它的对话都一起过去；这段对话里不再有它。"),
            (false, false, false) => t("PDF、标注、读到的位置和关于它的对话都一起过去，收录状态回到待判断；原来的那条留在本库的回收站里。"),
            (true, true, false) => t("PDF、标注、读到的位置和各自的对话都一起过去；这段对话里不再有它们。"),
            (true, false, false) => t("PDF、标注、读到的位置和各自的对话都一起过去，收录状态回到待判断；原来的条目留在本库的回收站里。"),
            (false, true, true) => t("PDF、标注、读到的位置和关于它的对话都复制一份过去；这段对话里仍然有它。"),
            (false, false, true) => t("PDF、标注、读到的位置和关于它的对话都复制一份过去，在那边收录状态是待判断；这里的这条不变。"),
            (true, true, true) => t("PDF、标注、读到的位置和各自的对话都复制一份过去；这段对话里仍然有它们。"),
            (true, false, true) => t("PDF、标注、读到的位置和各自的对话都复制一份过去，在那边收录状态是待判断；这里的条目不变。"),
        };
        // Papers the library chosen holds already: named, and what 追加 and 覆盖 do with them.
        let target = to.as_deref().and_then(|id| self.libraries.iter().find(|info| info.id == id));
        let held_note = target.filter(|_| !held.is_empty()).map(|target| {
            let english = crate::i18n::is_english();
            let named: Vec<String> = held.iter().filter_map(|id| self.lib.as_ref()?.paper(id)).map(|p| if english { format!("“{}”", brief(&p.title)) } else { format!("「{}」", brief(&p.title)) }).collect();
            let and = if english { ", " } else { "、" };
            let which = if papers.len() == 1 {
                tf!("「{}」里已有这篇。", "“{}” already has this paper.", target.name)
            } else if named.len() <= 3 {
                tf!("「{}」里已有其中 {} 篇：{}。", "“{}” already has {} of them: {}.", target.name, held.len(), named.join(and))
            } else {
                tf!("「{}」里已有其中 {} 篇：{}等。", "“{}” already has {} of them: {} and others.", target.name, held.len(), named[..3].join(and))
            };
            div().flex().flex_col().gap(px(4.)).text_size(text_px(12.)).line_height(text_px(18.))
                .child(div().text_color(Theme::warning()).font_weight(FontWeight::MEDIUM).child(which))
                .child(div().text_color(Theme::text_secondary()).child(t("追加：那边已有的都保留，这边的标签、笔记、PDF 和标注加上去，空着的信息补上。")))
                .child(div().text_color(Theme::text_secondary()).child(t("覆盖：两边都有的以这边为准，包括题目等信息、笔记、标签、阅读状态和标注；那边独有的保留，收录状态不变。")))
        });
        let append = to.clone().filter(|_| !held.is_empty()).map(|to| if *copy { Cmd::CopyPapers(papers.clone(), to, Held::Append) } else { Cmd::MovePapers(papers.clone(), to, Held::Append) });
        Some(
            ui::dialog(460.)
                .child(ui::dialog_title(t(if *copy { "复制到文献库" } else { "移到文献库" })))
                .child(
                    div().id("modal-body").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().gap(px(10.)).px(px(20.)).py(px(8.))
                        .child(div().font_weight(FontWeight::MEDIUM).line_height(text_px(21.)).child(what))
                        .child(div().text_size(text_px(12.)).line_height(text_px(18.)).text_color(Theme::text_tertiary()).child(note))
                        .child(div().flex().flex_col().gap(px(2.)).children(rows))
                        .when_some(held_note, |d, note| d.child(note))
                        .child(ui::error_line(&self.modal_error)),
                )
                .child(
                    div().flex().flex_none().justify_end().gap(px(8.)).px(px(20.)).py(px(14.))
                        .child(ui::secondary_button("modal-cancel", t("取消")).on_click(self.on(Cmd::CloseModal, cx)))
                        .when_some(append, |d, cmd| d.child(ui::secondary_button("modal-append", t("追加")).on_click(self.on(cmd, cx))))
                        .child(ui::primary_button("modal-ok", t(if !held.is_empty() { "覆盖" } else if *copy { "复制过去" } else { "移过去" }).to_string()).when(to.is_none(), |d| d.opacity(0.4)).when(to.is_some(), |d| d.on_click(self.on(Cmd::SubmitModal, cx)))),
                ),
        )
    }
}
