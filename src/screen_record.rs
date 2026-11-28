//! 筛选记录 in the window: every decision and every confirmation made on
//! 筛选 is written with its record (`wisp_core::screening_record`), so it can
//! be taken back later, not only while the page is open. The last step of
//! this visit to the page goes back with Z or ⌘Z / Ctrl+Z (`screen_undo`);
//! any write, older ones too, from the record's menu under the goal
//! (`MenuKind::ScreenRecord`), after asking. Either way a paper decided
//! otherwise since keeps that decision.

use crate::app::{Cmd, MenuItem, MenuKind, Modal, Page, ScreenMode, Workspace};
use crate::i18n::{label, t};
use crate::screen_confirm::breakdown;
use crate::theme::text_px;
use crate::ui;
use gpui::*;
use wisp_core::model::{Paper, SCREENINGS};
use wisp_core::screening::Before;
use wisp_core::screening_record::{Decision, KINDS, ScreeningWrite};
use wisp_core::time;

/// The writes the record's menu lists, the latest first; it scrolls past sixteen.
const LISTED: usize = 30;

/// A step of this visit to 筛选 that Z and ⌘Z take back, the last one first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A decision, gone into this write: taking it back takes the write's last decision back.
    Decision(i64),
    /// A confirmation, taken back whole.
    Confirmation(i64),
}

impl Step {
    pub fn write(self) -> i64 {
        match self {
            Step::Decision(id) | Step::Confirmation(id) => id,
        }
    }
}

/// A decision on a paper as it is now.
pub(crate) fn decision(paper: &Paper, status: &str, reason: &str) -> Decision {
    Decision { before: Before::of(paper), status: status.to_string(), reason: reason.to_string(), reviewed_at: if status == "pending" { String::new() } else { time::now_iso() } }
}

/// The name of the face of 筛选 a write was made on.
fn kind_name(kind: &str) -> &str {
    label(&KINDS, kind)
}

/// A write as one line: when, on which face, and what it made of its papers.
fn described(write: &ScreeningWrite) -> String {
    format!("{} · {} · {}", time::short_time(&write.made_at), kind_name(&write.kind), breakdown(&write.counts))
}

impl Workspace {
    /// Write a decision made on 筛选, with its record: onto the write of this round of 逐篇, or of this visit to the queue.
    pub(crate) fn record_decision(&mut self, decision: Decision, cx: &mut Context<Self>) -> bool {
        let Some(lib) = self.lib.as_ref() else { return false };
        let kind = if lib.screen.mode == ScreenMode::Single { "round" } else { "queue" };
        let (open, criteria) = (lib.screen.open, lib.info.criteria.clone());
        let mut written = None;
        if !self.change(cx, |lib| {
            written = Some(lib.store.record_screenings(open, kind, &criteria, &[decision])?);
            Ok(())
        }) { return false }
        if let (Some(lib), Some(id)) = (self.lib.as_mut(), written) {
            lib.screen.open = Some(id);
            lib.screen.steps.push(Step::Decision(id));
        }
        true
    }

    /// Write what 确认 confirms, as a write of its own. The write, or none when nothing was written.
    pub(crate) fn record_confirmation(&mut self, decisions: Vec<Decision>, cx: &mut Context<Self>) -> Option<i64> {
        let criteria = self.lib.as_ref()?.info.criteria.clone();
        let mut written = None;
        if !self.change(cx, |lib| {
            written = Some(lib.store.record_screenings(None, "confirm", &criteria, &decisions)?);
            Ok(())
        }) { return None }
        let id = written?;
        self.lib.as_mut()?.screen.steps.push(Step::Confirmation(id));
        Some(id)
    }

    /// Z, ⌘Z / Ctrl+Z and 撤销上一个判断: the last step of this visit to 筛选
    /// taken back. A decision shows its paper again; a confirmation goes back whole.
    pub(crate) fn screen_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(step) = lib.screen.steps.pop() else { return self.say(t("这次在筛选页上还没有可撤销的判断；更早的筛选在「筛选记录」里撤销。"), cx) };
        let id = match step {
            Step::Confirmation(id) => return self.undo_screening(id, cx),
            Step::Decision(id) => id,
        };
        let mut taken = None;
        if !self.change(cx, |lib| {
            taken = lib.store.take_back_screening(id)?;
            Ok(())
        }) { return }
        // Nothing of it left to take back: the step before is the one meant.
        let Some((decision, back)) = taken else { return self.screen_undo(window, cx) };
        let id = decision.paper_id().to_string();
        if back { self.say(tf!("已撤销：回到「{}」。", "Undone: back to “{}”.", label(&SCREENINGS, &decision.before.screening)), cx) } else { self.say(t("这一篇之后又改过判断，保持现在的样子。"), cx) }
        let Some(lib) = self.lib.as_mut() else { return };
        match lib.screen.mode {
            ScreenMode::Single => {
                lib.screen.round.show(&id);
                self.round_moved(window, cx);
            }
            ScreenMode::Batch => self.select(Some(id), window, cx),
            ScreenMode::Confirm => {}
        }
    }

    /// ⌘Z / Ctrl+Z where no text field, PDF or mind map has the keyboard: on 筛选 what Z does; anywhere else it goes on.
    pub(crate) fn undo_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lib.as_ref().is_some_and(|lib| lib.page == Page::Screen) { self.screen_key("z", window, cx) } else { cx.propagate() }
    }

    /// Whether 撤销上一个判断 has a decision to take back: the last step of this visit is one.
    pub(crate) fn screen_can_undo(&self) -> bool {
        self.lib.as_ref().is_some_and(|lib| matches!(lib.screen.steps.last(), Some(Step::Decision(_))))
    }

    /// A write of the record taken back, all of it: from its menu after asking, or the confirmation just written.
    pub(crate) fn undo_screening(&mut self, id: i64, cx: &mut Context<Self>) {
        let mut counted = (0, 0);
        if !self.change(cx, |lib| {
            counted = lib.store.undo_screening_write(id)?;
            Ok(())
        }) { return }
        if let Some(lib) = self.lib.as_mut() {
            let screen = &mut lib.screen;
            screen.steps.retain(|step| step.write() != id);
            if screen.open == Some(id) { screen.open = None }
            if screen.confirmed.as_ref().is_some_and(|confirmed| confirmed.write == id) { screen.confirmed = None }
        }
        let (back, stayed) = counted;
        self.say(if stayed == 0 { tf!("已撤销：{back} 篇回到这次之前的判断。", "Undone: {back} papers are back to their decisions before it.") } else { tf!("已撤销：{back} 篇回到这次之前的判断；{stayed} 篇之后又改过，保持现在的样子。", "Undone: {back} papers are back to their decisions before it; {stayed} decided again since stay as they are.") }, cx);
    }

    /// The question before a write of the record is taken back: what it decided, and what taking it back leaves.
    pub(crate) fn ask_undo_screening(&mut self, id: i64, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let write = match lib.store.screening_writes() {
            Ok(writes) => writes.into_iter().find(|write| write.id == id),
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        let Some(write) = write else { return self.say(t("筛选记录里没有这一次。"), cx) };
        if write.undone() { return self.say(t("这一次已经撤销过了。"), cx) }
        if write.standing == 0 { return self.say(t("这一次判断的文献之后都又改过，撤销它不会改变什么。"), cx) }
        let (when, kind, papers) = (time::short_time(&write.made_at), kind_name(&write.kind), write.papers());
        let mut message = tf!("{when}「{kind}」判断了 {papers} 篇：{}。撤销后它们回到这次之前的判断。", "At {when}, “{kind}” decided {papers} papers: {}. Undoing it returns them to their decisions before it.", breakdown(&write.counts));
        let stayed = papers - write.standing;
        if stayed > 0 { message.push_str(&tf!("其中 {stayed} 篇之后又改过，保持现在的样子。", " {stayed} of them decided again since stay as they are.")) }
        self.modal_error.clear();
        self.modal = Some(Modal::Confirm { title: t("撤销这次筛选？").into(), message, action: t("撤销").into(), cmd: Cmd::UndoScreening(id) });
        cx.notify();
    }

    /// 筛选记录 as a menu: the latest writes, each with what became of it.
    pub(crate) fn screen_record_menu(&self) -> Vec<MenuItem> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let writes = lib.store.screening_writes().unwrap_or_default();
        writes.iter().take(LISTED).map(|write| {
            let note = if write.undone() { t("已撤销") } else if write.standing == 0 { t("之后都改过") } else { t("撤销…") };
            MenuItem { label: described(write).into(), checked: false, note: Some(note.into()), cmd: Cmd::AskUndoScreening(write.id) }
        }).collect()
    }

    /// The button that opens 筛选记录, once there is a record.
    pub(crate) fn render_record_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        self.lib.as_ref().filter(|lib| lib.screen_record)?;
        Some(ui::ghost_button("screen-record", Some("history"), Some(t("筛选记录").into())).h(text_px(22.)).text_size(text_px(12.)).on_click(self.on_menu(MenuKind::ScreenRecord, cx)))
    }
}
