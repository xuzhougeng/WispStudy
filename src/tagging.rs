//! Tags for several papers at once (#338): the papers a search and the filters left in the list (给这 N 篇加标签,
//! at the end of the filters' row) and the ones marked in it (the right click's 添加标签 and 去掉标签). A tag is
//! where a narrowed list is kept: the filters are for now, the tag stays. The engine's `Store::tag_papers` writes
//! all of it or none and spells a tag the way the library already does; here are the two dialogs, the sentence
//! that says how many papers gained the tag and how many had it, and the way back (the toast's 撤销).

use crate::app::{Cmd, Modal, Workspace};
use crate::theme::{Theme, text_px};
use gpui::{prelude::FluentBuilder, *};
use wisp_core::filter;
use wisp_core::model::{clean_tag, split_list, tag_key};
use wisp_core::organise::Tagged;

/// How many of the library's tags the dialog offers under its box: what is typed narrows a longer list.
const OFFERED: usize = 12;

/// The tags a sentence names, each in its brackets.
fn named(tags: &[String]) -> String {
    tags.iter().map(|tag| format!("「{tag}」")).collect()
}

/// What is being typed in a list of tags: its last entry while that is neither finished with a separator nor the
/// name of a tag the library has. It is what narrows the tags offered, and what a click on one of them replaces.
fn typing<'a>(text: &'a str, known: &[(String, usize)]) -> &'a str {
    let last = text.rsplit([',', '，', ';', '；', '\n']).next().unwrap_or("").trim();
    if known.iter().any(|(tag, _)| tag_key(tag) == tag_key(last)) { "" } else { last }
}

/// What a tagging did, for the toast: how many papers gained the tag and how many had it already.
fn said(tagged: &Tagged) -> String {
    let (gained, held) = (tagged.papers.len(), tagged.unchanged);
    let total = gained + held;
    if !tagged.removed.is_empty() && tagged.added.is_empty() {
        let names = named(&tagged.removed);
        return if gained == 0 { tf!("这 {total} 篇都没有{names}，没有改动。", "None of the {total} papers has {names}; nothing changed.") } else { tn!("已从 {n} 篇上去掉{names}。", "Removed {names} from {n}.", gained, "paper") };
    }
    let names = named(&tagged.added);
    if gained == 0 {
        tf!("这 {total} 篇都已经有{names}，没有改动。", "All {total} papers already have {names}; nothing changed.")
    } else if held == 0 {
        tn!("已给 {n} 篇加上{names}。", "Added {names} to {n}.", total, "paper")
    } else if tagged.added.len() == 1 {
        tn!("已给 {n} 篇加上{names}：新增 {gained}，已有 {held}。", "Added {names} to {n}: {gained} new, {held} already had it.", total, "paper")
    } else {
        tn!("已给 {n} 篇加上{names}：{gained} 篇有新增，{held} 篇已经都有。", "Added {names} to {n}: {gained} gained a tag, {held} already had them all.", total, "paper")
    }
}

impl Workspace {
    pub(crate) fn run_tagging(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::TagPapers(ids) => {
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                let name = self.modal_input("例如 综述、方法", "", false, window, cx);
                name.read(cx).focus(window);
                self.modal = Some(Modal::TagPapers { ids, name });
            }
            // Every paper on show, as the list has them now: the count on the button is the count tagged.
            Cmd::TagShown => {
                let ids: Vec<String> = self.lib.as_ref().map(|lib| lib.shown.iter().map(|&i| lib.papers[i].id.clone()).collect()).unwrap_or_default();
                if !ids.is_empty() { self.run_tagging(Cmd::TagPapers(ids), window, cx) }
            }
            Cmd::OfferTag(tag) => {
                let Some(Modal::TagPapers { name, .. }) = self.modal.as_ref() else { return };
                let known = self.lib.as_ref().map(|lib| lib.tags.clone()).unwrap_or_default();
                name.update(cx, |input, cx| {
                    let (mut tags, typed) = (split_list(input.text()), typing(input.text(), &known).to_string());
                    // The letters typed to find it are not a tag of their own.
                    if let Some(at) = tags.iter().rposition(|kept| !typed.is_empty() && *kept == typed) { tags.remove(at); }
                    if !tags.iter().any(|kept| tag_key(kept) == tag_key(&tag)) { tags.push(tag) }
                    input.set_text(&tags.join(", "), cx);
                });
                self.modal_error.clear();
                cx.notify();
            }
            Cmd::UntagPapers(ids) => {
                self.modal_subscriptions.clear();
                self.modal_error.clear();
                self.modal = Some(Modal::UntagPapers { ids, chosen: vec![] });
                cx.notify();
            }
            Cmd::ChooseUntag(tag) => {
                let Some(Modal::UntagPapers { chosen, .. }) = self.modal.as_mut() else { return };
                match chosen.iter().position(|kept| *kept == tag) {
                    Some(at) => { chosen.remove(at); }
                    None => chosen.push(tag),
                }
                self.modal_error.clear();
                cx.notify();
            }
            Cmd::UndoTagging(tagged) => {
                self.toast = None;
                let mut back = 0;
                let done = self.change(cx, |lib| {
                    back = lib.store.untag(&tagged)?;
                    Ok(())
                });
                // Nothing to take back where the library has been left since.
                if done && back > 0 { self.say(tn!("已撤销：{n} 篇的标签回到了之前。", "Undone: the tags of {n} are as they were.", back, "paper"), cx) }
            }
            _ => {}
        }
    }

    /// The tag that `name`, as another name for `tag`, would join: one the library has under it, case and width aside.
    pub(crate) fn tag_merged(&self, tag: &str, name: &str) -> Option<(String, usize)> {
        let (tags, key) = (&self.lib.as_ref()?.tags, tag_key(name));
        tags.iter().find(|(kept, _)| kept != tag && kept == name).or_else(|| tags.iter().find(|(kept, _)| kept != tag && tag_key(kept) == key)).cloned()
    }

    /// The dialogs' 添加 and 去掉: written as a whole, said with its numbers, and with the way back beside them.
    pub(crate) fn tag_papers(&mut self, ids: Vec<String>, add: Vec<String>, remove: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let add: Vec<String> = add.iter().map(|tag| clean_tag(tag)).filter(|tag| !tag.is_empty()).collect();
        if add.is_empty() && remove.is_empty() { return }
        let mut tagged = None;
        let done = self.change(cx, |lib| {
            tagged = Some(lib.store.tag_papers(&ids, &add, &remove)?);
            Ok(())
        });
        let Some(tagged) = tagged.filter(|_| done) else { return };
        self.run(Cmd::CloseModal, window, cx);
        let said = said(&tagged);
        if tagged.papers.is_empty() { self.say(said, cx) } else { self.say_undoable(said, Cmd::UndoTagging(tagged), cx) }
    }

    /// The library's tags under the box of 添加标签, the most used first, to take with a click: those not written
    /// yet, and only the ones holding the letters being typed.
    pub(crate) fn tag_offers(&self, name: &Entity<crate::input::TextInput>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        let text = name.read(cx).text().to_string();
        let (written, typing) = (split_list(&text), typing(&text, &lib.tags).to_string());
        lib.tags
            .iter()
            .filter(|(tag, _)| !written.iter().any(|kept| tag_key(kept) == tag_key(tag)) && filter::tag_matches(tag, &typing))
            .take(OFFERED)
            .enumerate()
            .map(|(i, (tag, _))| {
                div()
                    .id(("tag-offer", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(text_px(22.))
                    .px(px(8.))
                    .rounded(px(11.))
                    .border_1()
                    .border_color(Theme::line_strong())
                    .text_size(text_px(12.))
                    .text_color(Theme::text_secondary())
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover()).border_color(Theme::accent()))
                    .on_click(self.on(Cmd::OfferTag(tag.clone()), cx))
                    .child(format!("+ {tag}"))
                    .into_any_element()
            })
            .collect()
    }

    /// The tags the papers of 去掉标签 hold, each with how many of them hold it; the ones chosen to go are struck out.
    pub(crate) fn untag_choices(&self, ids: &[String], chosen: &[String], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lib) = self.lib.as_ref() else { return vec![] };
        // Counted over every one of them, the ones judged 不相关 too: they are in the list that was marked.
        let mut held: Vec<(String, usize)> = vec![];
        for tag in ids.iter().filter_map(|id| lib.paper(id)).flat_map(|paper| paper.tags.iter()) {
            match held.iter_mut().find(|(kept, _)| kept == tag) {
                Some((_, count)) => *count += 1,
                None => held.push((tag.clone(), 1)),
            }
        }
        held.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        held.into_iter()
            .enumerate()
            .map(|(i, (tag, count))| {
                let going = chosen.contains(&tag);
                div()
                    .id(("untag-choice", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(text_px(24.))
                    .px(px(9.))
                    .rounded(px(12.))
                    .border_1()
                    .text_size(text_px(12.))
                    .cursor_pointer()
                    .when(going, |d| d.bg(Theme::danger_soft()).border_color(Theme::danger_soft()).text_color(Theme::danger()))
                    .when(!going, |d| d.border_color(Theme::line_strong()).text_color(Theme::text_secondary()).hover(|s| s.bg(Theme::hover())))
                    .on_click(self.on(Cmd::ChooseUntag(tag.clone()), cx))
                    .child(div().when(going, |d| d.line_through()).child(tag))
                    .child(div().text_color(if going { Theme::danger() } else { Theme::text_tertiary() }).child(count.to_string()))
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{said, typing};
    use wisp_core::organise::Tagged;

    #[test]
    fn the_last_entry_is_what_is_being_typed() {
        let known = vec![("CAR-T".to_string(), 3), ("耗竭".to_string(), 1)];
        assert_eq!(typing("综述, 耗", &known), "耗");
        assert_eq!(typing("耗", &known), "耗");
        // Finished with a separator, or the name of a tag there is: nothing is being looked for.
        assert_eq!(typing("综述，", &known), "");
        assert_eq!(typing("综述, car-t", &known), "");
        assert_eq!(typing("", &known), "");
    }

    #[test]
    fn a_tagging_is_said_with_its_numbers() {
        let paper = |id: &str| (id.to_string(), vec![], vec!["耗竭".to_string()]);
        let tagged = |papers: usize, unchanged: usize| Tagged { added: vec!["耗竭".into()], removed: vec![], papers: (0..papers).map(|i| paper(&i.to_string())).collect(), unchanged };
        assert_eq!(said(&tagged(25, 12)), "已给 37 篇加上「耗竭」：新增 25，已有 12。");
        assert_eq!(said(&tagged(37, 0)), "已给 37 篇加上「耗竭」。");
        assert_eq!(said(&tagged(0, 37)), "这 37 篇都已经有「耗竭」，没有改动。");
        let off = Tagged { added: vec![], removed: vec!["耗竭".into(), "根".into()], papers: vec![paper("a"), paper("b")], unchanged: 3 };
        assert_eq!(said(&off), "已从 2 篇上去掉「耗竭」「根」。");
    }
}
