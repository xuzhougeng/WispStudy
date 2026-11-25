//! Search, filters and sorting of the paper list, plus the values the filter
//! menus offer. Pure functions over `&[Paper]`.

use crate::journals;
use crate::model::Paper;
use crate::time;
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// The sidebar's views: each narrows the library before the filters apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Everything except the papers marked 不相关.
    #[default]
    All,
    /// Added within the last seven days.
    Recent,
    Unread,
    Starred,
    /// No stored PDF.
    NoPdf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub view: View,
    pub search: String,
    /// The tags chosen in the sidebar: a paper must hold every one (AND, as Zotero does).
    pub tags: Vec<String>,
    /// The tags excluded (反选): a paper holding any of them is hidden.
    pub not_tags: Vec<String>,
    /// One year (`2024`), a span of years with either end left open (`2020-2026`, `2020-`, `-2010`), or the
    /// last years up to this one (`last:3` is this year and the two before): `year_span`.
    pub year: String,
    /// The journal's impact factor as a span like the years' (`5-` is 5 and more, `5-10`, `-3`): `impact_span`.
    /// A paper whose journal the table (`journals`) has no factor for is left out.
    pub impact: String,
    /// The journal's zone: the partition (`journals::ZONES`) and the last zone kept, `cas:2` for 中科院 1–2 区,
    /// `jcr:1` for Q1 (`zone_parts`, written by `zone_value`). A paper of a journal without a zone there is left out.
    pub zone: String,
    pub screening: String,
    pub reading: String,
    pub fulltext: String,
    pub source: String,
    /// "yes" keeps preprints only, "no" leaves them out.
    pub preprint: String,
    /// Custom field id → the value a paper must hold.
    pub fields: BTreeMap<String, String>,
    /// newest, or `<key>-asc` / `<key>-desc` with key title, year, journal or authors.
    pub sort: String,
}

impl Filters {
    /// How many filters (not counting the search and the view) are set.
    pub fn active(&self) -> usize {
        [&self.year, &self.impact, &self.zone, &self.screening, &self.reading, &self.fulltext, &self.source, &self.preprint].iter().filter(|v| !v.is_empty()).count() + self.fields.len() + self.tags.len() + self.not_tags.len()
    }

    /// Whether a paper holds every chosen tag and none of the excluded ones.
    pub fn tagged(&self, p: &Paper) -> bool {
        self.tags.iter().all(|tag| p.tags.contains(tag)) && !self.not_tags.iter().any(|tag| p.tags.contains(tag))
    }

    pub fn excluded(&self, tag: &str) -> bool {
        self.not_tags.iter().any(|t| t == tag)
    }

    pub fn chosen(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// A click on a tag. An excluded tag is let go; else alone (`add` false) it chooses just this tag, or none
    /// when it was the only one chosen; with ⌘ / Ctrl (`add`) it joins the chosen tags or leaves them.
    pub fn click_tag(&mut self, tag: &str, add: bool) {
        if self.excluded(tag) {
            self.not_tags.retain(|t| t != tag);
        } else if add {
            if self.chosen(tag) { self.tags.retain(|t| t != tag) } else { self.tags.push(tag.to_string()) }
        } else if self.tags.len() == 1 && self.chosen(tag) {
            self.tags.clear();
        } else {
            self.tags = vec![tag.to_string()];
        }
    }

    /// Excludes a tag (no longer chosen), or lets an excluded one go.
    pub fn exclude_tag(&mut self, tag: &str) {
        if self.excluded(tag) {
            self.not_tags.retain(|t| t != tag);
        } else {
            self.tags.retain(|t| t != tag);
            self.not_tags.push(tag.to_string());
        }
    }

    /// A tag renamed (`Some`) or taken off every paper (`None`): the choices follow it.
    pub fn retag(&mut self, tag: &str, name: Option<&str>) {
        for list in [&mut self.tags, &mut self.not_tags] {
            let mut kept: Vec<String> = vec![];
            for old in list.drain(..) {
                let new = if old == tag { name.map(str::to_string) } else { Some(old) };
                if let Some(new) = new && !kept.contains(&new) { kept.push(new) }
            }
            *list = kept;
        }
    }

    /// The tag filter as one line: `#a #b −#c`.
    pub fn tags_said(&self) -> String {
        self.tags.iter().map(|t| format!("#{t}")).chain(self.not_tags.iter().map(|t| format!("−#{t}"))).collect::<Vec<_>>().join(" ")
    }

    pub fn clear(&mut self) {
        *self = Filters { view: self.view, sort: std::mem::take(&mut self.sort), ..Default::default() };
    }
}

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub fn in_view(p: &Paper, view: View, now_ms: i64) -> bool {
    match view {
        View::All => true,
        View::Recent => crate::time::millis(&p.added_at) >= now_ms - WEEK_MS,
        View::Unread => p.reading == "unread",
        View::Starred => p.starred,
        View::NoPdf => !matches!(p.fulltext.as_str(), "stored" | "textless"),
    }
}

/// A span of numbers as a filter keeps it: `low-high` with either end left out, or one value alone, which is both ends.
fn span<T: std::str::FromStr>(value: &str) -> Option<(Option<T>, Option<T>)> {
    let end = |text: &str| if text.trim().is_empty() { Some(None) } else { text.trim().parse().ok().map(Some) };
    match value.split_once('-') {
        Some((low, high)) => Some((end(low)?, end(high)?)).filter(|(low, high)| low.is_some() || high.is_some()),
        None => {
            let one = value.trim();
            Some((Some(one.parse().ok()?), Some(one.parse().ok()?)))
        }
    }
}

/// A span as `Filters::year` and `Filters::impact` keep it: `2020-`, `-10`, `5-10`; nothing when neither end is given.
pub fn span_value<T: std::fmt::Display>(low: Option<T>, high: Option<T>) -> String {
    if low.is_none() && high.is_none() { return String::new() }
    format!("{}-{}", low.map(|v| v.to_string()).unwrap_or_default(), high.map(|v| v.to_string()).unwrap_or_default())
}

fn within<T: PartialOrd>(value: T, (low, high): &(Option<T>, Option<T>)) -> bool {
    low.as_ref().is_none_or(|low| value >= *low) && high.as_ref().is_none_or(|high| value <= *high)
}

/// The first and the last year a `Filters::year` keeps, either left open; `None` for a value that says no years.
/// `last:N` is the N calendar years up to the one `now_ms` falls in, and whatever is dated later.
pub fn year_span(value: &str, now_ms: i64) -> Option<(Option<i32>, Option<i32>)> {
    match value.strip_prefix("last:") {
        Some(count) => count.trim().parse::<i32>().ok().filter(|n| (1..=1000).contains(n)).map(|n| (Some(time::year_of(now_ms) - n + 1), None)),
        None => span(value),
    }
}

/// The least and the most impact factor a `Filters::impact` keeps, either left open.
pub fn impact_span(value: &str) -> Option<(Option<f64>, Option<f64>)> {
    span::<f64>(value).filter(|(low, high)| low.is_none_or(f64::is_finite) && high.is_none_or(f64::is_finite))
}

/// The partition and the last zone kept of a `Filters::zone`.
pub fn zone_parts(value: &str) -> Option<(&'static str, u8)> {
    let (which, last) = value.split_once(':')?;
    let which = journals::ZONES.iter().find(|(key, _)| *key == which)?.0;
    Some((which, last.trim().parse().ok().filter(|z| (1..=4).contains(z))?))
}

/// What `Filters::zone` is for the zones 1 to `last` of a partition.
pub fn zone_value(which: &str, last: u8) -> String {
    format!("{which}:{last}")
}

/// A zone filter as it is under another partition: the same zones of that one. Anything else as it is.
pub fn zone_in(value: &str, which: &str) -> String {
    zone_parts(value).map_or_else(|| value.to_string(), |(_, last)| zone_value(which, last))
}

/// Everything the search box matches, lowercased.
fn haystack(p: &Paper) -> String {
    let mut hay = [p.title.as_str(), &p.authors, &p.doi, &p.pmid, &p.journal, &p.abstract_text, &p.notes].join(" ");
    for tag in &p.tags {
        hay.push(' ');
        hay.push_str(tag);
    }
    for values in p.fields.values() {
        for value in values {
            hay.push(' ');
            hay.push_str(value);
        }
    }
    if let Some(year) = p.year {
        hay.push_str(&format!(" {year}"));
    }
    hay.to_lowercase()
}

pub fn matches(p: &Paper, f: &Filters, now_ms: i64) -> bool {
    // Papers marked irrelevant stay in the library only so that imports skip
    // them; they show up when asked for by name.
    if f.screening.is_empty() { if p.screening == "irrelevant" { return false } } else if p.screening != f.screening { return false }
    if !in_view(p, f.view, now_ms) { return false }
    if !f.tagged(p) { return false }
    if !f.year.is_empty() && !year_span(&f.year, now_ms).zip(p.year).is_some_and(|(span, year)| within(year, &span)) { return false }
    if !f.impact.is_empty() || !f.zone.is_empty() {
        // Both are the journal's, as the table that comes with the app has it.
        let journal = journals::find(&p.journal);
        if !f.impact.is_empty() && !impact_span(&f.impact).zip(journal.and_then(|j| j.impact)).is_some_and(|(span, impact)| within(impact, &span)) { return false }
        if !f.zone.is_empty() && !zone_parts(&f.zone).zip(journal).is_some_and(|((which, last), j)| j.zone_number(which).is_some_and(|zone| zone <= last)) { return false }
    }
    if !f.reading.is_empty() && p.reading != f.reading { return false }
    if !f.fulltext.is_empty() && p.fulltext != f.fulltext { return false }
    if !f.source.is_empty() && p.source != f.source { return false }
    match f.preprint.as_str() {
        "yes" if p.publication_status != "preprint" => return false,
        "no" if p.publication_status == "preprint" => return false,
        _ => {}
    }
    for (field, wanted) in &f.fields {
        if !p.fields.get(field).is_some_and(|values| values.contains(wanted)) { return false }
    }
    says(p, &f.search)
}

/// Whether a paper holds every word of what was typed into a search box, somewhere in its record; nothing typed matches every paper.
pub fn says(p: &Paper, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() { return true }
    let hay = haystack(p);
    query.split_whitespace().all(|word| hay.contains(word))
}

/// The key and direction of a sort value; anything unknown is "newest".
pub fn sort_parts(sort: &str) -> (&str, bool) {
    match sort.rsplit_once('-') {
        Some((key, dir)) if ["title", "year", "journal", "authors", "impact", "cas", "xr", "jcr"].contains(&key) => (key, dir == "desc"),
        _ => ("added", true),
    }
}

/// Clicking a column header: flip the direction when it already sorts, else its default.
pub fn toggle_sort(current: &str, key: &str) -> String {
    let (now, desc) = sort_parts(current);
    if now == key {
        format!("{key}-{}", if desc { "asc" } else { "desc" })
    } else {
        format!("{key}-{}", if key == "year" || key == "impact" { "desc" } else { "asc" })
    }
}

/// Text order in either direction; blank values always sink to the end.
fn text_cmp(a: &str, b: &str, desc: bool) -> Ordering {
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => {
            let order = a.to_lowercase().cmp(&b.to_lowercase());
            if desc { order.reverse() } else { order }
        }
    }
}

fn compare(a: &Paper, b: &Paper, key: &str, desc: bool) -> Ordering {
    match key {
        "title" => text_cmp(&a.title, &b.title, desc),
        "journal" => text_cmp(&a.journal, &b.journal, desc),
        "authors" => text_cmp(&a.authors, &b.authors, desc),
        "year" => match (a.year, b.year) {
            (None, None) => Ordering::Equal,
            (None, _) => Ordering::Greater,
            (_, None) => Ordering::Less,
            (Some(x), Some(y)) => if desc { y.cmp(&x) } else { x.cmp(&y) },
        },
        // The journal's impact factor or zone (`journals`): papers of journals the table does not know sink.
        "impact" => sunk(journals::find(&a.journal).and_then(|j| j.impact), journals::find(&b.journal).and_then(|j| j.impact), desc),
        "cas" | "xr" | "jcr" => sunk(journals::find(&a.journal).and_then(|j| j.zone_rank(key)), journals::find(&b.journal).and_then(|j| j.zone_rank(key)), desc),
        _ => b.added_at.cmp(&a.added_at),
    }
}

/// Order in either direction, with what has no value at the end.
fn sunk<T: PartialOrd>(a: Option<T>, b: Option<T>, desc: bool) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let order = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
            if desc { order.reverse() } else { order }
        }
    }
}

/// The indices of the papers that pass the filters, in display order.
pub fn filter_papers(papers: &[Paper], f: &Filters, now_ms: i64) -> Vec<usize> {
    let (key, desc) = sort_parts(&f.sort);
    let mut found: Vec<usize> = (0..papers.len()).filter(|&i| matches(&papers[i], f, now_ms)).collect();
    // Stable: equal keys keep the library's newest-first order.
    found.sort_by(|&a, &b| compare(&papers[a], &papers[b], key, desc));
    found
}

/// How many papers each sidebar view holds.
pub fn view_count(papers: &[Paper], view: View, now_ms: i64) -> usize {
    papers.iter().filter(|p| p.screening != "irrelevant" && in_view(p, view, now_ms)).count()
}

/// The sidebar's views, in the order `Counts::views` holds them.
pub const VIEWS: [View; 5] = [View::All, View::Recent, View::Unread, View::Starred, View::NoPdf];

/// What the sidebar counts under the search and filters that are set: how many papers each view would
/// show, and each tag. A view is counted as if it were the one chosen, under the tags chosen and excluded.
/// A tag is counted among the papers on show (the view chosen and the tags too): how many would stay if it
/// were added to the chosen ones, so a chosen tag counts all on show and a tag none of them holds counts 0.
/// An excluded tag counts the papers it alone hides, those that would be on show holding it were it not
/// excluded. Everything else set narrows both. With nothing set, the counts of the whole library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub views: [usize; 5],
    pub tags: std::collections::HashMap<String, usize>,
}

impl Counts {
    pub fn view(&self, view: View) -> usize {
        VIEWS.iter().position(|v| *v == view).map_or(0, |i| self.views[i])
    }

    pub fn tag(&self, tag: &str) -> usize {
        self.tags.get(tag).copied().unwrap_or(0)
    }
}

pub fn counts(papers: &[Paper], f: &Filters, now_ms: i64) -> Counts {
    let rest = Filters { view: View::All, tags: vec![], not_tags: vec![], ..f.clone() };
    let mut counts = Counts::default();
    for p in papers.iter().filter(|p| matches(p, &rest, now_ms)) {
        let held = f.tags.iter().all(|tag| p.tags.contains(tag));
        let mut hidden_by = f.not_tags.iter().filter(|tag| p.tags.contains(tag));
        let (first, second) = (hidden_by.next(), hidden_by.next());
        if held && first.is_none() {
            for (n, view) in counts.views.iter_mut().zip(VIEWS) {
                if in_view(p, view, now_ms) { *n += 1 }
            }
        }
        // On show, the paper counts for each of its tags; hidden by one excluded tag alone, for that one.
        if held && second.is_none() && in_view(p, f.view, now_ms) {
            if let Some(tag) = first {
                *counts.tags.entry(tag.clone()).or_default() += 1;
            } else {
                for tag in &p.tags {
                    *counts.tags.entry(tag.clone()).or_default() += 1;
                }
            }
        }
    }
    counts
}

/// Every distinct value with the number of papers holding it, most used first.
fn tally<'a>(values: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values.filter(|v| !v.is_empty()) {
        *counts.entry(value).or_default() += 1;
    }
    let mut list: Vec<(String, usize)> = counts.into_iter().map(|(v, n)| (v.to_string(), n)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    list
}

fn active(papers: &[Paper]) -> impl Iterator<Item = &Paper> {
    papers.iter().filter(|p| p.screening != "irrelevant")
}

pub fn tag_counts(papers: &[Paper]) -> Vec<(String, usize)> {
    tally(active(papers).flat_map(|p| p.tags.iter().map(String::as_str)))
}

/// Whether a tag holds every word typed into the sidebar's tag search, case aside, in any order.
pub fn tag_matches(tag: &str, words: &str) -> bool {
    let tag = tag.to_lowercase();
    words.split_whitespace().all(|word| tag.contains(&word.to_lowercase()))
}

pub fn source_counts(papers: &[Paper]) -> Vec<(String, usize)> {
    tally(active(papers).map(|p| p.source.as_str()))
}

pub fn field_counts(papers: &[Paper], field: &str) -> Vec<(String, usize)> {
    tally(active(papers).filter_map(|p| p.fields.get(field)).flat_map(|values| values.iter().map(String::as_str)))
}

/// How many papers a filter by itself would keep, those marked irrelevant apart: the number beside a choice in a
/// filter's menu, which like `years` and `tag_counts` is of the whole library.
pub fn count_with(papers: &[Paper], set: impl FnOnce(&mut Filters), now_ms: i64) -> usize {
    let mut f = Filters::default();
    set(&mut f);
    papers.iter().filter(|p| matches(p, &f, now_ms)).count()
}

/// The years papers were published in, latest first.
pub fn years(papers: &[Paper]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for year in active(papers).filter_map(|p| p.year) {
        *counts.entry(year).or_default() += 1;
    }
    counts.into_iter().rev().map(|(y, n)| (y.to_string(), n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(title: &str, year: Option<i32>, added: &str) -> Paper {
        let mut p = Paper { id: title.into(), title: title.into(), year, added_at: added.into(), ..Default::default() };
        p.normalize();
        p.added_at = added.into();
        p
    }

    fn library() -> Vec<Paper> {
        let mut a = paper("Zebra fish atlas", Some(2021), "2026-01-03T00:00:00.000Z");
        a.tags = vec!["atlas".into(), "fish".into()];
        a.fields.insert("F1".into(), vec!["Danio rerio".into()]);
        let mut b = paper("apple genome", None, "2026-01-02T00:00:00.000Z");
        b.starred = true;
        b.publication_status = "preprint".into();
        let mut c = paper("Maize root cells", Some(2024), "2026-01-01T00:00:00.000Z");
        c.tags = vec!["atlas".into()];
        c.reading = "read".into();
        let mut d = paper("Off topic", Some(2020), "2026-01-04T00:00:00.000Z");
        d.screening = "irrelevant".into();
        vec![a, b, c, d]
    }

    #[test]
    fn a_search_box_asks_for_every_word() {
        let papers = library();
        assert!(says(&papers[0], "  FISH atlas ") && says(&papers[0], "danio 2021") && says(&papers[0], ""));
        assert!(!says(&papers[0], "fish maize") && !says(&papers[1], "atlas"));
    }

    fn titles(papers: &[Paper], f: &Filters) -> Vec<String> {
        filter_papers(papers, f, 0).into_iter().map(|i| papers[i].title.clone()).collect()
    }

    #[test]
    fn default_hides_irrelevant_and_sorts_newest() {
        let papers = library();
        assert_eq!(titles(&papers, &Filters::default()), ["Zebra fish atlas", "apple genome", "Maize root cells"]);
        let asked = Filters { screening: "irrelevant".into(), ..Default::default() };
        assert_eq!(titles(&papers, &asked), ["Off topic"]);
    }

    #[test]
    fn search_and_filters() {
        let papers = library();
        let f = |edit: fn(&mut Filters)| {
            let mut f = Filters::default();
            edit(&mut f);
            titles(&papers, &f)
        };
        assert_eq!(f(|f| f.search = "ATLAS fish".into()), ["Zebra fish atlas"]);
        assert_eq!(f(|f| f.search = "danio".into()), ["Zebra fish atlas"]);
        assert_eq!(f(|f| f.tags = vec!["atlas".into()]).len(), 2);
        assert_eq!(f(|f| f.year = "2024".into()), ["Maize root cells"]);
        assert_eq!(f(|f| f.preprint = "yes".into()), ["apple genome"]);
        assert_eq!(f(|f| f.preprint = "no".into()).len(), 2);
        assert_eq!(f(|f| f.view = View::Starred), ["apple genome"]);
        assert_eq!(f(|f| f.view = View::Unread).len(), 2);
        assert_eq!(f(|f| { f.fields.insert("F1".into(), "Danio rerio".into()); }), ["Zebra fish atlas"]);
        assert_eq!(f(|f| { f.fields.insert("F1".into(), "Mus".into()); }).len(), 0);
    }

    #[test]
    fn sorting_sinks_blanks() {
        let papers = library();
        let sorted = |sort: &str| titles(&papers, &Filters { sort: sort.into(), ..Default::default() });
        assert_eq!(sorted("title-asc"), ["apple genome", "Maize root cells", "Zebra fish atlas"]);
        assert_eq!(sorted("year-desc"), ["Maize root cells", "Zebra fish atlas", "apple genome"]);
        assert_eq!(sorted("year-asc"), ["Zebra fish atlas", "Maize root cells", "apple genome"]);
        assert_eq!(toggle_sort("newest", "year"), "year-desc");
        assert_eq!(toggle_sort("year-desc", "year"), "year-asc");
        assert_eq!(toggle_sort("year-asc", "title"), "title-asc");
    }

    #[test]
    fn sorting_by_the_journal_metrics() {
        let mut papers = library();
        papers[0].journal = "PLoS One".into();
        papers[1].journal = "Nature".into();
        papers[2].journal = "No Such Journal Anywhere".into();
        let sorted = |sort: &str| titles(&papers, &Filters { sort: sort.into(), ..Default::default() });
        assert_eq!(sorted("impact-desc"), ["apple genome", "Zebra fish atlas", "Maize root cells"]);
        assert_eq!(sorted("impact-asc"), ["Zebra fish atlas", "apple genome", "Maize root cells"]);
        assert_eq!(sorted("cas-asc"), ["apple genome", "Zebra fish atlas", "Maize root cells"]);
        assert_eq!(toggle_sort("newest", "impact"), "impact-desc");
        assert_eq!(toggle_sort("newest", "cas"), "cas-asc");
    }

    #[test]
    fn tallies() {
        let papers = library();
        assert_eq!(tag_counts(&papers), [("atlas".to_string(), 2), ("fish".to_string(), 1)]);
        assert_eq!(years(&papers), [("2024".to_string(), 1), ("2021".to_string(), 1)]);
        assert_eq!(view_count(&papers, View::All, 0), 3);
        assert_eq!(field_counts(&papers, "F1"), [("Danio rerio".to_string(), 1)]);
        let mut f = Filters { tags: vec!["x".into()], search: "q".into(), sort: "year-asc".into(), view: View::Starred, ..Default::default() };
        assert_eq!(f.active(), 1);
        f.clear();
        assert_eq!((f.active(), f.search.as_str(), f.sort.as_str(), f.view), (0, "", "year-asc", View::Starred));
    }

    #[test]
    fn several_tags_are_all_held_and_excluded_ones_hide() {
        let papers = library();
        let tagged = |tags: &[&str], not_tags: &[&str]| titles(&papers, &Filters { tags: tags.iter().map(|t| t.to_string()).collect(), not_tags: not_tags.iter().map(|t| t.to_string()).collect(), ..Default::default() });
        assert_eq!(tagged(&["atlas"], &[]), ["Zebra fish atlas", "Maize root cells"]);
        assert_eq!(tagged(&["atlas", "fish"], &[]), ["Zebra fish atlas"]);
        assert_eq!(tagged(&["fish", "none"], &[]).len(), 0);
        assert_eq!(tagged(&["atlas"], &["fish"]), ["Maize root cells"]);
        assert_eq!(tagged(&[], &["atlas"]), ["apple genome"]);
        assert_eq!(tagged(&[], &["fish", "atlas"]), ["apple genome"]);
        let f = Filters { tags: vec!["a".into()], not_tags: vec!["b".into(), "c".into()], ..Default::default() };
        assert_eq!((f.active(), f.tags_said().as_str()), (3, "#a −#b −#c"));
    }

    #[test]
    fn clicks_choose_add_and_exclude_tags() {
        let mut f = Filters::default();
        let state = |f: &Filters| (f.tags.join(","), f.not_tags.join(","));
        f.click_tag("a", false);
        assert_eq!(state(&f), ("a".into(), "".into()));
        // Alone, a click on the only tag chosen lets it go.
        f.click_tag("a", false);
        assert_eq!(state(&f), ("".into(), "".into()));
        f.click_tag("a", false);
        f.click_tag("b", true);
        assert_eq!(state(&f), ("a,b".into(), "".into()));
        // Alone, a click on one of several chooses just that one.
        f.click_tag("b", false);
        assert_eq!(state(&f), ("b".into(), "".into()));
        f.click_tag("a", true);
        f.click_tag("b", true);
        assert_eq!(state(&f), ("a".into(), "".into()));
        // Excluding a chosen tag takes it out of the chosen; a click on an excluded tag lets the exclusion go.
        f.exclude_tag("a");
        f.exclude_tag("c");
        assert_eq!(state(&f), ("".into(), "a,c".into()));
        f.click_tag("a", false);
        assert_eq!(state(&f), ("".into(), "c".into()));
        f.exclude_tag("c");
        assert_eq!(state(&f), ("".into(), "".into()));
        // Renamed onto one already chosen, a tag is chosen once; taken off the papers, it is no choice.
        let mut f = Filters { tags: vec!["a".into(), "b".into()], not_tags: vec!["c".into()], ..Default::default() };
        f.retag("a", Some("b"));
        f.retag("c", None);
        assert_eq!(state(&f), ("b".into(), "".into()));
        f.clear();
        assert_eq!(f, Filters::default());
    }

    #[test]
    fn a_tag_counts_what_would_stay_were_it_added() {
        let papers = library();
        let with = |tags: &[&str], not_tags: &[&str]| Filters { tags: tags.iter().map(|t| t.to_string()).collect(), not_tags: not_tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let c = counts(&papers, &with(&["atlas"], &[]), 0);
        assert_eq!((c.view(View::All), c.view(View::Unread), c.tag("atlas"), c.tag("fish")), (2, 1, 2, 1));
        let c = counts(&papers, &with(&["atlas", "fish"], &[]), 0);
        assert_eq!((c.view(View::All), c.tag("atlas"), c.tag("fish")), (1, 1, 1));
        // An excluded tag counts the papers it alone hides.
        let c = counts(&papers, &with(&[], &["fish"]), 0);
        assert_eq!((c.view(View::All), c.tag("atlas"), c.tag("fish")), (2, 1, 1));
        let c = counts(&papers, &with(&[], &["fish", "atlas"]), 0);
        assert_eq!((c.view(View::All), c.tag("atlas"), c.tag("fish")), (1, 1, 0));
        let c = counts(&papers, &with(&["atlas"], &["fish"]), 0);
        assert_eq!((c.view(View::All), c.tag("atlas"), c.tag("fish")), (1, 1, 1));
        // A tag not excluded counts what the list shows with it added to the chosen ones.
        for f in [with(&[], &[]), with(&["atlas"], &[]), with(&[], &["fish"]), Filters { view: View::Unread, ..with(&["fish"], &[]) }] {
            let c = counts(&papers, &f, 0);
            for (tag, _) in tag_counts(&papers).into_iter().filter(|(tag, _)| !f.excluded(tag)) {
                let mut added = f.clone();
                if !added.chosen(&tag) { added.tags.push(tag.clone()) }
                assert_eq!(c.tag(&tag), filter_papers(&papers, &added, 0).len(), "{tag} under {f:?}");
            }
        }
    }

    #[test]
    fn the_sidebar_counts_follow_the_search_and_filters() {
        let papers = library();
        // Nothing set: the whole library, without the irrelevant paper.
        let all = counts(&papers, &Filters::default(), 0);
        assert_eq!((all.view(View::All), all.view(View::Starred), all.view(View::Unread)), (3, 1, 2));
        assert_eq!((all.tag("atlas"), all.tag("fish"), all.tag("none")), (2, 1, 0));
        assert_eq!(all.view(View::All), view_count(&papers, View::All, 0));
        // A search narrows every view and every tag.
        let searched = counts(&papers, &Filters { search: "atlas".into(), ..Default::default() }, 0);
        assert_eq!((searched.view(View::All), searched.view(View::Unread), searched.view(View::Starred)), (2, 1, 0));
        assert_eq!((searched.tag("atlas"), searched.tag("fish")), (2, 1));
        // The tags chosen narrow the views, and a tag counts the papers on show; the view chosen narrows the tags, not the other views.
        let tagged = counts(&papers, &Filters { tags: vec!["fish".into()], view: View::Unread, ..Default::default() }, 0);
        assert_eq!((tagged.view(View::All), tagged.view(View::Unread), tagged.view(View::Starred)), (1, 1, 0));
        assert_eq!((tagged.tag("atlas"), tagged.tag("fish")), (1, 1));
        // Asked for by name, the irrelevant paper is counted.
        let irrelevant = counts(&papers, &Filters { screening: "irrelevant".into(), ..Default::default() }, 0);
        assert_eq!(irrelevant.view(View::All), 1);
        // Each count is what the list shows with that view chosen.
        let f = Filters { search: "a".into(), year: "2021".into(), ..Default::default() };
        let c = counts(&papers, &f, 0);
        for view in VIEWS {
            assert_eq!(c.view(view), filter_papers(&papers, &Filters { view, ..f.clone() }, 0).len());
        }
    }

    #[test]
    fn years_are_one_year_a_span_or_the_last_few() {
        let papers = library();
        let now = time::millis("2026-06-15T00:00:00.000Z");
        let kept = |year: &str| {
            let mut found: Vec<i32> = filter_papers(&papers, &Filters { year: year.into(), ..Default::default() }, now).into_iter().filter_map(|i| papers[i].year).collect();
            found.sort();
            found
        };
        // The irrelevant paper of 2020 stays out, and the one without a year passes no filter by year.
        assert_eq!(kept("2024"), [2024]);
        assert_eq!(kept("2021-2024"), [2021, 2024]);
        assert_eq!(kept("2022-"), [2024]);
        assert_eq!(kept("-2021"), [2021]);
        assert_eq!(kept("2025-2021"), [] as [i32; 0]);
        // 近 3 年 in 2026 is 2024, 2025 and 2026; 近 6 年 reaches 2021.
        assert_eq!((kept("last:3"), kept("last:6")), (vec![2024], vec![2021, 2024]));
        assert_eq!(year_span("last:3", now), Some((Some(2024), None)));
        // What says no years keeps no paper, and does not pass for "everything".
        for nonsense in ["soon", "-", "last:0", "last:x", "20x0-2024"] {
            assert_eq!((year_span(nonsense, now), kept(nonsense).len()), (None, 0), "{nonsense}");
        }
        assert_eq!((span_value(Some(2020), None::<i32>), span_value(None, Some(2010)), span_value(Some(5.5), Some(10.)), span_value(None::<i32>, None)), ("2020-".into(), "-2010".into(), "5.5-10".into(), String::new()));
        assert_eq!(year_span(&span_value(Some(2000), Some(2026)), now), Some((Some(2000), Some(2026))));
    }

    #[test]
    fn impact_and_zone_are_the_journals() {
        // A journal of the table in 中科院's zone 3 with a low factor, beside Nature.
        let modest = journals::table().journals.iter().find(|j| j.cas == Some(3) && j.impact.is_some_and(|f| f < 5.) && j.quartile == "Q2" && journals::find(j.name()).is_some_and(|found| std::ptr::eq(found, *j))).unwrap();
        let mut papers = library();
        papers[0].journal = "Nature".into();
        papers[1].journal = modest.name().to_string();
        papers[2].journal = "Journal of Things Nobody Indexed".into();
        let kept = |edit: fn(&mut Filters)| {
            let mut f = Filters::default();
            edit(&mut f);
            titles(&papers, &f)
        };
        assert_eq!(kept(|f| f.impact = "10-".into()), ["Zebra fish atlas"]);
        assert_eq!(kept(|f| f.impact = "-5".into()), ["apple genome"]);
        assert_eq!(kept(|f| f.impact = "0-".into()), ["Zebra fish atlas", "apple genome"]);
        assert_eq!(kept(|f| f.impact = "many".into()).len(), 0);
        assert_eq!(impact_span("2.5-10"), Some((Some(2.5), Some(10.))));
        assert_eq!((impact_span("inf-"), impact_span("-")), (None, None));
        // 1 区 alone, then 1–3 区; JCR counts its quartiles the same way.
        assert_eq!(kept(|f| f.zone = "cas:1".into()), ["Zebra fish atlas"]);
        assert_eq!(kept(|f| f.zone = "cas:3".into()), ["Zebra fish atlas", "apple genome"]);
        assert_eq!(kept(|f| f.zone = "jcr:1".into()), ["Zebra fish atlas"]);
        assert_eq!(kept(|f| f.zone = "jcr:2".into()), ["Zebra fish atlas", "apple genome"]);
        assert_eq!(kept(|f| f.zone = "cas:9".into()).len(), 0);
        assert_eq!((zone_parts("xr:2"), zone_parts("cas"), zone_parts("abc:1")), (Some(("xr", 2)), None, None));
        // The same zones under another partition, when the 分区 column changes to it.
        assert_eq!((zone_in("cas:2", "jcr"), zone_in("", "jcr")), ("jcr:2".to_string(), String::new()));
        // The filters work together, and each counts as one that is set.
        let both = Filters { year: "2020-2022".into(), impact: "10-".into(), zone: "cas:1".into(), ..Default::default() };
        assert_eq!((titles(&papers, &both), both.active()), (vec!["Zebra fish atlas".to_string()], 3));
        assert_eq!(counts(&papers, &both, 0).view(View::All), 1);
        assert_eq!((count_with(&papers, |f| f.impact = "10-".into(), 0), count_with(&papers, |f| f.zone = "cas:3".into(), 0), count_with(&papers, |_| {}, 0)), (1, 2, 3));
    }

    #[test]
    fn tags_found_by_their_words() {
        assert!(tag_matches("单细胞转录组", "转录"));
        assert!(tag_matches("BiblioCell已纳入", "bibliocell"));
        assert!(tag_matches("细胞分化与谱系", "谱系 分化"));
        assert!(!tag_matches("细胞分化与谱系", "谱系 图谱"));
        assert!(tag_matches("atlas", "  "));
    }
}
