//! `WISP_STUDY_DEV`: the steps that script a debug window into a state at start, for screenshots while
//! developing. They are listed in `docs/dev/debugging.md`; a new step is an arm of `dev_step`.

use crate::app::{Cmd, FilterKey, Page, ReviewDo, ReviewText, ScopeDo, Side, Tab, Talk, Workspace};
use crate::chat_state::{ChatItem, Consent, Picture};
use crate::fetch::{FETCH_WORKERS, FetchRow, FetchRun};
use crate::importing::{IMPORT_WORKERS, ImportRow, ImportRun, Imported, ZoteroRun};
use crate::menus::MenuKind;
use crate::modal::Modal;
use crate::screen_run::ScreenMode;
use crate::input::InputEvent;
use crate::pdf::{Shows, Split, Zoom, ZoomTo};
use crate::settings::{AiTab, SettingsPage};
use gpui::*;
use std::path::PathBuf;
use wisp_core::ai;
use wisp_core::filter::View;
use wisp_core::libraries;
use wisp_core::settings::Language;
use wisp_core::zotero;

/// Debug builds only: `WISP_STUDY_DEV=open:0,select:1,tab:pdf` puts the
/// window into a state at start, for screenshots while developing.
#[cfg(debug_assertions)]
impl Workspace {
    pub fn dev_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(steps) = wisp_core::settings::env("DEV") else { return };
        for step in steps.split(',') {
            // `8@shot:/tmp/a.png` takes the step eight seconds after the start: what it is
            // to show (an import, an answer) needs that long to come about.
            // Parts of a second too (`3.2@shot:…`): a tooltip comes half a second after the pointer stops.
            match step.split_once('@').and_then(|(seconds, later)| Some((seconds.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.)?, later.to_string()))) {
                Some((seconds, later)) => {
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(std::time::Duration::from_secs_f64(seconds)).await;
                        this.update_in(cx, |this, window, cx| this.dev_step(&later, window, cx)).ok();
                    })
                    .detach();
                }
                None => self.dev_step(step, window, cx),
            }
        }
    }

    fn dev_step(&mut self, step: &str, window: &mut Window, cx: &mut Context<Self>) {
        {
            let (name, arg) = step.split_once(':').unwrap_or((step, ""));
            let cmd = match name {
                "do" => {
                    for step in arg.split('|') { self.dev_step(step, window, cx) }
                    return;
                }
                // UI layout checks without changing the normal startup or real libraries.
                "size" => {
                    if let Some((width, height)) = arg.split_once(';').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))) {
                        window.resize(size(px(width), px(height)));
                    }
                    return;
                }
                "ui" => match arg {
                    "clear" => {
                        self.run(Cmd::CloseModal, window, cx);
                        self.run(Cmd::CloseSettings, window, cx);
                        self.menu = None;
                        return;
                    }
                    "home" => Some(Cmd::GoHome),
                    "detail-close" => Some(Cmd::CloseDetail),
                    "new-library" => Some(Cmd::NewLibrary),
                    "edit-library" => self.lib.as_ref().map(|lib| Cmd::EditLibrary(lib.info.id.clone())),
                    "add" => Some(Cmd::AddById),
                    "manual" => Some(Cmd::AddManual),
                    "fields" => Some(Cmd::ManageFields),
                    "new-search" => Some(Cmd::NewSearch),
                    "zotero" => Some(Cmd::ImportZotero),
                    "edit-paper" => self.lib.as_ref().and_then(|lib| lib.selected.clone()).map(Cmd::EditPaper),
                    "delete-paper" => self.lib.as_ref().and_then(|lib| lib.selected.clone()).map(Cmd::AskDeletePaper),
                    // The trash page's head: 全部恢复 and the dialog of 清空回收站.
                    "restore-all" => Some(Cmd::RestoreAll),
                    "trash-all" => Some(Cmd::ChooseAllTrash(true)),
                    "empty-trash" => Some(Cmd::AskEmptyTrash),
                    "guide" => Some(Cmd::OpenGuide(true)),
                    "guide-search" => Some(Cmd::OpenGuide(false)),
                    _ => None,
                },
                // The n-th library card of the home page; the conversations of 最近对话 are `conv:<n>`.
                "open" => libraries::shelf(&self.libraries, &self.settings.library_order).into_iter().nth(arg.parse::<usize>().unwrap_or(0)).map(|l| Cmd::OpenLibrary(l.id.clone())),
                // `conv:<n>` by place (latest opened first), or `conv:<words>` the first whose name starts so.
                "conv" => match arg.parse::<usize>() {
                    Ok(n) => self.conversations().nth(n),
                    Err(_) => self.conversations().find(|l| l.name.starts_with(arg)),
                }
                .map(|l| Cmd::Draft(crate::draft::Do::Open(l.id.clone()))),
                // The home page's assistant: `home:<words>` leaves them in its box, `homefile:<path>` puts a Word file or a PDF with them
                // (as dropping it on the home page does), `homeask:<words>` sends them, which makes the conversation.
                "home" => {
                    self.home.input.update(cx, |input, cx| input.set_text(arg, cx));
                    self.home.input.read(cx).focus(window);
                    // As if typed: an `@` at its end is offered what it can name.
                    self.home_input(InputEvent::Changed, window, cx);
                    Some(Cmd::Draft(crate::draft::Do::Begin))
                }
                // `assistant` is the home page's corner button; `drafts` 历史 inside a conversation (again: back to it).
                "assistant" => Some(Cmd::Draft(crate::draft::Do::Begin)),
                // `homelist` / `homelist:cards`: the home page's libraries as rows, or as cards again.
                "homelist" => Some(Cmd::HomeList(arg != "cards")),
                // The home page's invitation to star the project, due in this run; nothing is written until it is answered.
                // 铃铛 on the home page: `bell` opens its list as a click on it does, `bell:<n>` is a click on its n-th row from 0, `bell:close` puts it away.
                "bell" => match arg {
                    "" => Some(Cmd::Bell(crate::bell::Do::Open)),
                    "close" => Some(Cmd::Bell(crate::bell::Do::Close)),
                    n => n.parse::<usize>().ok().and_then(|n| self.bell.done.get(n)).map(|done| Cmd::Bell(crate::bell::Do::Go(done.id))),
                },
                // 教程: `tutorials` opens them as the book on the home page does, `tutorials:<page>` or `tutorials:<page>#<section>` shows one (`index` is the shelf), `tutorials:close` leaves them.
                "tutorials" => match arg {
                    "" => Some(Cmd::Tutorial(crate::tutorial::Do::Open)),
                    "close" => Some(Cmd::Tutorial(crate::tutorial::Do::Close)),
                    page => {
                        let (page, section) = page.split_once('#').map_or((page, None), |(page, section)| (page, Some(section.to_string())));
                        if self.tutorials.is_none() { self.run(Cmd::Tutorial(crate::tutorial::Do::Open), window, cx) }
                        Some(Cmd::Tutorial(crate::tutorial::Do::Show(page.to_string(), section)))
                    }
                },
                // 收藏 of every library: `collected` opens it as the star on the home page does, `collected:<kind>` shows 文献, 图片, 方法 or 导言 (`papers`, `figures`, `methods`, `intros`), `collected:library:<n>` the n-th library that holds something alone (`collected:library` all again), `collected:go:<n>` is a click on the n-th row on show, `collected:star:<n>` on the n-th paper's star, `collected:close` leaves it.
                "collected" => self.collected_step(arg, cx),
                // 写作 from the home page: `written` opens it as the pen does, `written:all` / `written:unbound` / `written:library:<n>` show a place, `written:go:<n>` goes to the n-th row, `written:new` is 新稿件, `written:bind:<n>` opens the dialog that binds the n-th row (in a conversation's window: that conversation), `written:to:<n>` picks the n-th library in it (`submit` binds).
                "written" => self.written_step(arg, cx),
                "starask" => {
                    self.star_ask = true;
                    cx.notify();
                    return;
                }
                // 新手引导, which a scripted window does not begin by itself: `tour` begins it, `tour:next` is 下一步, `tour:end` its ×.
                "tour" => Some(Cmd::Tour(match arg {
                    "next" => crate::tour::Do::Next,
                    "end" => crate::tour::Do::End,
                    _ => crate::tour::Do::Begin,
                })),
                "drafts" => Some(Cmd::Draft(crate::draft::Do::History)),
                "newdraft" => Some(Cmd::Draft(crate::draft::Do::New)),
                // `draftmanuscript:off` closes the manuscript beside a conversation (its ×), `draftmanuscript:on` brings it back (稿件 in the header).
                "draftmanuscript" => Some(Cmd::Draft(crate::draft::Do::Manuscript(arg != "off"))),
                "homefile" => {
                    self.home_attach(vec![arg.into()], cx);
                    None
                }
                // `homestart:<n>` is the n-th start under the box; one that picks a Word file takes the one in the box when there is one.
                "homestart" => Some(Cmd::Draft(crate::draft::Do::Start(arg.parse().unwrap_or(0)))),
                "homeask" => {
                    if !arg.is_empty() { self.home.input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    Some(Cmd::Draft(crate::draft::Do::Send))
                }
                // `promote` is 建成文献库 for the conversation on show (the dialog; `submit` makes it one), `promote:propose` its sparkle;
                // `rename` the dialog that names the conversation anew.
                "promote" if arg == "propose" => Some(Cmd::Draft(crate::draft::Do::Propose)),
                "promote" => self.lib.as_ref().map(|lib| Cmd::Draft(crate::draft::Do::Promote(lib.info.id.clone()))),
                "rename" => self.lib.as_ref().map(|lib| Cmd::Draft(crate::draft::Do::Rename(lib.info.id.clone()))),
                "select" => self.lib.as_ref().and_then(|l| l.shown.get(arg.parse::<usize>().unwrap_or(0)).map(|&i| Cmd::Select(l.papers[i].id.clone()))),
                "tab" => Some(Cmd::SetTab(Tab::from_key(arg))),
                "page" => Some(Cmd::SetPage(Page::from_key(arg))),
                // `restore` goes back to where `session.json` says the window was, as a start that is not scripted does.
                "restore" => {
                    self.restore_session(window, cx);
                    return;
                }
                // `write:methods` shows another part of the manuscript; `draft:<text>` leaves text in it, as if typed.
                "write" => Some(Cmd::WriteSection(arg.to_string())),
                // `manuscript:new` makes another manuscript, `manuscript:<title>` gives the current one its title;
                // `section` / `section:<id>` is the dialog for a new section / one's name and kind;
                // `reading` shows the whole manuscript, `versions` the section's history.
                "manuscript" if arg == "new" => Some(Cmd::NewManuscript),
                // `manuscript:open;<n>` writes the n-th of the library's manuscripts, from 0.
                "manuscript" if arg.starts_with("open;") => self.lib.as_ref().and_then(|lib| lib.writing.manuscripts.get(arg[5..].parse::<usize>().ok()?)).map(|m| Cmd::WriteManuscript(m.id.clone())),
                "manuscript" => {
                    if let Some(lib) = self.lib.as_ref() { lib.writing.title.update(cx, |input, cx| input.set_text(arg, cx)) }
                    self.write_title_typed(arg.to_string(), cx);
                    None
                }
                "section" => Some(Cmd::EditSection((!arg.is_empty()).then(|| arg.to_string()))),
                // `subsection` asks for a subsection under the section being written; `indent` / `outdent` make it one level deeper / less deep.
                "subsection" => self.lib.as_ref().map(|lib| Cmd::AddSubsection(lib.writing.section_id.clone())),
                "indent" | "outdent" => self.lib.as_ref().map(|lib| Cmd::IndentSection(lib.writing.section_id.clone(), name == "indent")),
                // `split` / `split:heading` / `split:sub` cut the section at the caret's paragraph (that paragraph a new section's first / its heading / a subsection's heading); `merge` puts the section being written back into the one before;
                // `headings:<n>` / `headings:all` / `headings:none` tick in the dialog of a Word file without heading styles (`submit` imports).
                "split" => Some(Cmd::SplitSection(match arg {
                    "heading" => wisp_core::manuscript::Cut::Heading,
                    "sub" => wisp_core::manuscript::Cut::Subheading,
                    _ => wisp_core::manuscript::Cut::Paragraph,
                })),
                "merge" => self.lib.as_ref().map(|lib| Cmd::MergeSection(lib.writing.section_id.clone())),
                "headings" => Some(Cmd::Comment(match arg {
                    "all" => crate::comments::Do::Headings(true),
                    "none" => crate::comments::Do::Headings(false),
                    n => crate::comments::Do::Heading(n.parse().unwrap_or(0)),
                })),
                "reading" => Some(Cmd::WriteReading(true)),
                // 译文对照: `wfacing` reads the manuscript beside its translation, `wfacing:off` without, `wfacing:retry` asks again
                // for what failed, `wfacing:<code>` chooses the language (a code of `manuscript::LANGUAGES`).
                "wfacing" => Some(match arg { "" => Cmd::WriteFacing(true), "off" => Cmd::WriteFacing(false), "retry" => Cmd::WriteFacingRetry, code => Cmd::WriteFacingLanguage(code.to_string()) }),
                // 翻译 (write_translate.rs): `wtranslate:<code>` works on the version in that language, made when there is none;
                // by a unit's place among the manuscript's units from 0 (the title, each section's name and blocks): `edit;<n>`, `redo;<n>`,
                // `carry;<n>`, `take;<n>`, `leave;<n>`, `check;<n>`; `type;<text>` writes the unit edited, `done`, `next`, `retry`, `stop`,
                // `primary`, `remove`, `alone` / `together`; `body;<section from 0>;<text>` sets a section's text (¶ between paragraphs);
                // `dump;<file>` writes each unit's state and translation as JSON.
                "wtranslate" => {
                    self.dev_translate(arg, window, cx);
                    return;
                }
                "versions" => Some(Cmd::WriteHistory),
                // `importdoc:<path>` starts a manuscript from a Word or Markdown file; `comments` shows the comments beside the manuscript (`comments:off` the candidates);
                // `comment:<step>` is `review`, `stop`, `write`, `save`, `cancel`, `takeall`, `import`, `fromreview` (a manuscript of the newest review the assistant wrote sections for), and by a comment's place among those of the section from 0: `take;<n>`, `back;<n>`, `drop;<n>`, `done;<n>`, `show;<n>`;
                // `wish:<text>` / `commenttext:<text>` leave text in the review's box / the comment being written; `exportdoc:<path>` / `exportdoc:track;<path>` write the manuscript as a Word file there.
                // 移到文献库 (`copypaper`: 复制到文献库) for the marked papers or the selected one: the dialog; `moveto:<n>` chooses the n-th library it offers, `submit` moves or copies
                // (覆盖 for those the library holds already), `moveappend` with 追加.
                "movepaper" | "copypaper" => self.lib.as_ref().and_then(|lib| {
                    let ids: Vec<String> = if lib.marked.len() > 1 { lib.shown.iter().map(|&i| lib.papers[i].id.clone()).filter(|id| lib.marked.contains(id)).collect() } else { lib.selected.clone().into_iter().collect() };
                    if ids.is_empty() { return None }
                    Some(if name == "copypaper" { Cmd::AskCopyPapers(ids) } else { Cmd::AskMovePapers(ids) })
                }),
                "moveto" => self.move_targets().get(arg.parse::<usize>().unwrap_or(0)).map(|info| Cmd::ChooseMoveTarget(info.id.clone())),
                "moveappend" => match &self.modal {
                    Some(Modal::MoveTo { papers, to: Some(to), copy, .. }) => Some(if *copy { Cmd::CopyPapers(papers.clone(), to.clone(), wisp_core::transfer::Held::Append) } else { Cmd::MovePapers(papers.clone(), to.clone(), wisp_core::transfer::Held::Append) }),
                    _ => None,
                },
                // 归入已有文献 for the selected paper: the dialog; `fileto:<n>` chooses the n-th paper it offers, `submit` files; `undofiling` is the toast's 撤销.
                // `files:<n>` shows the files of the n-th paper on show under its row, or puts them away.
                "fileinto" => self.lib.as_ref().and_then(|lib| lib.selected.clone()).map(|id| Cmd::AskFileInto(vec![id])),
                "fileto" => self.file_targets(cx).get(arg.parse::<usize>().unwrap_or(0)).map(|id| Cmd::ChooseFileTarget(id.clone())),
                // `toastbutton` presses the button of whatever toast is up (打开 after 移到文献库); like a click, the toast goes.
                "undofiling" | "undotagging" | "toastbutton" => self.toast.take().and_then(|toast| toast.action).map(|(_, cmd)| cmd),
                // 给这 N 篇加标签 for the papers on show (`tagshown`) and 去掉标签 for the marked ones (`untag`): `tagtext:<tags>` writes
                // in the first dialog's box, `offertag:<tag>` is a click on a tag offered under it, `chooseuntag:<tag>` one on a tag of
                // the second; `submit` writes, `undotagging` is the toast's 撤销. `renametag:<tag>` is the sidebar's 重命名… for a tag,
                // its box written with `tagtext:` too.
                "tagshown" => Some(Cmd::TagShown),
                "renametag" => Some(Cmd::RenameTag(arg.to_string())),
                "tagtext" => {
                    if let Some(Modal::TagPapers { name, .. } | Modal::Tag { name, .. }) = &self.modal { name.update(cx, |input, cx| input.set_text(arg, cx)) }
                    None
                }
                "offertag" => Some(Cmd::OfferTag(arg.to_string())),
                "untag" => self.lib.as_ref().map(|lib| Cmd::UntagPapers(lib.marked.clone())),
                "chooseuntag" => Some(Cmd::ChooseUntag(arg.to_string())),
                "files" => self.lib.as_ref().and_then(|l| l.shown.get(arg.parse::<usize>().unwrap_or(0)).map(|&i| Cmd::UnfoldFiles(l.papers[i].id.clone()))),
                "importdoc" => {
                    self.import_manuscript_files(vec![PathBuf::from(arg)], cx);
                    None
                }
                "comments" => Some(Cmd::WriteSide(if arg == "off" { Side::Candidates } else { Side::Comments })),
                // `references` shows the reference list beside the manuscript (`references:off` the candidates), `references:whole` the whole manuscript's, as the outline's row does.
                "references" => Some(match arg { "off" => Cmd::WriteSide(Side::Candidates), "whole" => Cmd::WriteReferences, _ => Cmd::WriteSide(Side::References) }),
                // `glossary` shows 术语 beside the manuscript; `glossary:add;<中文>;<译名>`, `glossary:edit;<n>` (the n-th row shown, from 0),
                // `glossary:save`, `glossary:remove;<n>`, `glossary:propose`, `glossary:tick;<n>`, `glossary:take`, `glossary:lang;<code>`;
                // `requirement:<text>` types the manuscript's standing requirement.
                "glossary" => return self.glossary_dev(arg, window, cx),
                "requirement" => {
                    // Setting a field's text is not typing into it: it is kept as typing would keep it.
                    if let Some(lib) = self.lib.as_ref() { lib.glossary.requirement.update(cx, |input, cx| input.set_text(arg, cx)) }
                    self.requirement_typed(arg.to_string(), cx);
                    None
                }
                "wish" | "commenttext" => {
                    if let Some(comments) = self.lib.as_ref().map(|lib| &lib.writing.comments) { (if name == "wish" { &comments.wish } else { &comments.draft }).update(cx, |input, cx| input.set_text(arg, cx)) }
                    None
                }
                // `exportmd:<path>` writes the manuscript as Markdown there, its pictures in `figures/` beside it.
                "exportmd" => {
                    self.write_markdown_to(PathBuf::from(arg), cx);
                    None
                }
                "exportdoc" => {
                    let (track, path) = arg.strip_prefix("track;").map_or((false, arg), |path| (true, path));
                    self.write_word_to(PathBuf::from(path), track, cx);
                    None
                }
                "comment" => {
                    use crate::comments::Do;
                    let (what, n) = arg.split_once(';').map_or((arg, 0), |(what, n)| (what, n.parse::<usize>().unwrap_or(0)));
                    let nth = self.lib.as_ref().and_then(|lib| lib.writing.comments.list.iter().filter(|comment| comment.section_id == lib.writing.section_id).nth(n)).map(|comment| comment.id.clone()).unwrap_or_default();
                    Some(Cmd::Comment(match what {
                        "review" => Do::Review,
                        "stop" => Do::Stop,
                        "write" => Do::Write,
                        "save" => Do::Save,
                        "cancel" => Do::Cancel,
                        "takeall" => Do::TakeAll,
                        "import" => Do::Import,
                        "fromreview" => Do::FromReview(self.reviews_written().first().map(|(conversation, _, _)| conversation.clone()).unwrap_or_default()),
                        "take" => Do::Take(nth),
                        "back" => Do::Back(nth),
                        "drop" => Do::Drop(nth),
                        "done" => Do::Done(nth, true),
                        _ => Do::Show(nth),
                    }))
                }
                // `find` asks for candidates as the shortcut does; `find:<words>` puts words in the find box and presses Enter.
                "find" if arg.is_empty() => Some(Cmd::WriteFind),
                "find" => {
                    if let Some(lib) = self.lib.as_ref() { lib.writing.find.update(cx, |input, cx| input.set_text(arg, cx)) }
                    self.write_find(true, cx);
                    return;
                }
                // `unfold` shows the sentences around the first candidate.
                "unfold" => self.lib.as_ref().and_then(|lib| lib.writing.search.candidates.as_ref()?.as_ref().ok()?["sentences"].get(0).map(|s| Cmd::WriteUnfold(format!("{}-{}", s["paperId"].as_str().unwrap_or(""), s["position"].as_u64().unwrap_or(0))))),
                "only" => Some(Cmd::WriteOnly(arg.strip_prefix('*').unwrap_or(arg).to_string(), arg.starts_with('*'))),
                "draft" => {
                    if let Some(lib) = self.lib.as_ref() { lib.writing.editor.update(cx, |input, cx| input.insert(arg, cx)) }
                    return;
                }
                // `import:<folder>` takes in every PDF under a folder, as if it were dropped on the window:
                // the folders below it become tags.
                // As a file dropped on the window: on the home page a PDF is read (`open_file.rs`).
                "import" => {
                    self.drop_paths(vec![PathBuf::from(arg)], window, cx);
                    return;
                }
                // `openfile:<path>`: a file handed over as Finder's or Explorer's 打开方式 hands it.
                "openfile" => {
                    self.open_handed(vec![PathBuf::from(arg)], window, cx);
                    return;
                }
                // `shot:<file.png>` writes the window as it is drawn to a picture, without the screen:
                // the frame is rendered off it. macOS only.
                // `window` reads the paper on show in a window of its own, `window:<n>` the n-th paper of the list; `windowshot:<file.png>` draws the last such window.
                "window" if arg.is_empty() => self.lib.as_ref().and_then(|l| l.selected.clone()).map(Cmd::PaperWindow),
                "window" => self.lib.as_ref().and_then(|l| l.shown.get(arg.parse::<usize>().unwrap_or(0)).map(|&i| Cmd::PaperWindow(l.papers[i].id.clone()))),
                // `windowkey:cmd-f`: a key as the last paper window hears it.
                "windowkey" => {
                    let Some(handle) = self.lib.as_ref().and_then(|l| l.windows.last()).map(|w| AnyWindowHandle::from(w.window)) else { eprintln!("windowkey: no paper window"); return };
                    if let Ok(stroke) = Keystroke::parse(arg) { cx.defer(move |cx| { handle.update(cx, |_, window, cx| { window.dispatch_keystroke(stroke, cx); }).ok(); }) }
                    return;
                }
                "windowshot" => {
                    let path = PathBuf::from(arg);
                    // Untyped: a typed handle would hold the window's root view, which drawing it needs.
                    let Some(handle) = self.lib.as_ref().and_then(|l| l.windows.last()).map(|w| AnyWindowHandle::from(w.window)) else { eprintln!("windowshot: no paper window"); return };
                    cx.defer(move |cx| {
                        handle.update(cx, |_, window, cx| {
                            let Some((width, height, mut pixels)) = window.render_offscreen(cx) else { return eprintln!("windowshot: this platform cannot render off the screen") };
                            for pixel in pixels.as_chunks_mut::<4>().0 { pixel.swap(0, 2) }
                            match image::RgbaImage::from_raw(width as u32, height as u32, pixels).map(|picture| picture.save(&path)) {
                                Some(Ok(())) => eprintln!("windowshot: {}", path.display()),
                                other => eprintln!("windowshot: not written ({other:?})"),
                            }
                        })
                        .ok();
                    });
                    return;
                }
                "shot" => {
                    let path = PathBuf::from(arg);
                    window.defer(cx, move |window, cx| {
                        let Some((width, height, mut pixels)) = window.render_offscreen(cx) else { return eprintln!("shot: this platform cannot render off the screen") };
                        for pixel in pixels.as_chunks_mut::<4>().0 {
                            pixel.swap(0, 2);
                        }
                        match image::RgbaImage::from_raw(width as u32, height as u32, pixels).map(|picture| picture.save(&path)) {
                            Some(Ok(())) => eprintln!("shot: {}", path.display()),
                            other => eprintln!("shot: not written ({other:?})"),
                        }
                    });
                    return;
                }
                // The pointer, as the window hears of it: `press:x;y`, `move:x;y` (the button held), `release:x;y`,
                // `click:x;y`, `click2:x;y` / `click3:x;y` (the second click of a double click, the third of a triple), and `wheel:x;y;dy;cmd` (dy in pixels;
                // `cmd` or `shift` held, or nothing).
                // A key as the window hears it (`key:down`, `key:cmd-backspace`).
                "key" => {
                    if let Ok(stroke) = Keystroke::parse(arg) { window.defer(cx, move |window, cx| { window.dispatch_keystroke(stroke, cx); }) }
                    return;
                }
                "press" | "move" | "hover" | "release" | "click" | "click2" | "click3" | "rclick" | "wheel" => {
                    let mut numbers = arg.split(';');
                    let mut number = || numbers.next().and_then(|n| n.parse::<f32>().ok()).unwrap_or(0.);
                    let position = point(px(number()), px(number()));
                    let turned = number();
                    let held = arg.rsplit(';').next().unwrap_or("");
                    let modifiers = Modifiers { platform: held == "cmd", control: held == "ctrl", shift: held == "shift", ..Default::default() };
                    let count = match name { "click2" => 2, "click3" => 3, _ => 1 };
                    let events = match name {
                        "press" => vec![PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position, modifiers, click_count: 1, first_mouse: false })],
                        "move" => vec![PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: Some(MouseButton::Left), modifiers })],
                        "hover" => vec![PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers })],
                        "release" => vec![PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers, click_count: 1 })],
                        "rclick" => vec![PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers }), PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Right, position, modifiers, click_count: 1, first_mouse: false }), PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Right, position, modifiers, click_count: 1 })],
                        "wheel" => vec![PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers }), PlatformInput::ScrollWheel(ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(turned))), modifiers, touch_phase: TouchPhase::Moved })],
                        _ => vec![
                            PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers }),
                            PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position, modifiers, click_count: count, first_mouse: false }),
                            PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers, click_count: count }),
                        ],
                    };
                    window.defer(cx, move |window, cx| {
                        for event in events {
                            window.dispatch_event(event, cx);
                        }
                    });
                    return;
                }
                "textless" => Some(Cmd::ShowTextless),
                // `findpdf` asks about the papers of the list on show, `findpdf:one` about the selected paper; `fetched` shows what came of it.
                "findpdf" => Some(Cmd::FindPdfs(if arg == "one" { self.lib.as_ref().and_then(|lib| lib.selected.clone()) } else { None })),
                "fetched" => Some(Cmd::ShowFetch),
                // A network-free preview of a mixed batch in an isolated demo library.
                "fetch-preview" => {
                    let Some(lib) = self.lib.as_mut() else { return };
                    let total = lib.papers.len().min(428);
                    let done = if arg == "complete" { total } else if arg == "empty" { 0 } else { total / 3 };
                    let mut run = FetchRun { total, run: lib.fetching.as_ref().map_or(1, |run| run.run + 1), ..Default::default() };
                    for (i, paper) in lib.papers.iter().take(total).enumerate() {
                        if i < done {
                            let (outcome, detail) = match i % 4 {
                                0 => ("saved", "来自 OpenAlex · 正式版，读出 12 页文字"),
                                1 => ("failed", "出版社返回 HTTP 403，拒绝程序下载。可在浏览器中下载 PDF 后拖入文献库。"),
                                _ => ("not_found", "本次检索未找到可下载的 PDF 地址。"),
                            };
                            run.rows.push(FetchRow { paper_id: paper.id.clone(), title: paper.title.clone(), outcome: outcome.into(), detail: detail.into(), textless: false });
                        } else if run.working.len() < FETCH_WORKERS && arg != "stopped" {
                            run.working.push(paper.id.clone());
                        } else {
                            run.todo.push(paper.id.clone());
                        }
                    }
                    if arg == "stopped" { run.stop() }
                    lib.fetching = Some(run);
                    cx.notify();
                    None
                }
                "view" => Some(Cmd::SetView(match arg { "nopdf" => View::NoPdf, "unread" => View::Unread, _ => View::All })),
                "import-preview" | "zotero-preview" => {
                    let Some(lib) = self.lib.as_mut() else { return };
                    let total = lib.papers.len().min(24);
                    let done = if arg == "complete" { total } else { total / 2 };
                    let mut pdfs = ImportRun { total, ..Default::default() };
                    let mut zotero = ZoteroRun { total, running: done < total, ..Default::default() };
                    for (i, paper) in lib.papers.iter().take(total).enumerate() {
                        let file = format!("{}.pdf", paper.title);
                        if i < done {
                            let failed = i % 5 == 4;
                            let skipped = i % 5 == 3;
                            let incomplete = i % 5 == 2;
                            pdfs.rows.push(ImportRow { name: file, outcome: if failed { Imported::Failed("无法读取这个文件".into()) } else if skipped { Imported::InTrash } else if i % 5 == 1 { Imported::Attached } else { Imported::Added }, paper_id: (!failed).then(|| paper.id.clone()), unrecognised: incomplete, textless: String::new(), no_pdf: String::new() });
                            zotero.rows.push(zotero::Row { title: paper.title.clone(), paper_id: (!failed && !skipped).then(|| paper.id.clone()), added: i % 5 == 0, in_trash: skipped, files_added: usize::from(!failed && !skipped), problems: if failed { vec!["无法读取文献记录".into()] } else if incomplete { vec!["附件尚未下载到本机".into()] } else { vec![] }, ..Default::default() });
                        } else if pdfs.working.len() < IMPORT_WORKERS {
                            pdfs.working.push(file);
                            zotero.current = paper.title.clone();
                        }
                    }
                    if name == "import-preview" { lib.import = Some(pdfs) } else { lib.zotero = Some(zotero) }
                    cx.notify();
                    None
                }
                "materials" => Some(Cmd::ToggleMaterials),
                // `scopefind:xylem` searches in the panel over the library's conversation, as if typed; `pin:2` keeps the next conversation to the third paper it lists (again lets it go).
                "scopefind" => {
                    if let Some(lib) = self.lib.as_ref() { lib.scope_find.update(cx, |input, cx| input.set_text(arg, cx)) }
                    self.scope_typed(arg.to_string(), cx);
                    None
                }
                "pin" => self.lib.as_ref().and_then(|lib| {
                    let id = lib.scope_listed().get(arg.parse::<usize>().unwrap_or(0))?.id.clone();
                    Some(Cmd::Scope(if lib.asked.contains(&id) { ScopeDo::Unpin(id) } else { ScopeDo::Pin(id) }))
                }),
                // `review:requirements|framework|outline|section|checks` takes the last answer for that step, `review:confirm` confirms what the step waits for, `review:skip` goes on without a framework,
                // `review:record` shows the record or the conversation, `review:judge` says the first row of evidence is borne out, `review:manuscript` makes the manuscript.
                "review" => self.lib.as_ref().and_then(|lib| {
                    let answer = lib.library_chat.messages.iter().rev().find_map(|item| if let ChatItem::Answer(text) = item { Some(text.clone()) } else { None }).unwrap_or_default();
                    let review = lib.library_chat.review.as_ref();
                    match arg {
                        "requirements" => Some(Cmd::Review(ReviewDo::Requirements, answer)),
                        "framework" => Some(Cmd::Review(ReviewDo::Framework, answer)),
                        "skip" => Some(Cmd::Review(ReviewDo::SkipFramework, String::new())),
                        "outline" => Some(Cmd::Review(ReviewDo::Outline, answer)),
                        "section" => Some(Cmd::Review(ReviewDo::Section, answer)),
                        "checks" => Some(Cmd::Review(ReviewDo::Checks, answer)),
                        "confirm" => Some(Cmd::Review(match review.map_or(1, |review| review.step()) { 1 => ReviewDo::ConfirmRequirements, 2 => ReviewDo::ConfirmFramework, _ => ReviewDo::ConfirmOutline }, String::new())),
                        "complete" => Some(Cmd::Review(ReviewDo::Complete, String::new())),
                        "record" => Some(Cmd::ToggleReview),
                        "judge" => review.and_then(|review| review.evidence_rows.first()).map(|row| Cmd::JudgeReviewEvidence(row.id.clone(), wisp_core::review::EvidenceJudgment::Supports)),
                        "evidence" => Some(Cmd::EditReviewEvidence(None, String::new())),
                        "edit" => Some(Cmd::EditReviewText(ReviewText::Requirements)),
                        "manuscript" => Some(Cmd::ReviewToManuscript),
                        _ => Some(Cmd::StartReview),
                    }
                }),
                "history-paper" => Some(Cmd::ToggleHistory(Talk::Paper)),
                "newchat" => Some(Cmd::NewChat(match arg { "paper" => Talk::Paper, "write" => Talk::Write, _ => Talk::Library })),
                "note" => {
                    let Some(answer) = self.lib.as_ref().and_then(|lib| lib.paper_chat.messages.iter().rev().find_map(|item| if let ChatItem::Answer(text) = item { Some(text.clone()) } else { None })) else { return };
                    Some(Cmd::AnswerToNote("（脚本）".into(), answer))
                }
                "exportnote" => Some(Cmd::NoteOpen(match arg { "paper" => Some(Talk::Paper), "library" => Some(Talk::Library), _ => None }, true)),
                "notepreview" => Some(Cmd::NotePreview),
                "notedestination" => Some(Cmd::NoteDestination(arg != "local")),
                "notepick" => arg.parse().ok().map(Cmd::NotePick),
                "submit" => Some(Cmd::SubmitModal),
                "reread" | "byeye" | "compress" => {
                    let Some(file) = self.lib.as_ref().and_then(|lib| lib.selected.as_deref().and_then(|id| lib.pdf_file(id)).map(|file| file.id.clone())) else { return };
                    Some(match name { "reread" => Cmd::RereadFile(file), "compress" => Cmd::CompressFile(file), _ => Cmd::ReadPages(file) })
                }
                // `swapfile:<path>` puts the file at the path in the place of the selected paper's file on show (换成另一个文件…, without the dialog).
                "swapfile" => {
                    let Some(file) = self.lib.as_ref().and_then(|lib| lib.selected.as_deref().and_then(|id| lib.pdf_file(id)).map(|file| file.id.clone())) else { return };
                    return self.swap_file(file, std::path::Path::new(arg), cx);
                }
                // `filenote:<n>` opens the note of the selected paper's n-th file (in the detail panel's order), `filenote:<n>;<words>` with the words written in; `submit` keeps it.
                "filenote" => {
                    let (n, words) = arg.split_once(';').unwrap_or((arg, ""));
                    let Some(file) = self.lib.as_ref().and_then(|lib| lib.selected.as_deref().and_then(|id| lib.paper_files(id).get(n.parse::<usize>().unwrap_or(0)).map(|f| f.id.clone()))) else { return };
                    self.file_note_dialog(file, window, cx);
                    if let Some(Modal::FileNote { text, .. }) = &self.modal && !words.is_empty() { text.update(cx, |input, cx| input.set_text(words, cx)) }
                    return;
                }
                // The file on show with its marks, written to a path without the save dialog.
                "exportpdf" => {
                    let Some(file) = self.lib.as_ref().and_then(|lib| lib.pdf.as_ref()).map(|view| view.read(cx).resource_id().to_string()) else { return };
                    return self.export_marked_pdf(file, Some(PathBuf::from(arg)), cx);
                }
                "width" => {
                    self.settings.detail_width = arg.parse().unwrap_or(0.);
                    cx.notify();
                    return;
                }
                // `readmode` gives the selected paper the whole window, or brings the library back; `chatwidth:400` is the conversation's width beside the PDF.
                "readmode" => Some(Cmd::ToggleReading),
                "chatwidth" => {
                    self.settings.chat_width = arg.parse().unwrap_or(0.);
                    cx.notify();
                    return;
                }
                // `asklist:240` is the width of the list beside a paper that is asked about (`tab:ask`).
                "asklist" => {
                    self.settings.ask_list_width = arg.parse().unwrap_or(0.);
                    cx.notify();
                    return;
                }
                // `column:journal=240` gives a column of the paper table a width (`year`, `fulltext`, `reading`, `screening`, `field:<id>`).
                "column" => {
                    if let Some((key, width)) = arg.split_once('=') { self.settings.column_widths.insert(key.into(), width.parse().unwrap_or(0.)); }
                    cx.notify();
                    return;
                }
                // `sidebar` puts the sidebar away or brings it back; `sidebar:300` gives it a width.
                "sidebar" if arg.is_empty() => Some(Cmd::ToggleSidebar),
                "sidebar" => {
                    self.settings.sidebar_width = arg.parse().unwrap_or(0.);
                    cx.notify();
                    return;
                }
                // `report` shows what came of the last import.
                "report" => Some(Cmd::ShowImport),
                "zotero" => {
                    self.modal = Some(Modal::Zotero { open: false });
                    self.zotero_picked(PathBuf::from(arg), cx);
                    None
                }
                "zotero-report" => Some(Cmd::ShowZotero),
                // `screen` starts the assistant on the pending papers, `screen:decided` on the decided ones, `screen:included` on the included ones;
                // `screen:unincluded` asks to move the decided papers not included to the trash.
                // `screen:single`, `screen:confirm` and `screen:batch` are the page's three faces; `screen:go;<n>` the n-th paper of a round;
                // `screen:close` the × of 确认's row in full under the list; `screen:undo` takes the visit's last step back, as Z does.
                // 筛选记录 is `menu:record`.
                "screen" => Some(match arg {
                    "decided" => Cmd::ScreenRecheck(false),
                    "included" => Cmd::ScreenRecheck(true),
                    "unincluded" => Cmd::AskTrashUnincluded,
                    "single" => Cmd::ScreenMode(ScreenMode::Single),
                    "confirm" => Cmd::ScreenMode(ScreenMode::Confirm),
                    "batch" => Cmd::ScreenMode(ScreenMode::Batch),
                    "apply" => Cmd::ConfirmApply,
                    "unconfirm" => Cmd::ConfirmUndo,
                    "close" => Cmd::ConfirmClose,
                    "undo" => Cmd::ScreenUndo,
                    go if go.starts_with("go;") => Cmd::ScreenGo(go[3..].parse().unwrap_or(0)),
                    _ => Cmd::ScreenStart,
                }),
                // 提取: `extract` is the dialog for the list on show (`extract:marked` for the marked papers), `submit` starts it;
                // the rest are the steps of `Workspace::extract_dev` (extract.rs).
                "extract" => self.extract_dev(arg),
                // `goal:<text>` writes the screening goal, as if typed.
                "goal" => {
                    if let Some(lib) = self.lib.as_mut() {
                        lib.screen.criteria.update(cx, |input, cx| input.set_text(arg, cx));
                        lib.info.criteria = arg.trim().to_string();
                        libraries::save(&lib.info).ok();
                    }
                    None
                }
                // `sort` has the model sort the full texts into parts, `read` has it read the pages of those without text.
                "sort" => Some(Cmd::WriteRun(false)),
                // `assist:rewrite|translate|polish|<own command>` asks the model about the caret's paragraph;
                // `assisted:replace|below|copy|drop` uses the result, `assiststop` stops it; `caret:<byte>` / `pick:<from>;<to>` place the caret / select.
                // `assist:reword` is 换个说法; `assistto:<code>` asks 译成… again into that language; `once:<text>` types a requirement
                // for this once under the card and asks again with it; `assistrecord` is 记入术语表 beside it.
                "assist" => Some(Cmd::WriteAssist(match arg { "rewrite" => ai::Assist::Rewrite, "translate" => ai::Assist::Translate, "reword" => ai::Assist::Reword, "polish" => ai::Assist::Polish, "cite" => ai::Assist::Cite, own => ai::Assist::Own(own.to_string()) })),
                "assistto" => Some(Cmd::WriteAssistTarget(arg.to_string())),
                "once" => {
                    if let Some(lib) = self.lib.as_ref() { lib.writing.assist.once.update(cx, |input, cx| input.set_text(arg, cx)) }
                    if !arg.is_empty() { self.assist_again(cx) }
                    None
                }
                "assistrecord" => Some(Cmd::WriteAssistRecord),
                // `compare:<id>;<id>` chooses the models 译成… and 换个说法 compare (their ids in settings.json); `compare:` none.
                "compare" => {
                    self.settings.compare_models = arg.split(';').map(str::trim).filter(|id| !id.is_empty()).map(str::to_string).collect();
                    self.save_settings(cx);
                    None
                }
                "assiststop" => Some(Cmd::WriteAssistStop),
                // `cited:tick;<sentence>;<paper>` ticks a paper 配引用 found (by their places from 0), `cited:more;<sentence>` shows its other papers,
                // `cited:rest` the sentences nothing was chosen for, `cited:insert` writes the ticked citations.
                "cited" => {
                    use crate::write_cite::Do;
                    let mut parts = arg.split(';');
                    let what = parts.next().unwrap_or("");
                    let mut number = || parts.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                    Some(Cmd::WriteCited(match what {
                        "tick" => Do::Tick(number(), number()),
                        "more" => Do::Unfold(number()),
                        "rest" => Do::Rest,
                        _ => Do::Insert,
                    }))
                }
                // `cite:<n>` is 引用 on the n-th candidate (from 0): the paper cited at the caret, the sentence kept as what the citation rests on.
                "cite" => self.lib.as_ref().and_then(|lib| {
                    let found = lib.writing.search.candidates.as_ref()?.as_ref().ok()?;
                    let sentence = found["sentences"].get(arg.parse::<usize>().unwrap_or(0))?;
                    Some(Cmd::WriteCiteFrom(sentence["paperId"].as_str()?.to_string(), wisp_core::passage::Candidate { text: sentence["text"].as_str()?.to_string(), part: found["section"].as_str().unwrap_or("").to_string(), page: sentence["page"].as_u64().unwrap_or(0) as usize }))
                }),
                // `source` looks for the passages of the citations that have none, `source:stop` stops that;
                // `source:go;<n>` shows the n-th passage kept (from 0) in its paper, `source:drop;<n>` lets go of it.
                "source" => {
                    use crate::write_source::Do;
                    let (what, n) = arg.split_once(';').map_or((arg, 0), |(what, n)| (what, n.parse::<usize>().unwrap_or(0)));
                    let nth = self.lib.as_ref().and_then(|lib| lib.citing.sources.kept.get(n)).cloned();
                    match (what, nth) {
                        ("stop", _) => Some(Cmd::Source(Do::Stop)),
                        ("go", Some(passage)) => Some(Cmd::GotoSource(passage.paper_id.clone(), (passage.page > 0).then_some(passage.page), passage.passage.clone())),
                        ("drop", Some(passage)) => Some(Cmd::Source(Do::Drop(passage.id.clone()))),
                        ("go" | "drop", None) => None,
                        _ => Some(Cmd::Source(Do::Find(None))),
                    }
                }
                // The list of what the assistant changed in the section on show (助手写): `revise:keep`, `revise:undo` (all) /
                // `revise:undo;<n>`, `revise:go;<n>`, `revise:next` / `revise:prev` are its buttons.
                "revise" => {
                    use crate::revise::Do;
                    let (what, n) = arg.split_once(';').map_or((arg, 0), |(what, n)| (what, n.parse::<usize>().unwrap_or(0)));
                    Some(Cmd::Revise(match (what, arg.contains(';')) {
                        ("keep", _) => Do::Keep,
                        ("undo", false) => Do::UndoAll,
                        ("undo", true) => Do::Undo(n),
                        ("go", _) => Do::Go(n),
                        ("next", _) => Do::Step(true),
                        ("prev", _) => Do::Step(false),
                        _ => return,
                    }))
                }
                // `assisted:take;<n>` takes the n-th version (from 0), `assisted:copy;<n>` copies it.
                "assisted" => {
                    use crate::write_assist::Use;
                    let (what, n) = arg.split_once(';').map_or((arg, None), |(what, n)| (what, n.parse::<usize>().ok()));
                    Some(Cmd::WriteAssistUse(match (what, n) { ("take", Some(n)) => Use::Take(n), ("copy", Some(n)) => Use::CopyOne(n), ("replace", _) => Use::Replace, ("below", _) => Use::Below, ("copy", _) => Use::Copy, _ => Use::Drop }))
                }
                "undo" => {
                    if let Some(lib) = self.lib.as_ref() { lib.writing.editor.update(cx, |input, cx| input.undo_last(cx)) }
                    return;
                }
                "caret" | "pick" => {
                    let (from, to) = arg.split_once(';').unwrap_or((arg, arg));
                    let (from, to) = (from.parse().unwrap_or(0), to.parse().unwrap_or(0));
                    if let Some(lib) = self.lib.as_ref() { lib.writing.editor.update(cx, |input, cx| input.select_range(from..to, cx)) }
                    return;
                }
                "read" => Some(Cmd::WriteRun(true)),
                // `guide:new` is what a new library shows; `guide:search` goes straight to composing, `guide:go` also generates.
                "guide" => {
                    self.run(Cmd::OpenGuide(arg == "new"), window, cx);
                    (arg == "go").then_some(Cmd::GuideGenerate)
                }
                // `quote:3|a sentence` opens the selected paper's PDF at page 3 with that sentence highlighted.
                // `quote:S<n>;<page>|<sentence>` is the same in the n-th attachment, as a citation of it is followed.
                "quote" => self.lib.as_ref().and_then(|l| {
                    let id = l.selected.clone()?;
                    let (page, quote) = arg.split_once('|')?;
                    let (file, page) = match page.strip_prefix('S').and_then(|rest| rest.split_once(';')) {
                        Some((n, page)) => (Some(l.attachments(&id).get(n.parse::<usize>().ok()?.checked_sub(1)?)?.id.clone()), page),
                        None => (None, page),
                    };
                    Some(Cmd::GotoFile(id, file, page.parse().ok(), quote.to_string()))
                }),
                // `pdfpick:<page>;<from>;<to>` selects glyphs of a 1-based page of the PDF on show (after a `shot:` has drawn it);
                // `pdftranslate` translates the selection, `pdftranslate:<n>` with the n-th model for this once.
                "pdfpick" => {
                    let numbers: Vec<usize> = arg.split(';').filter_map(|n| n.parse().ok()).collect();
                    if let (Some(pdf), [page, from, to]) = (self.lib.as_ref().and_then(|l| l.pdf.clone()), numbers.as_slice()) && !pdf.update(cx, |pdf, cx| pdf.select_marks(page.saturating_sub(1), *from, *to, cx)) {
                        eprintln!("pdfpick: the glyphs of page {page} are not at hand yet");
                    }
                    return;
                }
                // `pdffig:<page>;<n>` shows the figure the n-th mention (from 0) of a 1-based page of the PDF on show leads to,
                // as a click on the number does (after a `shot:` has drawn the page and the file's figures are read);
                // `pdffig:pin` keeps it in the other half, `pdffig:go` goes to it, `pdffig:back` comes back;
                // `pdffig:pick;<n>` chooses the n-th of the figures the card lists.
                "pdffig" => {
                    let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) else { return };
                    match arg {
                        "pin" => pdf.update(cx, |pdf, cx| pdf.pin_figure(cx)),
                        "go" => pdf.update(cx, |pdf, cx| pdf.go_to_figure(cx)),
                        "back" => pdf.update(cx, |pdf, cx| pdf.return_back(cx)),
                        _ if arg.starts_with("pick;") => pdf.update(cx, |pdf, cx| pdf.pick_figure(arg[5..].parse().unwrap_or(0), cx)),
                        // 相关文字 of the figure on show, then the n-th paragraph it lists: gone to, or kept beside.
                        "said" => pdf.update(cx, |pdf, cx| pdf.show_said(cx)),
                        _ if arg.starts_with("said;") => pdf.update(cx, |pdf, cx| pdf.go_to_said(arg[5..].parse().unwrap_or(0), cx)),
                        _ if arg.starts_with("saidpin;") => pdf.update(cx, |pdf, cx| pdf.pin_said(arg[8..].parse().unwrap_or(0), cx)),
                        _ => {
                            let numbers: Vec<usize> = arg.split(';').filter_map(|n| n.parse().ok()).collect();
                            if let [page, n] = numbers.as_slice() && !pdf.update(cx, |pdf, cx| pdf.open_figure_mention(page.saturating_sub(1), *n, cx)) {
                                eprintln!("pdffig: page {page} has no such mention of a figure, or it is not read yet");
                            }
                        }
                    }
                    return;
                }
                // `pdfview:<n>;pages|theirs|before|after|close|menu`: the label of the n-th view (from 0) of the PDF on show:
                // it shows the pages / their translation, changes places with the view before / after it, is closed, or opens its menu.
                // `pdfmap`: 导图 on the PDF's bar. `pdfmap:add` makes what is selected a node (after `pdfpick:`), `pdfmap:draft`
                // asks the model for a draft, and anything else is a step in the map (`MindMapView::script`: `child:<words>`,
                // `sibling:<words>`, `remove`, `fold`, `indent`, `outdent`, `undo`, `redo`, `up`, `down`, `in`, `out`, `zoom:<in|out|fit|percent>`,
                // `move:<id>;before|under|after`).
                "pdfmap" => {
                    let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) else { return };
                    match arg {
                        "" => return self.run(Cmd::PdfMap, window, cx),
                        "add" => pdf.update(cx, |pdf, cx| pdf.map_selection(cx)),
                        _ => {
                            let Some(map) = self.ensure_map(&pdf, cx) else { return };
                            if arg == "draft" { self.draft_map(&pdf, &map, cx) } else { map.update(cx, |map, cx| map.script(arg, cx)) }
                        }
                    }
                    return;
                }
                "pdfview" => {
                    let (view, what) = arg.split_once(';').unwrap_or((arg, "menu"));
                    let view = view.parse::<usize>().unwrap_or(0);
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) {
                        pdf.update(cx, |pdf, cx| match what {
                            "pages" => pdf.show_in_view(view, Shows::Pages, cx),
                            "theirs" => pdf.show_in_view(view, Shows::Theirs, cx),
                            "map" => pdf.show_in_view(view, Shows::Map, cx),
                            "before" => pdf.move_view(view, view.saturating_sub(1), cx),
                            "after" => pdf.move_view(view, view + 1, cx),
                            "close" => pdf.close_view(view, cx),
                            _ => pdf.open_view_menu(view, cx),
                        })
                    }
                    return;
                }
                // `pdfquote` hands the PDF's selection to the paper's conversation, as 添加到对话 does; `pdfquote:library` to the library's.
                "pdfquote" => {
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.quote(arg == "library", cx)) }
                    return;
                }
                "pdftranslate" => {
                    let profile = arg.parse::<usize>().ok().and_then(|n| self.settings.profiles().into_iter().nth(n)).map(|p| p.id);
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.translate(profile, cx)) }
                    return;
                }
                // `try:0` runs the first saved search as a trial.
                "try" => self.lib.as_ref().and_then(|l| l.searches.get(arg.parse::<usize>().unwrap_or(0)).map(|s| Cmd::TrySearch(s.id.clone()))),
                // `runsearch:0` is 运行并导入 of the first saved search, `takeall:0` its 取回全部, `editsearch:0` its dialog.
                "runsearch" | "takeall" | "editsearch" => self.lib.as_ref().and_then(|l| l.searches.get(arg.parse::<usize>().unwrap_or(0))).map(|s| match name {
                    "runsearch" => Cmd::RunSearch(s.id.clone()),
                    "takeall" => Cmd::TakeAllHits(s.id.clone()),
                    _ => Cmd::EditSearch(s.id.clone()),
                }),
                "wide" => Some(Cmd::ToggleWide),
                "cut" => Some(Cmd::PdfCut),
                // `readaloud` reads the PDF on show aloud from the page in view (again stops it); `readaloud:selection` reads what is selected on it.
                "readaloud" if arg == "selection" => {
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.read_selection_aloud(cx)) }
                    return;
                }
                "readaloud" => Some(Cmd::ReadAloud),
                // `podcast` plays the podcast of the PDF on show: the one kept, else two hosts talk it over anew (again pauses
                // or goes on, as `pause` does); `podcast:new` makes it anew without asking. `seek:<0..1>` plays it from that
                // share of what was made, `speed:<n>` at the n-th of `playback::SPEEDS`.
                "podcast" if arg == "new" => Some(Cmd::NewPodcast),
                "podcast" => Some(Cmd::Podcast),
                "pause" => Some(Cmd::PauseReading),
                "speed" => Some(Cmd::PlaySpeed(arg.parse().unwrap_or(1))),
                "seek" => {
                    if let Some(reading) = self.lib.as_ref().and_then(|lib| lib.aloud.as_ref()) { reading.seek((arg.parse::<f64>().unwrap_or(0.) * reading.status().length as f64) as usize) }
                    return;
                }
                // `pdftool:box` / `pdftool:note` / `pdftool:text` draw boxes / place a note / write a text box on the PDF on show; again, or `pdftool:off`, selects text.
                "pdftool" => Some(Cmd::PdfTool(match arg { "box" => crate::pdf::Tool::Box, "note" => crate::pdf::Tool::Note, "text" => crate::pdf::Tool::Text, _ => crate::pdf::Tool::Select })),
                // `pdfwrite:<words>` writes in the text box being written on the PDF on show, as if typed (`\n` breaks the line).
                "pdfwrite" => {
                    if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.dev_write(arg, cx)) }
                    return;
                }
                // `readfile:<n>` reads the n-th PDF of the selected paper (0 its main text, then its attachments) in the viewer.
                "readfile" => self.lib.as_ref().and_then(|lib| {
                    let paper = lib.selected.as_deref()?;
                    let files: Vec<String> = lib.main_pdf(paper).into_iter().chain(lib.attachments(paper)).map(|f| f.id.clone()).collect();
                    files.get(arg.parse::<usize>().unwrap_or(0)).cloned().map(Cmd::ReadFile)
                }),
                // `pdfbeside:<n>` shows the n-th PDF of the selected paper (counted as `readfile:` does) in the other half of the viewer.
                // `pane` marks the paper on show to stay beside the next one selected (留在旁边), `pane:<n>` pins the n-th paper of the list beside it; `paneclose:<n>` closes the n-th such pane, `panework:<n>` makes it the paper on show where it is (a press in it).
                "pane" if arg.is_empty() => Some(Cmd::KeepPane),
                "pane" => self.lib.as_ref().and_then(|l| l.shown.get(arg.parse::<usize>().unwrap_or(0)).map(|&i| Cmd::PanePaper(l.papers[i].id.clone()))),
                "panework" => self.lib.as_ref().and_then(|l| l.pins.get(arg.parse::<usize>().unwrap_or(0)).map(|pin| Cmd::WorkInPane(pin.paper.clone()))),
                "layout" => wisp_core::panes::Mode::from_key(arg).map(Cmd::Layout),
                "paneclose" => self.lib.as_ref().and_then(|l| l.pins.get(arg.parse::<usize>().unwrap_or(0)).map(|pin| Cmd::ClosePane(pin.paper.clone()))),
                "pdfbeside" => self.lib.as_ref().and_then(|lib| lib.selected.as_deref().and_then(|paper| lib.pdf_choices(paper).into_iter().nth(arg.parse::<usize>().unwrap_or(0)))).map(|(id, _)| Cmd::PdfBeside(id)),
                // `pdfcut:<page>;<x0>;<y0>;<x1>;<y1>` marks a piece of a 1-based page of the PDF on show as cut out, in the page's
                // points (after a `shot:` has drawn the page); `collect` hands it to the figure library, as 收藏到图片库 does.
                "pdfcut" => {
                    let numbers: Vec<f32> = arg.split(';').filter_map(|n| n.parse().ok()).collect();
                    if let (Some(pdf), [page, x0, y0, x1, y1]) = (self.lib.as_ref().and_then(|l| l.pdf.clone()), numbers.as_slice()) { pdf.update(cx, |pdf, cx| pdf.set_cut((*page as usize).saturating_sub(1), [*x0, *y0, *x1, *y1], cx)) }
                    return;
                }
                "collect" => {
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.collect_cut(cx)) }
                    return;
                }
                // `figure:<n>` opens the n-th figure of the figure library, `figure:source:<n>` the place it was cut from;
                // `figtag:<tag>` narrows the figures to a tag, `figfind:<words>` searches them.
                "figure" => {
                    let (source, n) = arg.strip_prefix("source:").map_or((false, arg), |n| (true, n));
                    self.lib.as_ref().and_then(|l| l.gallery.items.get(n.parse::<usize>().unwrap_or(0))).map(|figure| if source { Cmd::FigureSource(figure.id.clone()) } else { Cmd::OpenFigure(figure.id.clone()) })
                }
                "figtag" => Some(Cmd::FigureTag(arg.to_string())),
                // `method:<n>` / `intro:<n>` opens the n-th method of 方法库 / introduction of 导言库, `method:new` / `intro:new` the
                // dialog of one written down, `method:source:<n>` / `method:ask:<n>` the place its words stand / the library's
                // conversation with it; `methodtag:<tag>` / `introtag:<tag>` narrows that page, `methodfind:<words>` /
                // `introfind:<words>` searches it; in a method's dialog `methodpick` opens or closes the list of figures,
                // `methodlink:<n>` links the n-th figure of the library; `excerptname:<text>` is typed into the dialog's name.
                "method" | "intro" => {
                    let kind = if name == "intro" { wisp_core::excerpt::INTRO } else { wisp_core::excerpt::METHOD };
                    let nth = |this: &Self, n: &str| this.lib.as_ref().and_then(|l| l.excerpts.items.iter().filter(|e| e.kind == kind).nth(n.parse::<usize>().unwrap_or(0))).map(|e| e.id.clone());
                    use crate::excerpts::Do;
                    match arg.split_once(':') {
                        _ if arg == "new" => Some(Cmd::Excerpt(Do::New(kind.into()))),
                        Some(("source", n)) => nth(self, n).map(|id| Cmd::Excerpt(Do::Source(id))),
                        Some(("ask", n)) => nth(self, n).map(|id| Cmd::Excerpt(Do::Ask(id))),
                        _ => nth(self, arg).map(|id| Cmd::Excerpt(Do::Open(id))),
                    }
                }
                "methodtag" | "introtag" => Some(Cmd::Excerpt(crate::excerpts::Do::Tag(if name == "introtag" { wisp_core::excerpt::INTRO } else { wisp_core::excerpt::METHOD }.into(), arg.to_string()))),
                "methodfind" | "introfind" => {
                    let kind = if name == "introfind" { wisp_core::excerpt::INTRO } else { wisp_core::excerpt::METHOD };
                    if let Some(lib) = self.lib.as_ref() { lib.excerpts.shelf(kind).search.update(cx, |input, cx| input.insert(arg, cx)) }
                    return;
                }
                "methodpick" => Some(Cmd::Excerpt(crate::excerpts::Do::Pick)),
                "methodlink" => self.lib.as_ref().and_then(|l| l.gallery.items.get(arg.parse::<usize>().unwrap_or(0))).map(|figure| Cmd::Excerpt(crate::excerpts::Do::Link(figure.id.clone()))),
                "excerptname" => {
                    if let Some(Modal::Excerpt(form)) = self.modal.as_ref() { form.name.update(cx, |input, cx| input.insert(arg, cx)) }
                    return;
                }
                // `pdfmethod` / `pdfintro` keeps what `pdfpick:` selected as a method / an introduction, as 收藏为方法 /
                // 收藏为导言 does: its dialog.
                "pdfmethod" | "pdfintro" => {
                    let kind = if name == "pdfintro" { wisp_core::excerpt::INTRO } else { wisp_core::excerpt::METHOD };
                    if let Some(pdf) = self.lib.as_ref().and_then(|l| l.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.excerpt_selection(kind, cx)) }
                    return;
                }
                // `tag:<tag>` is a click on a tag of the sidebar, `tag:+<tag>` one with ⌘ / Ctrl, `tag:-<tag>` one with ⌥ / Alt.
                "tag" => Some(match (arg.strip_prefix('+'), arg.strip_prefix('-')) {
                    (Some(tag), _) => Cmd::ClickTag(tag.to_string(), true),
                    (_, Some(tag)) => Cmd::ExcludeTag(tag.to_string()),
                    _ => Cmd::ClickTag(arg.to_string(), false),
                }),
                "figfind" => {
                    if let Some(lib) = self.lib.as_ref() { lib.gallery.search.update(cx, |input, cx| input.insert(arg, cx)) }
                    return;
                }
                // `figinfer:legend|tags|notes` has the model write that field of the figure's dialog, as its sparkle does.
                "figinfer" => Some(Cmd::FigureInfer(match arg { "tags" => wisp_core::figure::Ask::Tags, "notes" => wisp_core::figure::Ask::Notes, _ => wisp_core::figure::Ask::Legend })),
                // `figdraw` opens the dialog a picture is drawn in, `figdraw:go` has the model draw (again stops it);
                // `figprompt:<text>` leaves the description in its box, `figshape:wide|tall|square` picks the shape.
                "figdraw" => Some(if arg == "go" { Cmd::FigureDraw } else { Cmd::DrawFigure }),
                "figshape" => Some(Cmd::FigureShape(match arg { "wide" => wisp_core::imagegen::Shape::Wide, "tall" => wisp_core::imagegen::Shape::Tall, _ => wisp_core::imagegen::Shape::Square })),
                "figprompt" => {
                    if let Some(prompt) = self.modal.as_ref().and_then(|modal| match modal { Modal::Figure(form) => form.draw.as_ref().map(|draw| draw.prompt.clone()), _ => None }) {
                        prompt.update(cx, |input, cx| input.set_text(arg, cx));
                    }
                    return;
                }
                // `figlabel:` / `figlegend:` / `figtags:` / `fignotes:` leave text in a field of the figure's dialog, as if typed.
                "figlabel" | "figlegend" | "figtags" | "fignotes" => {
                    if let Some(Modal::Figure(form)) = self.modal.as_ref() {
                        let field = match name { "figlabel" => &form.label, "figlegend" => &form.legend, "figtags" => &form.tags, _ => &form.notes };
                        field.update(cx, |input, cx| input.set_text(arg, cx));
                    }
                    return;
                }
                "beside" => Some(Cmd::BesidePdf(arg != "off")),
                // `swap` puts the paper's PDF and its conversation at each other's side.
                "swap" => Some(Cmd::SwapPanes),
                // `pdftools` / `pdftools:bar`: the PDF's tools floating at the page's right edge, or back in its toolbar.
                "pdftools" => Some(Cmd::FloatPdfTools(arg != "bar")),
                // `pdffree`, `pdffree:off`: 分屏 自由 or 固定. `halves:0.3`: 自由, the first half's share of the room.
                "pdffree" => Some(Cmd::PdfSplitFree(arg != "off")),
                "halves" => {
                    if let Some(lib) = self.lib.as_mut() { lib.split_share = crate::pdf::dragged_share(arg.parse().unwrap_or(0.5), 0., (lib.pdf_area(cx).0 - crate::pdf::DIVIDER).max(0.)) }
                    cx.notify();
                    return;
                }
                // `float` / `float:off`: the paper's conversation over its PDF, or beside it; `fold` / `fold:off` folds it up or opens it.
                "float" => Some(Cmd::FloatChat(arg != "off")),
                "fold" => Some(Cmd::FoldChat(arg != "off")),
                // `pdfsplit:side`, `pdfsplit:stacked`, `pdfsplit:off`: the PDF on show twice, or once again.
                "pdfsplit" => Some(Cmd::PdfSplit(match arg {
                    "side" => Split::Side,
                    "stacked" => Split::Stacked,
                    "triple" => Split::Triple,
                    _ => Split::Off,
                })),
                // `pdfnext`: 下一页 beside the pages of the PDF on show, on or off. `pdfshow:<view>;<pages|theirs|next|map>` is a view's label told what to show.
                "pdfnext" => Some(Cmd::PdfNext),
                "pdfshow" => {
                    let (view, what) = arg.split_once(';').unwrap_or((arg, "pages"));
                    let shows = match what { "theirs" => crate::pdf::Shows::Theirs, "next" => crate::pdf::Shows::Next, "map" => crate::pdf::Shows::Map, _ => crate::pdf::Shows::Pages };
                    if let (Some(pdf), Ok(view)) = (self.lib.as_ref().and_then(|lib| lib.pdf.clone()), view.parse::<usize>()) { pdf.update(cx, |pdf, cx| pdf.show_in_view(view, shows, cx)) }
                    None
                }
                // `pdffind`: the find bar of the PDF on show, as its button in the bar opens it (again puts it away);
                // `pdffind:<words>` types them into it. The key itself is `key:cmd-f`.
                "pdffind" if arg.is_empty() => Some(Cmd::PdfFind(!self.lib.as_ref().and_then(|lib| lib.pdf.as_ref()).is_some_and(|pdf| pdf.read(cx).finding()))),
                "pdffind" => {
                    if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.open_finder(Some(arg.to_string()), false, window, cx)) }
                    None
                }
                // `pdfoverview`: 页面总览 of the PDF on show, on or off.
                "pdfoverview" => Some(Cmd::PdfOverview),
                // `pdfpage`: the dialog that goes to a page typed. `pdfpage:12` goes there.
                "pdfpage" => match arg.parse::<usize>() {
                    Ok(page) => {
                        if let Some(pdf) = self.lib.as_ref().and_then(|lib| lib.pdf.clone()) { pdf.update(cx, |pdf, cx| pdf.go_to(page, cx)) }
                        None
                    }
                    Err(_) => Some(Cmd::AskPdfPage),
                },
                // `zoom:150`, `zoom:page`, `zoom:width`, `zoom:in`, `zoom:out`
                "zoom" => Some(Cmd::PdfZoom(match arg {
                    "in" => ZoomTo::In,
                    "out" => ZoomTo::Out,
                    "page" => ZoomTo::Set(Zoom::FitPage),
                    "width" => ZoomTo::Set(Zoom::FitWidth),
                    percent => ZoomTo::Set(Zoom::Percent(percent.parse().unwrap_or(100.))),
                })),
                "translate" if matches!(arg, "titles" | "abstracts") => {
                    let Some(lib) = self.lib.as_ref() else { return };
                    let ids = lib.shown.iter().map(|&i| lib.papers[i].id.clone()).filter(|id| lib.marked.contains(id)).collect();
                    Some(Cmd::TranslatePapers(ids, if arg == "titles" { wisp_core::translation::Field::Title } else { wisp_core::translation::Field::Abstract }))
                }
                "translate" if arg == "stop" => Some(Cmd::StopBatchTranslation),
                // `facing` shows the pages with their paragraphs translated beside the PDF and starts translating (or puts
                // it away), `facing:run` translates what is not translated yet, `facing:stop` stops.
                "facing" => Some(match arg { "run" => Cmd::FacingRun, "stop" => Cmd::FacingStop, "inplace" => Cmd::FacingPlace(true), "beside" => Cmd::FacingPlace(false), _ => Cmd::ToggleFacing }),
                "translate" => Some(Cmd::ToggleTranslation),
                // `profile:1` shows the second provider on 设置 › AI › 模型.
                "profile" => Some(Cmd::SelectProvider(arg.parse().unwrap_or(0))),
                // `testmodel:0` tests the first model of the provider on show; `testmodel:new` tests the candidate in `new_model`.
                "testmodel" => Some(if arg == "new" { Cmd::TestNewModel } else { Cmd::TestModel(arg.parse().unwrap_or(0)) }),
                // `newmodel:<name>` types into the add-a-model box on 设置 › AI › 模型; `addmodel` adds it once ready.
                "newmodel" => {
                    self.set_new_model_text(arg, cx);
                    None
                }
                "addmodel" => Some(Cmd::AddModel),
                // `palette` opens the command palette; `palette:<text>` with that typed into it.
                "palette" => {
                    if self.palette.is_none() { self.toggle_palette(window, cx) }
                    if let Some(input) = self.palette.as_ref().map(|palette| palette.input.clone()) { input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    self.palette_event(InputEvent::Changed, window, cx);
                    return;
                }
                "menu" => {
                    let kind = match arg { "sort" => MenuKind::Sort, "columns" => MenuKind::Columns, "year" => MenuKind::Filter(FilterKey::Year), "impact" => MenuKind::Filter(FilterKey::Impact), "zones" => MenuKind::Filter(FilterKey::Zone), "zoom" => MenuKind::PdfZoom, "pdfsplit" => MenuKind::PdfSplit, "pdfmore" => MenuKind::PdfMore, "panes" => MenuKind::Panes, "layout" => MenuKind::Layout, "pdffiles" => MenuKind::PdfFiles, "fulltext" => MenuKind::Filter(FilterKey::Fulltext), "source" => MenuKind::Filter(FilterKey::Source), "manuscripts" => MenuKind::Manuscripts, "export" => MenuKind::ManuscriptExport, "retry" => MenuKind::ExtractRetry, "record" => MenuKind::ScreenRecord, "podcasts" => MenuKind::Podcasts, "speed" => MenuKind::PlaySpeed, _ => MenuKind::Add };
                    self.open_menu(kind, point(px(if matches!(arg, "zoom" | "manuscripts" | "export" | "retry") { 900. } else { 20. }), px(120.)), cx);
                    return;
                }
                "search" => {
                    if let Some(lib) = self.lib.as_mut() {
                        lib.search.update(cx, |input, cx| input.set_text(arg, cx));
                        lib.filters.search = arg.to_string();
                        lib.refilter();
                    }
                    None
                }
                // 全库查找: `libfind` what the search box holds, `libfind:<words>` those (as the palette's row), `libfind:go;<n>`
                // the n-th row of the places (a paper, a place), `libfind:close` the list again.
                "libfind" => Some(Cmd::LibraryFind(match arg.split_once(';') {
                    Some(("go", n)) => crate::library_find::Do::Go(n.parse().unwrap_or(0)),
                    _ if arg == "close" => crate::library_find::Do::Close,
                    _ if arg.is_empty() => crate::library_find::Do::Open,
                    _ => crate::library_find::Do::Query(arg.to_string()),
                })),
                // Ask the library a question (no commas in it: they separate the steps).
                "ask" => {
                    if let Some(lib) = self.lib.as_ref() { lib.library_chat.input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    Some(Cmd::Ask(Talk::Library))
                }
                // The library's conversation, by places from 0: `retract:<n>` takes back the n-th question, `unretract` puts it back,
                // `regenerate` asks for the last answer again, `fork:<n>` branches after the n-th answer.
                // `paper` first in the argument is the paper's conversation instead: `retract:paper;0`, `regenerate:paper`.
                "retract" | "regenerate" | "fork" | "unretract" => self.lib.as_ref().and_then(|lib| {
                    let (talk, arg) = match arg.strip_prefix("paper") { Some(rest) => (Talk::Paper, rest.trim_start_matches(';')), None => (Talk::Library, arg) };
                    let messages = &lib.chat(talk).messages;
                    let n: usize = arg.parse().unwrap_or(0);
                    // An answer ends where the next question (or the conversation) begins.
                    let answer_ends: Vec<usize> = (0..messages.len()).filter(|&i| matches!(messages[i], ChatItem::Answer(_)) && !matches!(messages.get(i + 1), Some(ChatItem::Answer(_) | ChatItem::Tool { .. }))).collect();
                    match name {
                        "retract" => messages.iter().enumerate().filter(|(_, item)| matches!(item, ChatItem::User { .. })).nth(n).map(|(at, _)| Cmd::Retract(talk, at)),
                        "regenerate" => Some(Cmd::Regenerate(talk, messages.len().checked_sub(1)?)),
                        "fork" => answer_ends.get(n).map(|&at| Cmd::Fork(talk, at)),
                        _ => Some(Cmd::Unretract(talk)),
                    }
                }),
                // `wfig` opens 写作's 资料库; `wfig:add:<path>` brings a picture file in, `wfig:put:<path>` also puts it into
                // the manuscript; `wfig:insert:<n>` puts the n-th figure of the library into the manuscript, `wfig:edit:<n>` opens its
                // dialog; `wfig:only:uploaded|papers|drawn|used|all` narrows; `wfig:find:<words>` searches.
                "wfig" => {
                    use crate::write_figures::{Do, Only};
                    let (what, rest) = arg.split_once(':').unwrap_or((arg, ""));
                    let nth = |this: &Self| this.lib.as_ref().and_then(|lib| lib.gallery.items.get(rest.parse::<usize>().unwrap_or(1).saturating_sub(1))).map(|figure| figure.id.clone());
                    match what {
                        "add" => Some(Cmd::WriteFigures(Do::Add(vec![rest.into()]))),
                        "put" => {
                            self.write_figures_add(vec![rest.into()], true, cx);
                            return;
                        }
                        "insert" => nth(self).map(|id| Cmd::WriteFigures(Do::Insert(id))),
                        "edit" => nth(self).map(|id| Cmd::WriteFigures(Do::Edit(id))),
                        "only" => Some(Cmd::WriteFigures(Do::Only(match rest { "uploaded" => Only::Uploaded, "papers" => Only::Papers, "drawn" => Only::Drawn, "used" => Only::Used, _ => Only::All }))),
                        "find" => {
                            if let Some(lib) = self.lib.as_ref() { lib.write_figures.search.update(cx, |input, cx| input.set_text(rest, cx)) }
                            return;
                        }
                        _ => Some(Cmd::WriteFigures(Do::Open)),
                    }
                }
                // `agent` / `agent:off` is 写作's other mode, where the assistant writes; `agent:earlier|later|now|restore` steps through the versions of the section on show.
                "agent" => Some(Cmd::Agent(match arg {
                    "off" => crate::write_agent::Do::Mode(false),
                    "earlier" => crate::write_agent::Do::Step(false),
                    "later" => crate::write_agent::Do::Step(true),
                    "now" => crate::write_agent::Do::Now,
                    "restore" => crate::write_agent::Do::Restore,
                    _ => crate::write_agent::Do::Mode(true),
                })),
                // `askwrite:<what to write>` says it to the assistant that writes the manuscript on show.
                "askwrite" => {
                    self.agent_attend(cx);
                    if let Some(lib) = self.lib.as_ref() { lib.write_chat.input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    Some(Cmd::Ask(Talk::Write))
                }
                // `compacted` shows or puts away the summary a compacted conversation with the library goes on from.
                "compacted" => Some(Cmd::ToggleCompacted(Talk::Library)),
                "history-write" => Some(Cmd::ToggleHistory(Talk::Write)),
                // The same of the selected paper.
                "askpaper" => {
                    if let Some(lib) = self.lib.as_ref() { lib.paper_chat.input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    Some(Cmd::Ask(Talk::Paper))
                }
                // `type:/se` leaves text in the library's question box, as if typed (the slash commands show).
                "type" => {
                    if let Some(lib) = self.lib.as_ref() {
                        lib.library_chat.input.update(cx, |input, cx| input.set_text(arg, cx));
                        lib.library_chat.input.read(cx).focus(window);
                    }
                    self.chat_input(Talk::Library, InputEvent::Changed, cx);
                    None
                }
                // `typewrite:/jo` does the same in the box of the assistant that writes the manuscript on show.
                "typewrite" => {
                    self.agent_attend(cx);
                    if let Some(lib) = self.lib.as_ref() {
                        lib.write_chat.input.update(cx, |input, cx| input.set_text(arg, cx));
                        lib.write_chat.input.read(cx).focus(window);
                    }
                    self.chat_input(Talk::Write, InputEvent::Changed, cx);
                    None
                }
                // `typepaper:@zh` does the same in the box of the selected paper's conversation (its 提问 tab).
                "typepaper" => {
                    if let Some(lib) = self.lib.as_ref() {
                        lib.paper_chat.input.update(cx, |input, cx| input.set_text(arg, cx));
                        lib.paper_chat.input.read(cx).focus(window);
                    }
                    self.chat_input(Talk::Paper, InputEvent::Changed, cx);
                    None
                }
                // `image:/path/to.png` puts a picture with the next question to the library.
                "image" => {
                    self.attach_images(Talk::Library, vec![Picture::File(arg.into())], cx);
                    None
                }
                // `document:<path>` puts a Word file or a PowerPoint file with the library's next question, as dropping it on 助手 does.
                "document" => {
                    self.attach_documents(Talk::Library, vec![arg.into()], cx);
                    None
                }
                // `history:paper` is the paper's.
                "history" => Some(Cmd::ToggleHistory(if arg == "paper" { Talk::Paper } else { Talk::Library })),
                // `openchat:<n>` takes up the n-th conversation 历史 lists (it must be open), as a click on its row does.
                "openchat" => self.lib.as_ref().and_then(|lib| lib.library_chat.history.as_ref()?.get(arg.parse::<usize>().unwrap_or(0)).map(|c| Cmd::OpenChat(Talk::Library, c.id.clone()))),
                // The steps below act on the 历史 on show: the library's on its page, else the paper's.
                // `historyall` / `historyall:off` (全部对话 / this place's), `historyfind:<words>` (typed into its search),
                // `historyrow:<n>` / `renamechat:<n>` (a click on the n-th row it shows, from 0 / 重命名… of its menu).
                "historyall" | "historyfind" | "historyrow" | "renamechat" => {
                    let Some(lib) = self.lib.as_ref() else { return };
                    let talk = if lib.page == Page::Ask && lib.library_chat.history.is_some() { Talk::Library } else { Talk::Paper };
                    let chat = lib.chat(talk);
                    let nth = |n: &str| chat.history.as_ref().and_then(|list| list.iter().filter(|c| chat.history_hits.as_ref().is_none_or(|hits| hits.contains(&c.id))).nth(n.parse::<usize>().unwrap_or(0)).map(|c| c.id.clone()));
                    match name {
                        "historyall" => Some(Cmd::HistoryAll(talk, arg != "off")),
                        "historyfind" => {
                            chat.history_find.clone().update(cx, |input, cx| input.set_text(arg, cx));
                            self.history_typed(talk, arg.to_string(), cx);
                            None
                        }
                        "historyrow" => nth(arg).map(|id| Cmd::OpenChat(talk, id)),
                        _ => nth(arg).map(|id| Cmd::AskRenameChat(talk, id)),
                    }
                }
                // What stands below the fold of a long page of settings: the page scrolled down by so many pixels.
                "settingsdown" => {
                    self.scroll_settings(arg.parse().unwrap_or(0.), cx);
                    return;
                }
                "settings" => Some(match arg {
                    "ai" | "models" => Cmd::AiTab(AiTab::Models),
                    "tasks" => Cmd::AiTab(AiTab::Tasks),
                    "commands" => Cmd::AiTab(AiTab::Commands),
                    "customize" => Cmd::OpenSettings(SettingsPage::Customize),
                    "skills" => Cmd::Custom(crate::custom::Do::Tab(crate::custom::Tab::Skills)),
                    _ => Cmd::OpenSettings(match arg {
                        "appearance" => SettingsPage::Appearance,
                        "shortcuts" => SettingsPage::Shortcuts,
                        "writing" => SettingsPage::Writing,
                        "network" => SettingsPage::Network,
                        "storage" => SettingsPage::Storage,
                        "about" => SettingsPage::About,
                        _ => SettingsPage::General,
                    }),
                }),
                // `citestyles` opens the dialog of the manuscript's citation style (after `page:write`); `citestyle:<key>` is a click
                // on a style in it, or on 设置 › 写作 (`settings:writing`) where it sets the default; `addcitestyle:<file.csl>` adds a file.
                "citestyles" => Some(Cmd::OpenCiteStyles),
                "citestyle" => Some(if matches!(self.modal, Some(Modal::CiteStyle { .. })) { Cmd::LookCiteStyle(arg.to_string()) } else { Cmd::DefaultCiteStyle(arg.to_string()) }),
                "addcitestyle" => {
                    self.take_cite_style(std::path::Path::new(arg), cx);
                    return;
                }
                // 调整 on the list of styles: `adjuststyle` opens it or puts it away; `tunestyle:<detail>;<value>` sets a
                // detail of the style on show (`brackets;square|round|bare`, `raised;on|off`, `cited;<n>`, `listed;<n>`,
                // `first;<n>`, `initials;dotspace|dot|space|none`; `tunestyle:reset` is 还原); `keepstyle` keeps what is
                // set; `askstyle:<key>` takes a style to the assistant.
                "adjuststyle" => Some(Cmd::AdjustCiteStyles),
                "tunestyle" => {
                    use crate::cite_styles::Knob;
                    use wisp_core::adjust::Brackets;
                    let (detail, value) = arg.split_once(';').unwrap_or((arg, ""));
                    let n = value.parse().unwrap_or(0);
                    Some(Cmd::TuneCiteStyle(match detail {
                        "brackets" => Some(Knob::Brackets(match value { "square" => Brackets::Square, "round" => Brackets::Round, _ => Brackets::Bare })),
                        "raised" => Some(Knob::Raised(value == "on")),
                        "cited" => Some(Knob::Cited(n)),
                        "listed" => Some(Knob::Listed(n)),
                        "first" => Some(Knob::ListedFirst(n)),
                        "initials" => Some(Knob::Initials(match value { "dotspace" => ". ", "dot" => ".", "space" => " ", _ => "" })),
                        _ => None,
                    }))
                }
                "keepstyle" => Some(Cmd::KeepCiteStyle),
                "askstyle" => Some(Cmd::AskCiteStyle(arg.to_string())),
                "candidate" => Some(Cmd::CompleteChoose(arg.parse().ok())),
                // `command:0` opens the first of the person's own commands on 设置 › 命令.
                "command" => Some(Cmd::SelectCommand(arg.parse().unwrap_or(0))),
                // `consent` / `consent:always` / `consent:deny` answers the card that asks about the assistant's tools.
                // It is the conversation that waits for a yes which is answered: the one that writes a manuscript before a paper's and the library's.
                "consent" => {
                    let waits = self.lib.as_ref().and_then(|lib| [Talk::Write, Talk::Paper, Talk::Library].into_iter().find(|talk| lib.chat(*talk).pending.is_some())).unwrap_or(Talk::Library);
                    Some(Cmd::AnswerTools(waits, match arg { "always" => Consent::Always, "deny" => Consent::Deny, _ => Consent::Once }))
                }
                // `stopchat` presses 停止 in the library's conversation, `stopchat:paper` / `stopchat:write` in a paper's / the one that writes.
                "stopchat" => Some(Cmd::StopChat(match arg { "paper" => Talk::Paper, "write" => Talk::Write, _ => Talk::Library })),
                // `connector:0` opens the first connector on 设置 › 自定义 (after `settings:customize`).
                "connector" => Some(Cmd::Custom(crate::custom::Do::SelectConnector(arg.parse().unwrap_or(0)))),
                // `skill:0` opens the first skill's page on 设置 › 自定义 (after `settings:skills`); `skill:file;<path>` shows a file of it,
                // `skill:fold;<folder>` puts a folder of its files away or brings it back, `skill:source` / `skill:page` is a Markdown
                // file as its source / as the page it makes, `skill:remove` takes the skill out and `skill:back` is the list again.
                "skill" => {
                    use crate::custom::Do;
                    let (what, rest) = arg.split_once(';').unwrap_or((arg, ""));
                    Some(Cmd::Custom(match what {
                        "file" => Do::ShowSkillFile(rest.to_string()),
                        "fold" => Do::FoldSkillFolder(rest.to_string()),
                        "source" => Do::SkillSource(true),
                        "page" => Do::SkillSource(false),
                        "remove" => Do::RemoveSkill,
                        "back" => Do::BackToSkills,
                        _ => Do::SelectSkill(what.parse().unwrap_or(0)),
                    }))
                }
                // `signin:0` signs in to the first connector in the browser (a scripted run prints the address instead of opening it); `signout:0` signs out.
                "signin" => Some(Cmd::Custom(crate::custom::Do::SignIn(arg.parse().unwrap_or(0)))),
                "signout" => Some(Cmd::Custom(crate::custom::Do::SignOut(arg.parse().unwrap_or(0)))),
                // Saved like the choice in 设置 › 外观: "light", "dark", or empty for the system's.
                "theme" => Some(Cmd::SetAppearance(arg.to_string())),
                // Typography is saved just as through the appearance controls.
                "fontsize" => arg.parse().ok().map(Cmd::SetFontSize),
                // `fontpicker:cjk` opens the Chinese font's picker, anything else the Latin one's.
                "fontpicker" => {
                    self.toggle_font_picker(if arg == "cjk" { crate::settings::FontSlot::Cjk } else { crate::settings::FontSlot::Latin }, window, cx);
                    return;
                }
                "fontsearch" => {
                    self.search_settings_fonts(arg, cx);
                    return;
                }
                "fontfamily" => Some(Cmd::SetFontFamily(crate::settings::FontSlot::Latin, arg.to_string())),
                "cjkfont" => Some(Cmd::SetFontFamily(crate::settings::FontSlot::Cjk, arg.to_string())),
                "fontreset" => Some(Cmd::ResetTypography),
                "linespacing" => Some(Cmd::SetLineSpacing(match arg { "compact" => wisp_core::settings::Spacing::Compact, "relaxed" => wisp_core::settings::Spacing::Relaxed, _ => wisp_core::settings::Spacing::Standard })),
                "paragraphspacing" => Some(Cmd::SetParagraphSpacing(match arg { "compact" => wisp_core::settings::Spacing::Compact, "relaxed" => wisp_core::settings::Spacing::Relaxed, _ => wisp_core::settings::Spacing::Standard })),
                "textwidth" => Some(Cmd::SetTextWidth(match arg { "narrow" => wisp_core::settings::TextWidth::Narrow, "wide" => wisp_core::settings::TextWidth::Wide, "fill" => wisp_core::settings::TextWidth::Fill, _ => wisp_core::settings::TextWidth::Standard })),
                "density" => Some(Cmd::SetDensity(arg.to_string())),
                "zone" => Some(Cmd::SetJournalZone(arg.to_string())),
                // A filter of the paper list set (`filter:year;last:3`, `filter:impact;5-`, `filter:zone;cas:2`, nothing after the `;` clears it), and 自定义区间…'s dialog (`range:year`, `range:impact`).
                "filter" | "range" => {
                    let (which, value) = arg.split_once(';').unwrap_or((arg, ""));
                    let key = match which { "impact" => FilterKey::Impact, "zone" => FilterKey::Zone, _ => FilterKey::Year };
                    Some(if name == "range" { Cmd::FilterRange(key) } else { Cmd::SetFilter(key, value.to_string()) })
                }
                "contrast" => Some(Cmd::ToggleHighContrast),
                "appearancereset" => Some(Cmd::ResetAppearance),
                "language" => Some(Cmd::SetLanguage(if arg == "en" { Language::En } else { Language::Zh })),
                "record" => Some(Cmd::RecordShortcut(arg.to_string())),
                "update" => Some(Cmd::CheckUpdate),
                // The window's close button: put away while 设置 › 通用 › 后台 is on, else closed (and the app quits).
                "close" => {
                    if crate::background::close(self.settings.keep_running, window, cx) { window.remove_window() }
                    return;
                }
                // `addid:<DOI or PMID>` opens 通过 DOI / PMID 添加 with that typed; `submit` then looks it up.
                "addid" => {
                    self.run(Cmd::AddById, window, cx);
                    if let Some(Modal::AddById { input, .. }) = &self.modal { input.update(cx, |input, cx| input.set_text(arg, cx)) }
                    None
                }
                "modal" => match arg {
                    "fields" => Some(Cmd::ManageFields),
                    "library" => Some(Cmd::NewLibrary),
                    "add" => Some(Cmd::AddById),
                    "search" => Some(Cmd::NewSearch),
                    "paper" => self.lib.as_ref().and_then(|l| l.selected.clone()).map(Cmd::EditPaper),
                    // 按题目补全 for the selected paper; `submit` then looks, `candidate:0` opens the first record found.
                    "complete" => self.lib.as_ref().and_then(|l| l.selected.clone()).map(Cmd::CompletePaper),
                    // 按 DOI / PMID 补全 for the selected paper: asks at once and opens the record.
                    "complete-id" => self.lib.as_ref().and_then(|l| l.selected.clone()).map(Cmd::CompleteById),
                    _ => None,
                },
                _ => None,
            };
            if let Some(cmd) = cmd { self.run(cmd, window, cx) }
        }
    }
}
