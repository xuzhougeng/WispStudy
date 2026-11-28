//! 筛选 at work: the state of the page (`Screen`), the queue the assistant reads through, and its run
//! (`screen_start`, `screen_next`): a few papers at a time, each answer a suggestion the person decides on.
//! The three faces of the page are `screen.rs`, `screen_single.rs` and `screen_confirm.rs`.

use crate::app::{Library, Step, Workspace};
use crate::i18n::t;
use crate::input::TextInput;
use gpui::*;
use wisp_core::ai;
use wisp_core::model::*;
use wisp_core::screening::{Confirming, Round};
use wisp_core::settings::Task;
use wisp_core::store::Suggestion;

/// The screening page: what to screen against, and the assistant's run
/// over the papers still waiting for a decision.
pub struct Screen {
    pub criteria: Entity<TextInput>,
    /// Papers the assistant has yet to be asked about in this run, last first.
    pub(crate) todo: Vec<String>,
    /// The papers being read right now, in the order they were started.
    pub reading: Vec<String>,
    pub total: usize,
    pub done: usize,
    /// Papers of the run the assistant gave no usable answer for.
    pub failed: usize,
    /// Failures since the last answer that could be used.
    streak: usize,
    /// The model the run was started with.
    pub model: String,
    /// Which part of the queue is on show: a suggested verdict, `none` for the papers without a suggestion, empty for all.
    pub filter: String,
    pub error: String,
    /// Bumped when a run starts or stops, so answers to an earlier one are dropped.
    pub(crate) run: u64,
    /// How the queue is worked through.
    pub mode: ScreenMode,
    /// 逐篇: the papers of this round and the one on show (`screen_single.rs`).
    pub round: Round,
    /// 确认: what the person changed in the list of suggestions (`screen_confirm.rs`).
    pub confirm: Confirming,
    /// What the last confirmation wrote, until the page is left: shown in place of the list, and how it is taken back.
    pub confirmed: Option<Confirmed>,
    /// The write of 筛选记录 that decisions go into: this round's of 逐篇, or this visit's to the queue (`screen_record.rs`).
    pub open: Option<i64>,
    /// What this visit to the page wrote, the last last: what Z and ⌘Z take back.
    pub steps: Vec<Step>,
    pub rail_scroll: UniformListScrollHandle,
    pub confirm_scroll: UniformListScrollHandle,
    pub paper_scroll: ScrollHandle,
}

/// The three faces of 筛选.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenMode {
    /// The queue as a list, with the assistant's run over it.
    #[default]
    Batch,
    /// One paper at a time, decided with a key.
    Single,
    /// The assistant's suggestions looked over and written together.
    Confirm,
}

/// What a confirmation wrote.
pub struct Confirmed {
    /// Its write in 筛选记录, which is how it is taken back.
    pub write: i64,
    pub papers: usize,
    /// How many papers got each status, in the order of `SCREENINGS`.
    pub counts: [usize; 5],
}

impl Screen {
    pub(crate) fn new(criteria: &str, cx: &mut App) -> Screen {
        let criteria = cx.new(|cx| TextInput::new("例如：只收以人类样本做单细胞测序的原始研究；综述、方法学文章不收。留空则按文献库的主题判断。", cx).multiline(3).with_text(criteria));
        Screen { criteria, todo: vec![], reading: vec![], total: 0, done: 0, failed: 0, streak: 0, model: String::new(), filter: String::new(), error: String::new(), run: 0, mode: ScreenMode::Batch, round: Round::default(), confirm: Confirming::default(), confirmed: None, open: None, steps: vec![], rail_scroll: UniformListScrollHandle::new(), confirm_scroll: UniformListScrollHandle::new(), paper_scroll: ScrollHandle::new() }
    }

    /// Whether the assistant is working through the papers right now.
    pub fn running(&self) -> bool {
        !self.reading.is_empty() || !self.todo.is_empty()
    }
}

/// A run of the screening assistant gives up after this many papers in a
/// row without a usable answer: a wrong key or a service that is down
/// would fail every paper the same way.
pub(crate) const SCREEN_GIVE_UP: usize = 3;

impl Library {
    /// The suggestion that counts for a paper: one made against the
    /// screening goal as it is now. An older one is shown, but not acted on.
    pub fn current_suggestion(&self, paper_id: &str) -> Option<&Suggestion> {
        self.suggestions.get(paper_id).filter(|s| s.made_for(&self.info.criteria))
    }

    /// Whether a decided paper is back for another look: asked again
    /// against the goal as it is now, the assistant suggested otherwise,
    /// and the person has not looked at it since.
    fn rechecking(&self, paper: &Paper) -> bool {
        paper.screening != "pending" && self.current_suggestion(&paper.id).is_some_and(|s| s.verdict != paper.screening && s.created_at > paper.reviewed_at)
    }

    /// The papers waiting for a decision, and the decided ones back for
    /// another look: the suggested ones first, by verdict and then by how
    /// sure the assistant was, then those it disagreed with the person on,
    /// those suggested for an earlier goal, and the ones without a suggestion.
    pub fn screen_queue(&self) -> Vec<&Paper> {
        let rank = |p: &Paper| match self.screen_group(&p.id) {
            "differs" => (20, -self.suggestions[&p.id].confidence.unwrap_or(0.)),
            "stale" => (50, 0.),
            "none" => (99, 0.),
            verdict => (SCREENINGS.iter().position(|(key, _)| *key == verdict).unwrap_or(9), -self.suggestions[&p.id].confidence.unwrap_or(0.)),
        };
        let mut queue: Vec<&Paper> = self.papers.iter().filter(|p| p.screening == "pending" || self.rechecking(p)).collect();
        // Stable: within a group the library's own order holds.
        queue.sort_by(|a, b| rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal));
        queue
    }

    /// Which part of the queue a paper belongs to: its suggested verdict,
    /// `differs` for a decided paper the assistant now sees otherwise,
    /// `stale` for a suggestion made against an earlier goal, or `none`.
    pub fn screen_group(&self, paper_id: &str) -> &str {
        let Some(s) = self.suggestions.get(paper_id) else { return "none" };
        if !s.made_for(&self.info.criteria) { return "stale" }
        if self.paper(paper_id).is_some_and(|p| p.screening != "pending") { return "differs" }
        s.verdict.as_str()
    }

    /// The papers the assistant is still to be asked about against the
    /// goal as it is now: those waiting for a decision, or, with `decided`,
    /// those the person has decided.
    pub fn screen_unasked(&self, decided: bool) -> Vec<&Paper> {
        self.papers.iter().filter(|p| (p.screening != "pending") == decided && self.current_suggestion(&p.id).is_none()).collect()
    }

    /// The decided papers that are not included (相关, 证据不足, 不相关):
    /// what 把未纳入的移到回收站 takes.
    pub fn unincluded(&self) -> Vec<String> {
        self.papers.iter().filter(|p| !matches!(p.screening.as_str(), "pending" | "included")).map(|p| p.id.clone()).collect()
    }

    /// The part of the queue on show, and its filter. A part that has
    /// emptied (the last of its papers was decided) gives way to the whole queue.
    pub fn screen_view(&self) -> (Vec<&Paper>, &str) {
        let queue = self.screen_queue();
        let filter = self.screen.filter.as_str();
        if filter.is_empty() || !queue.iter().any(|p| self.screen_group(&p.id) == filter) { return (queue, "") }
        (queue.into_iter().filter(|p| self.screen_group(&p.id) == filter).collect(), filter)
    }
}

impl Workspace {
    /// Start asking the assistant about the pending papers it has not judged
    /// against the goal as it is now (with `decided`, the decided ones),
    /// several at a time (设置 › AI says how many, and with which model).
    /// What it says is saved as a suggestion; the decision stays with the
    /// person, and a decided paper it sees otherwise comes back to the queue.
    /// With `only`, it is asked about those papers, whatever it said of them before.
    /// `scope` is which papers a run over the queue takes: `pending`, the
    /// `included` ones (继续筛) or all `decided` ones; such a run begins a
    /// round of the funnel when the goal is new (`Store::begin_round`).
    pub(crate) fn screen_start(&mut self, scope: &str, only: Option<Vec<String>>, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Screen);
        let workers = self.settings.screen_workers;
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.screen.running() { return }
        lib.screen.error.clear();
        // The key and address are checked once here, not once per paper.
        if let Err(err) = if profile.judges() { wisp_core::judge::ready(&profile) } else { ai::plain_request(&profile, "s", "p").map(|_| ()) } {
            lib.screen.error = err;
            return cx.notify();
        }
        let whole = only.is_none();
        let mut todo: Vec<String> = only.unwrap_or_else(|| lib.screen_unasked(scope != "pending").iter().filter(|p| scope != "included" || p.screening == "included").map(|p| p.id.clone()).collect());
        if todo.is_empty() {
            lib.screen.error = match scope {
                "pending" => t("待判断的文献都已经有按当前目标的建议了。"),
                "included" => t("已纳入的文献都有按当前目标的建议了。"),
                _ => t("已判断的文献都有按当前目标的建议了。"),
            }.into();
            return cx.notify();
        }
        // A run over the queue is part of the funnel; asking about one paper is not.
        if whole && lib.store.begin_round(&lib.info.criteria, scope).is_ok() && let Ok(rounds) = lib.store.screening_rounds() { lib.rounds = rounds }
        todo.reverse();
        let screen = &mut lib.screen;
        screen.run += 1;
        (screen.total, screen.done, screen.failed, screen.streak, screen.todo) = (todo.len(), 0, 0, 0, todo);
        screen.reading.clear();
        screen.model = profile.labelled();
        let run = screen.run;
        for _ in 0..workers {
            self.screen_next(run, cx);
        }
        cx.notify();
    }

    /// Ask about the next paper of the run, and about the one after it once
    /// the answer is in. A paper without a usable answer is counted and
    /// left waiting; several of them in a row end the run.
    fn screen_next(&mut self, run: u64, cx: &mut Context<Self>) {
        let profile = self.settings.profile_for_or_default(Task::Screen);
        let Some(lib) = self.lib.as_mut().filter(|l| l.screen.run == run) else { return };
        let Some(id) = lib.screen.todo.pop() else { return };
        // A paper that left the library meanwhile is done with, so the run still ends at its total.
        let Some(paper) = lib.paper(&id).cloned() else {
            lib.screen.done += 1;
            return self.screen_next(run, cx);
        };
        let pages = lib.store.paper_pages(&id).ok().flatten();
        let (system, prompt, basis) = ai::screen_prompt(&lib.info.topic, &lib.info.criteria, &paper, pages.as_deref());
        let author = profile.labelled();
        // What it is judged against, kept with the answer: the goal may change while it is read.
        let criteria = Some(lib.info.criteria.clone());
        // Nothing to read: say so without asking, rather than have the model guess from a title.
        if basis == "none" {
            let mut suggestion = Suggestion { paper_id: id, verdict: "uncertain".into(), reason: t("这篇文献没有摘要，也没有保存全文，无法判断。").into(), basis: basis.into(), author, criteria, ..Default::default() };
            let saved = lib.store.save_suggestion(&mut suggestion);
            lib.screen.done += 1;
            if saved.is_ok() { lib.suggestions.insert(suggestion.paper_id.clone(), suggestion); }
            return self.screen_next(run, cx);
        }
        // A model that judges is asked its own way, about the abstract (the first pages without one): it answers
        // with a verdict and how sure it is, and no reason. Any other model is asked in words and reads the full text.
        let judge = profile.judges().then(|| wisp_core::judge::screen_request(&profile, &lib.info.topic, &lib.info.criteria, &paper, pages.as_deref()));
        let (request, basis) = match judge {
            Some(Ok((request, basis))) => (Ok(request), basis),
            Some(Err(err)) => (Err(err), basis),
            None => (ai::plain_request(&profile, &system, &prompt), basis),
        };
        let request = match request {
            Ok(request) => request,
            Err(err) => {
                lib.screen.error = err;
                lib.screen.todo.clear();
                return;
            }
        };
        lib.screen.reading.push(id);
        let (library, protocol, judges) = (lib.info.id.clone(), profile.protocol, profile.judges());
        let work = cx.background_executor().spawn(async move {
            let judged = if judges { wisp_net::judge(request).and_then(|reply| wisp_core::judge::read_screening(&reply)) } else { wisp_net::complete(request, protocol).and_then(|reply| ai::parse_judgement(&reply, &paper, pages.as_deref())) };
            (paper, judged)
        });
        cx.spawn(async move |this, cx| {
            let (paper, judged) = work.await;
            // The library on show, or one left while this run went on (`in_library`).
            this.update(cx, |this, cx| this.in_library(&library, cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|l| l.screen.run == run) else { return };
                lib.screen.reading.retain(|reading| *reading != paper.id);
                lib.screen.done += 1;
                let saved = judged.and_then(|j| {
                    let mut suggestion = Suggestion { paper_id: paper.id.clone(), verdict: j.verdict, reason: j.reason, excerpt: j.excerpt, page: j.page, basis: basis.into(), confidence: j.confidence, author, criteria, ..Default::default() };
                    lib.store.save_suggestion(&mut suggestion).map(|()| suggestion).map_err(|e| format!("{e:#}"))
                });
                match saved {
                    Ok(suggestion) => {
                        lib.screen.streak = 0;
                        lib.suggestions.insert(suggestion.paper_id.clone(), suggestion);
                        this.screen_next(run, cx);
                    }
                    Err(err) => {
                        lib.screen.failed += 1;
                        lib.screen.streak += 1;
                        if lib.screen.streak >= SCREEN_GIVE_UP {
                            lib.screen.error = tf!("连续 {SCREEN_GIVE_UP} 篇没有得到可用的回答，已停止。最后一篇「{}」：{err}", "Stopped after {SCREEN_GIVE_UP} consecutive unusable responses. Last paper “{}”: {err}", paper.title);
                            lib.screen.todo.clear();
                        } else {
                            lib.screen.error = tf!("「{}」没有判断成，留待下一轮：{err}", "Could not screen “{}”; left for the next round: {err}", paper.title);
                            this.screen_next(run, cx);
                        }
                    }
                }
                cx.notify();
            }))
            .ok();
        })
        .detach();
    }
}
