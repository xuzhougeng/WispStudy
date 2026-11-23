//! A PDF handed to the app to be read: one dropped on the home page, chosen in Finder or Explorer with
//! 打开方式 › Wisp Study, or named on the command line. It opens in reading mode, in the library that
//! already holds the same file (its bytes, `Store::paper_holding`; the conversation of an earlier reading
//! included), or else in a new conversation named after the file (a draft, as a first question's PDF makes):
//! Esc leaves the reading for that conversation, where the paper is asked about, and 建成文献库 keeps it.

use crate::app::{Cmd, Tab, Workspace};
use gpui::*;
use std::path::{Path, PathBuf};
use wisp_core::libraries;
use wisp_core::lookup::{self, Identifier};
use wisp_core::store::Store;

pub fn is_pdf(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// The PDFs named on the command line: Explorer's 打开方式 passes the file as the first argument.
pub fn from_args() -> Vec<PathBuf> {
    std::env::args_os().skip(1).map(PathBuf::from).filter(|path| is_pdf(path)).collect()
}

/// A `file://` address, as macOS hands a file over, as a path.
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/…` names the same machine.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    // A `#` or `?` in a file's name comes encoded: unencoded, they end the path.
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    if !rest.starts_with('/') { return None }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, ..) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

impl Workspace {
    /// Files dropped on the window. On the home page a single PDF is read; anything else goes where
    /// `import_paths` sends it (the assistant's box on the home side, the library open).
    pub(crate) fn drop_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.lib.is_none() && !self.home.open && self.settings_view.is_none();
        if home && let [one] = paths.as_slice() && is_pdf(one) { return self.read_pdf(one.clone(), window, cx) }
        // Pictures dropped on a manuscript are its figures: kept in 资料库 and put in at the caret (write_figures.rs).
        if Self::pictures_by_name(&paths) && self.modal.is_none() && self.settings_view.is_none() && self.lib.as_ref().is_some_and(|lib| lib.page == crate::app::Page::Write && lib.writing.current().is_some()) { return self.write_figures_add(paths, true, cx) }
        self.import_paths(paths, cx)
    }

    /// Files handed over from outside the window (Finder, Explorer, the command line): the first PDF is read,
    /// wherever the window was, and the window comes to the front.
    pub(crate) fn open_handed(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let pdfs: Vec<PathBuf> = paths.into_iter().filter(|path| is_pdf(path)).collect();
        let Some(first) = pdfs.first().cloned() else { return };
        window.activate_window();
        self.settings_view = None;
        self.modal = None;
        self.read_pdf(first, window, cx);
        if pdfs.len() > 1 { self.say(tf!("一次打开一篇：其余 {} 个 PDF 没有打开。", "One at a time: the other {} PDFs were not opened.", pdfs.len() - 1), cx) }
    }

    /// The library that holds the file is looked for off the main thread (the one on show first, the
    /// libraries before the conversations); the text of a file held nowhere is read there too.
    pub(crate) fn read_pdf(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let on_show = self.lib.as_ref().map(|lib| lib.info.id.clone()).unwrap_or_default();
        let mut places: Vec<(bool, bool, String, PathBuf)> = self.libraries.iter().map(|l| (l.id != on_show, l.draft, l.id.clone(), l.dir.clone())).collect();
        places.sort_by_key(|(away, draft, ..)| (*away, *draft));
        let file = path.clone();
        let work = cx.background_executor().spawn(async move {
            let held = places.iter().find_map(|(_, _, id, dir)| {
                let store = Store::open(dir).ok()?;
                let paper = store.paper_holding(&file).ok()??;
                (!store.in_trash(&paper).unwrap_or(true)).then(|| (id.clone(), paper))
            });
            let pages = held.is_none().then(|| wisp_core::fulltext::file_pages(&file));
            (held, pages)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (held, pages) = work.await;
            this.update_in(cx, |this, window, cx| match (held, pages) {
                (Some((library, paper)), _) => this.read_held(&library, &paper, window, cx),
                (None, Some(pages)) => this.read_new(&path, pages, window, cx),
                (None, None) => {}
            })
            .ok();
        })
        .detach();
    }

    fn read_held(&mut self, library: &str, paper: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != library) { self.open_library(library, window, cx) }
        if self.lib.as_ref().is_some_and(|lib| lib.info.id == library && lib.paper(paper).is_some()) { self.show_reading(paper, window, cx) }
    }

    /// A file held nowhere: a conversation of its own, the file its paper. The DOI on its first pages is
    /// looked up afterwards, and what is found fills the record and names the conversation.
    fn read_new(&mut self, path: &Path, pages: Result<Vec<String>, String>, window: &mut Window, cx: &mut Context<Self>) {
        let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
        let doi = pages.as_ref().ok().and_then(|pages| lookup::find_doi(&pages.iter().take(2).cloned().collect::<Vec<_>>().join("\n")));
        let info = match libraries::create_draft(&self.root, &stem) {
            Ok(info) => info,
            Err(err) => return self.fail(format!("{err:#}"), cx),
        };
        self.libraries = libraries::list(&self.root);
        self.open_library(&info.id, window, cx);
        if self.lib.as_ref().is_none_or(|lib| lib.info.id != info.id) { return }
        self.add_pdf(path, &[], pages, None, cx);
        let Some(paper) = self.lib.as_ref().and_then(|lib| lib.store.paper_holding(path).ok().flatten()) else {
            let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            return self.fail(tf!("「{name}」没能打开。", "Could not open “{name}”."), cx);
        };
        self.show_reading(&paper, window, cx);
        let Some(doi) = doi else { return };
        let work = cx.background_executor().spawn(async move { wisp_net::lookup_paper(&Identifier::Doi(doi)).ok() });
        let library = info.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Some(found) = work.await else { return };
            this.update_in(cx, |this, window, cx| this.fill_read(&library, &paper, &stem, found, window, cx)).ok();
        })
        .detach();
    }

    /// What the DOI's lookup found, into a record still as the file made it (its name for a title, no DOI).
    fn fill_read(&mut self, library: &str, paper: &str, stem: &str, found: wisp_core::model::Paper, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref().filter(|lib| lib.info.id == library) else { return };
        let Some(held) = lib.paper(paper).cloned() else { return };
        if !held.doi.is_empty() { return }
        let mut blank = held.clone();
        if blank.title.trim() == stem.trim() { blank.title.clear() }
        let mut merged = lookup::merge_lookup(blank, found);
        self.change(cx, |lib| lib.store.save_paper(&mut merged));
        // The conversation is named after the paper, unless it was renamed meanwhile.
        let Some(lib) = self.lib.as_mut().filter(|lib| lib.info.id == library) else { return };
        let title = lib.paper(paper).map(|p| p.title.clone()).unwrap_or_default();
        if lib.info.draft && lib.info.name == libraries::draft_title(stem) && !title.is_empty() {
            lib.info.name = libraries::draft_title(&title);
            let _ = libraries::save(&lib.info);
            window.set_window_title(&format!("{} — Wisp Study", lib.info.name));
            self.libraries = libraries::list(&self.root);
        }
        cx.notify();
    }

    /// The paper on show in reading mode, at its PDF.
    fn show_reading(&mut self, paper: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Some(paper.to_string()), window, cx);
        self.run(Cmd::SetTab(Tab::Pdf), window, cx);
        if let Some(lib) = self.lib.as_mut() { lib.reading = true }
        window.focus(&self.focus);
        cx.notify();
    }

    /// Files handed over while the app runs: macOS's open events (through `inbox`), and on Windows the
    /// files a second launch left for this window when it brought it to the front. The files named at
    /// this start are read first.
    pub(crate) fn listen_for_files(&mut self, inbox: async_channel::Receiver<Vec<PathBuf>>, window: &mut Window, cx: &mut Context<Self>) {
        let named = from_args();
        if !named.is_empty() { self.open_handed(named, window, cx) }
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(paths) = inbox.recv().await {
                if this.update_in(cx, |this, window, cx| this.open_handed(paths, window, cx)).is_err() { break }
            }
        })
        .detach();
        #[cfg(target_os = "windows")]
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() { return }
            let paths = crate::background::take_handed();
            if !paths.is_empty() { this.open_handed(paths, window, cx) }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::file_url_path;
    use std::path::PathBuf;

    #[test]
    fn file_addresses_become_paths() {
        assert_eq!(file_url_path("file:///Users/a/Papers/Smith%202020.pdf"), Some(PathBuf::from("/Users/a/Papers/Smith 2020.pdf")));
        assert_eq!(file_url_path("file://localhost/tmp/a.pdf"), Some(PathBuf::from("/tmp/a.pdf")));
        assert_eq!(file_url_path("file:///tmp/%E6%96%87%E7%8C%AE.pdf"), Some(PathBuf::from("/tmp/文献.pdf")));
        // A lone % stays as it is.
        assert_eq!(file_url_path("file:///tmp/100%.pdf"), Some(PathBuf::from("/tmp/100%.pdf")));
        assert_eq!(file_url_path("file:///tmp/Notes%20%231.pdf"), Some(PathBuf::from("/tmp/Notes #1.pdf")));
        assert_eq!(file_url_path("https://example.org/a.pdf"), None);
        assert_eq!(file_url_path("wispstudy://oauth"), None);
    }
}
