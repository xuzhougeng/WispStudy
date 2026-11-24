//! 查找全文: the open full texts of the papers that have none, fetched a few at a time (`FetchRun`). It is one
//! of the runs that go on when the library is left (`works.rs`).

use crate::app::Workspace;
use crate::i18n::t;
use gpui::*;
use wisp_core::model::*;

/// What looking for the open full text of one paper came to.
pub struct FetchRow {
    pub paper_id: String,
    pub title: String,
    /// `saved`, `not_found` or `failed`, as `find_pdf` says.
    pub outcome: String,
    /// Where the file came from and what of its text; or why there is none.
    pub detail: String,
    /// The file was saved, and no text came of it.
    pub textless: bool,
}

/// A round of looking for open full texts: the papers still to ask about,
/// those being asked about, and what came of the rest, until it is put away.
#[derive(Default)]
pub struct FetchRun {
    pub total: usize,
    pub(crate) todo: Vec<String>,
    pub working: Vec<String>,
    pub rows: Vec<FetchRow>,
    /// Stop scheduling papers, but let requests already in flight report their results.
    pub stopped: bool,
    /// Bumped when a new round starts, so results from an earlier round are dropped.
    pub(crate) run: u64,
}

impl FetchRun {
    pub fn running(&self) -> bool {
        !self.todo.is_empty() || !self.working.is_empty()
    }

    pub fn count(&self, outcome: &str) -> usize {
        self.rows.iter().filter(|row| row.outcome == outcome).count()
    }

    pub fn remaining(&self) -> usize {
        self.total.saturating_sub(self.rows.len() + self.working.len())
    }

    pub fn status(&self) -> &'static str {
        match (self.stopped, self.running()) {
            (true, true) => t("正在停止"),
            (true, false) => t("已停止"),
            (false, true) => t("正在查找"),
            (false, false) => t("查找完成"),
        }
    }

    pub(crate) fn stop(&mut self) {
        self.stopped = true;
        self.todo.clear();
    }
}

/// The papers asked about at the same time.
pub(crate) const FETCH_WORKERS: usize = 2;

impl Workspace {
    /// Look for the open full text of one paper, or of the papers of the
    /// current list that have no PDF and a number to be looked up by. A
    /// round under way takes more papers in.
    pub(crate) fn fetch_start(&mut self, only: Option<String>, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        if lib.fetching.as_ref().is_some_and(|run| run.stopped && run.running()) { return self.say(t("正在结束当前请求，请稍后再开始新的查找。"), cx) }
        let known = wisp_core::openaccess::lookable;
        let lacking = |paper: &Paper| matches!(paper.fulltext.as_str(), "missing" | "linked");
        let wanted: Vec<String> = match &only {
            Some(id) => lib.paper(id).filter(|paper| lacking(paper)).map(|paper| paper.id.clone()).into_iter().collect(),
            None => lib.shown.iter().map(|&i| &lib.papers[i]).filter(|paper| lacking(paper) && known(paper)).map(|paper| paper.id.clone()).collect(),
        };
        if let Some(paper) = only.as_deref().and_then(|id| lib.paper(id)) && !known(paper) { return self.fail(t("这篇文献没有 DOI、PMID 或 arXiv 编号，无从查起。先在「编辑信息与词条」里补上编号。"), cx) }
        if wanted.is_empty() { return self.say(t("当前列表里没有可以查找的文献：要么已经有 PDF，要么没有 DOI、PMID 或 arXiv 编号。"), cx) }
        let fresh = lib.fetching.as_ref().is_none_or(|run| !run.running());
        if fresh { lib.fetching = Some(FetchRun { run: lib.fetching.as_ref().map_or(0, |run| run.run) + 1, ..Default::default() }) }
        let run = lib.fetching.as_mut().unwrap();
        let mut added = 0;
        for id in wanted.into_iter().rev() {
            if run.todo.contains(&id) || run.working.contains(&id) || run.rows.iter().any(|row| row.paper_id == id) { continue }
            run.todo.insert(0, id);
            added += 1;
        }
        run.total += added;
        let id = run.run;
        for _ in run.working.len()..FETCH_WORKERS {
            self.fetch_next(id, cx);
        }
        cx.notify();
    }

    /// Ask about the next paper of the round, and about the one after it once this is answered.
    fn fetch_next(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(run) = lib.fetching.as_mut().filter(|run| run.run == id) else { return };
        let Some(paper_id) = run.todo.pop() else { return };
        run.working.push(paper_id.clone());
        let title = lib.paper(&paper_id).map(|paper| paper.title.clone()).unwrap_or_default();
        self.library_tool_on("find_pdf", serde_json::json!({"paperId": paper_id}), true, cx, move |this, result, cx| {
            let Some(lib) = this.lib.as_mut() else { return };
            let Some(run) = lib.fetching.as_mut().filter(|run| run.run == id) else { return };
            run.working.retain(|working| *working != paper_id);
            let found = result.as_ref().ok().map(|value| value["papers"][0].clone()).unwrap_or_default();
            let outcome = found["outcome"].as_str().unwrap_or("failed").to_string();
            let textless = found["textReason"].is_string();
            let detail = match (&result, outcome.as_str()) {
                (Err(err), _) => err.clone(),
                (_, "saved") => {
                    let version = t(wisp_core::openaccess::version_label(found["version"].as_str().unwrap_or("")));
                    let from = [found["source"].as_str().unwrap_or(""), version].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ");
                    match found["textReason"].as_str() {
                        Some(reason) => tf!("来自 {from}。没读出文字：{}", "From {from}. No text extracted: {}", crate::i18n::text_failure(reason)),
                        None => tf!("来自 {from}，读出 {} 页文字", "From {from}, {} pages of text read", found["pages"].as_u64().unwrap_or(0)),
                    }
                }
                // Nothing found and nothing gone wrong: the outcome says it all.
                (_, "not_found") => found["reason"].as_str().filter(|reason| !reason.starts_with("没有找到开放获取的全文")).unwrap_or("").to_string(),
                _ => found["reason"].as_str().unwrap_or("").to_string(),
            };
            run.rows.push(FetchRow { paper_id: paper_id.clone(), title, outcome: outcome.clone(), detail, textless });
            if outcome == "saved" {
                // The paper has a PDF now: the list, the detail panel and the next question see it.
                let _ = lib.reload();
                lib.texts_changed();
                if lib.selected.as_deref() == Some(paper_id.as_str()) { lib.close_pdf() }
                // No viewer is made for a library that is not on show: it opens the file when it is come back to.
                if !this.away { this.ensure_pdf(cx) }
            }
            this.fetch_next(id, cx);
        });
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::{FetchRow, FetchRun};

    #[test]
    fn stopping_keeps_the_total_and_accepts_inflight_results() {
        let mut run = FetchRun { total: 4, todo: vec!["queued-a".into(), "queued-b".into()], working: vec!["first".into(), "second".into()], run: 7, ..Default::default() };
        run.stop();
        assert_eq!((run.total, run.run, run.remaining(), run.status()), (4, 7, 2, "正在停止"));
        assert!(run.todo.is_empty());
        assert_eq!(run.working.len(), 2);
        // An already-started request may save a PDF after Stop. Keep its result visible.
        for outcome in ["saved", "failed"] {
            let id = run.working.pop().unwrap();
            run.rows.push(FetchRow { paper_id: id, title: "Paper".into(), outcome: outcome.into(), detail: String::new(), textless: false });
        }
        assert!(!run.running());
        assert_eq!((run.status(), run.total, run.remaining()), ("已停止", 4, 2));
        assert_eq!((run.count("saved"), run.count("failed")), (1, 1));
    }
}
