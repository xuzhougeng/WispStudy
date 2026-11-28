//! The same request put to several models at once, each answer a version of its own: the comparison 译成… and
//! 换个说法 show when the person chose two or more models for it (`Settings::compare_models`, the menu over the
//! card). Each model streams into its own row and ends by itself: one that is slow or fails holds no other, and
//! its row says why. Every row belongs to one asking (`Comparison::generation`): an answer to an earlier one is
//! dropped.
//!
//! Nothing here knows the editor. `Workspace::compare_ask` takes the requests (made the same for every model, so
//! the comparison is fair), the library they are for, and two functions: where the comparison they write into is
//! now (`locate`, `None` once it is gone) and what to do after each change (`settled`). The card of write_assist.rs
//! keeps one; a mode that translates paragraph by paragraph can keep one a paragraph.

use crate::app::Workspace;
use gpui::*;
use wisp_core::ai::{self, ChatEvent, HttpRequest, Profile, Protocol};
use wisp_core::versions;

/// One model's version.
pub struct Row {
    /// The model's name as a card says it (`write_assist::model_name`).
    pub model: String,
    pub result: String,
    pub busy: bool,
    pub error: String,
    /// That it is being asked again, or why it ended early.
    pub note: String,
}

/// The versions of one asking, a row a model, in the order the models were chosen.
pub struct Comparison {
    pub rows: Vec<Row>,
    pub generation: u64,
}

impl Comparison {
    /// One row a model, each waiting for its answer.
    pub fn new(profiles: &[Profile], generation: u64) -> Comparison {
        Comparison { rows: profiles.iter().map(|profile| Row { model: crate::write_assist::model_name(profile), result: String::new(), busy: true, error: String::new(), note: String::new() }).collect(), generation }
    }

    /// Whether some model is still answering.
    pub fn busy(&self) -> bool {
        self.rows.iter().any(|row| row.busy)
    }

    /// The version a row came to: its words, without whatever a model put around them (a heading, quotation marks).
    pub fn version(&self, n: usize) -> Option<String> {
        let row = self.rows.get(n)?;
        let text = versions::parse(&row.result).into_iter().next().map(|version| version.text).unwrap_or_default();
        (!text.trim().is_empty()).then_some(text)
    }

    /// Stop every row, keeping what came.
    pub fn stop(&mut self) {
        for row in self.rows.iter_mut().filter(|row| row.busy) { row.busy = false }
    }
}

/// Where a comparison is now: `None` once it was dropped or another took its place.
pub type Locate = fn(&mut Workspace) -> Option<&mut Comparison>;

impl Workspace {
    /// Put each request to its model at once (`requests` in the comparison's row order; a request that could not be
    /// made fills its row's error), each answer streaming into its row while the comparison `locate` finds is still
    /// of `generation` and the library `library` is open. `settled` runs after every change.
    pub(crate) fn compare_ask(&mut self, library: String, generation: u64, requests: Vec<(Result<HttpRequest, String>, Protocol)>, locate: Locate, settled: fn(&mut Workspace), cx: &mut Context<Self>) {
        for (n, (request, protocol)) in requests.into_iter().enumerate() {
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    if let Some(row) = locate(self).filter(|comparison| comparison.generation == generation).and_then(|comparison| comparison.rows.get_mut(n)) { (row.busy, row.error) = (false, error) }
                    continue;
                }
            };
            let (sender, events) = async_channel::unbounded();
            cx.background_executor().spawn(async move { wisp_net::stream_chat(request, protocol, sender) }).detach();
            let library = library.clone();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    let alive = this.update(cx, |this, cx| {
                        if this.lib.as_ref().is_none_or(|lib| lib.info.id != library) { return false }
                        let Some(row) = locate(this).filter(|comparison| comparison.generation == generation).and_then(|comparison| comparison.rows.get_mut(n)) else { return false };
                        if !row.busy { return false }
                        let more = match event {
                            ChatEvent::Text(text) => {
                                row.note.clear();
                                row.result.push_str(&text);
                                true
                            }
                            ChatEvent::Retrying(attempt, why) => {
                                row.note = tf!("没有回应（{why}），正在重试，第 {attempt} 次…", "No response ({why}). Retrying, attempt {attempt}…");
                                true
                            }
                            ChatEvent::Used(_) => true,
                            ChatEvent::Error(error) => {
                                (row.busy, row.error) = (false, error);
                                row.note.clear();
                                false
                            }
                            ChatEvent::Done(turn) => {
                                row.busy = false;
                                row.note = ai::stop_note(&turn.stop).unwrap_or_default().to_string();
                                if row.result.trim().is_empty() && row.error.is_empty() { row.error = crate::i18n::t("模型没有返回内容。").into() }
                                false
                            }
                        };
                        settled(this);
                        cx.notify();
                        more
                    });
                    if !matches!(alive, Ok(true)) { break }
                }
            })
            .detach();
        }
        settled(self);
        cx.notify();
    }
}
