//! The changes an assistant makes to a section of a manuscript, each the
//! words to find there and the words to put in their place (`edit_section`
//! through `compose`, and the list of what 助手写 changed in the window).
//! Nothing here touches a store: `place` finds where a change goes in the
//! text as it now stands, `follow` keeps a change's place while the text
//! around it changes, and `added` is the words to light up.

use std::ops::Range;

/// One change the model asks for: the words to find, and what to put there.
/// No words to find means the end of the section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub old: String,
    pub new: String,
}

/// Where a change goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placed {
    /// These bytes of the text become these words.
    At(Range<usize>, String),
    /// Its words are not in the text.
    Missing,
    /// It would change nothing.
    Same,
}

/// Where the words `old` stand in `text`, and whether as written: the place nearest to `near` when there are
/// several, else as `comment::locate` finds them, so a line break or a curled quotation mark written
/// differently does not lose the change. Without words to find it is the end of the text.
pub fn find(text: &str, old: &str, near: Option<usize>) -> Option<(Range<usize>, bool)> {
    if old.trim().is_empty() { return Some((text.trim_end().len()..text.len(), true)) }
    let exact = text.match_indices(old).map(|(at, _)| at).min_by_key(|at| near.map_or(*at, |near| at.abs_diff(near)));
    match exact {
        Some(at) => Some((at..at + old.len(), true)),
        None => crate::comment::locate(text, old, near).map(|range| (range, false)),
    }
}

/// Where `edit` goes in `text` as it stands (`find`), and the words that go there. At the end of the text
/// they open a paragraph of their own; a paragraph taken out takes the empty line after it along.
pub fn place(text: &str, edit: &Edit, near: Option<usize>) -> Placed {
    let Some((mut range, exact)) = find(text, &edit.old, near) else { return Placed::Missing };
    if edit.old.trim().is_empty() {
        let new = edit.new.trim();
        if new.is_empty() { return Placed::Same }
        return Placed::At(range.clone(), if range.start == 0 { new.to_string() } else { format!("\n\n{new}") });
    }
    let new = if exact { edit.new.clone() } else { edit.new.trim().to_string() };
    if text[range.clone()] == new { return Placed::Same }
    if new.trim().is_empty() {
        let alone = (range.start == 0 || text[..range.start].ends_with('\n')) && (range.end == text.len() || text[range.end..].starts_with('\n'));
        if alone {
            let after = text[range.end..].len() - text[range.end..].trim_start_matches('\n').len();
            if after > 0 { range.end += after } else { range.start = text[..range.start].trim_end_matches('\n').len() }
        }
        return Placed::At(range, String::new());
    }
    Placed::At(range, new)
}

/// Where `range` of `before` stands in `after`, when one stretch of the text was changed between the two
/// (a key typed, a paste, an undo, another change): as it was before that stretch, moved along after it,
/// and nowhere once the stretch reaches into it.
pub fn follow(range: &Range<usize>, before: &str, after: &str) -> Option<Range<usize>> {
    if before == after { return Some(range.clone()) }
    let (a, b) = (before.as_bytes(), after.as_bytes());
    let mut head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !before.is_char_boundary(head) { head -= 1 }
    let most = a.len().min(b.len()) - head;
    let mut tail = a.iter().rev().zip(b.iter().rev()).take(most).take_while(|(x, y)| x == y).count();
    while !before.is_char_boundary(a.len() - tail) { tail -= 1 }
    let changed = head..a.len() - tail;
    if range.end <= changed.start { return Some(range.clone()) }
    if range.start >= changed.end {
        let moved = |at: usize| at + b.len() - a.len();
        return Some(moved(range.start)..moved(range.end));
    }
    None
}

/// The words of `new` that `old` did not have, as bytes of `new`: what to light up where a change was made.
/// All of it when the two are too unlike to compare, or when words were only taken out.
pub fn added(old: &str, new: &str) -> Vec<Range<usize>> {
    // Without the spaces and line breaks at either end of a stretch.
    let tidy = |ranges: Vec<Range<usize>>| -> Vec<Range<usize>> { ranges.into_iter().map(|range| range.start + (new[range.clone()].len() - new[range.clone()].trim_start().len())..range.start + new[range.clone()].trim_end().len()).filter(|range| range.start < range.end).collect() };
    let whole = || tidy(std::iter::once(0..new.len()).collect());
    if old.trim().is_empty() { return whole() }
    let Some((_, pieces)) = crate::ai::word_diff(old, new) else { return whole() };
    let (mut at, mut out): (usize, Vec<Range<usize>>) = (0, vec![]);
    for (change, piece) in &pieces {
        let range = at..at + piece.len();
        at = range.end;
        if *change != crate::ai::Change::Added { continue }
        match out.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => out.push(range),
        }
    }
    let out = tidy(out);
    if at != new.len() || out.is_empty() { whole() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str) -> Edit {
        Edit { old: old.into(), new: new.into() }
    }

    #[test]
    fn a_change_goes_where_its_words_are() {
        let text = "Roots grow. Shoots grow.\n\nRoots grow. The end.";
        assert_eq!(place(text, &edit("Roots grow.", "Roots branch."), None), Placed::At(0..11, "Roots branch.".into()));
        // The place nearest to where the last change was made.
        assert_eq!(place(text, &edit("Roots grow.", "Roots branch."), Some(30)), Placed::At(26..37, "Roots branch.".into()));
        assert_eq!(place(text, &edit("Leaves grow.", "x"), None), Placed::Missing);
        assert_eq!(place(text, &edit("The end.", "The end."), None), Placed::Same);
        // Written with another line break and quotation marks: found by its characters.
        assert_eq!(place("He said “roots\ngrow” here.", &edit("said \"roots grow\"", "wrote"), None), Placed::At(3..24, "wrote".into()));
    }

    #[test]
    fn without_words_to_find_a_change_goes_to_the_end() {
        assert_eq!(place("", &edit("", "First."), None), Placed::At(0..0, "First.".into()));
        assert_eq!(place("One.\n\n", &edit("\n", "Two.\n"), None), Placed::At(4..6, "\n\nTwo.".into()));
        assert_eq!(place("One.", &edit("", " "), None), Placed::Same);
    }

    #[test]
    fn a_paragraph_taken_out_takes_its_empty_line_along() {
        let text = "One.\n\nTwo.\n\nThree.";
        let Placed::At(range, new) = place(text, &edit("Two.", ""), None) else { panic!() };
        assert_eq!(format!("{}{new}{}", &text[..range.start], &text[range.end..]), "One.\n\nThree.");
        let Placed::At(range, _) = place(text, &edit("Three.", ""), None) else { panic!() };
        assert_eq!(&text[..range.start], "One.\n\nTwo.");
        // Words inside a paragraph leave the lines alone.
        assert_eq!(place("One two.\n\nThree.", &edit(" two", ""), None), Placed::At(3..7, String::new()));
    }

    #[test]
    fn a_place_follows_the_text_around_it() {
        let before = "One. Two. Three.";
        let two = 5..9;
        assert_eq!(follow(&two, before, "One. Two. Three. Four."), Some(5..9));
        assert_eq!(follow(&two, before, "Zero. One. Two. Three."), Some(11..15));
        assert_eq!(follow(&two, before, "One. Three."), None);
        assert_eq!(follow(&two, before, "One. Twoo. Three."), None);
        // Typed right after it: not part of it. Right before it: it moves.
        assert_eq!(follow(&two, before, "One. Two.! Three."), Some(5..9));
        assert_eq!(follow(&two, before, "One. !Two. Three."), Some(6..10));
        // Letters of several bytes are not cut.
        assert_eq!(follow(&(6..9), "根。生长。", "根系。生长。"), Some(9..12));
        // A place without words (something taken out) stays where it is when words are typed at it.
        assert_eq!(follow(&(5..5), before, "One. !Two. Three."), Some(5..5));
    }

    #[test]
    fn the_new_words_are_what_lights_up() {
        let new = "Roots grow fast and deep in soil.";
        let lit: Vec<&str> = added("Roots grow in soil.", new).into_iter().map(|range| &new[range]).collect();
        assert_eq!(lit, vec!["fast and deep"]);
        assert_eq!(added("", "All new."), vec![0..8]);
        // Put at the end of a text: the words, not the empty line before them.
        assert_eq!(added("\n", "\n\nAll new."), vec![2..10]);
        // Words only taken out: the sentence they were in.
        assert_eq!(added("Roots grow very fast.", "Roots grow fast."), vec![0..16]);
        assert_eq!(added("Roots.", ""), Vec::<Range<usize>>::new());
    }
}
