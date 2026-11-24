//! 朗读 and 播客 as the window runs them: starting and stopping a reading of the PDF on show, following it
//! through the pages, and a podcast made, kept with the paper's files, played again and saved. The thread
//! that speaks is `read_aloud.rs`, the card is `player.rs`.

use crate::app::{Cmd, Library, Workspace};
use crate::modal::Modal;
use crate::i18n::t;
use gpui::*;
use wisp_core::model::*;

impl Library {
    /// Where the kept podcast heard is: its sound's attachment and the sample heard, kept as a file's place
    /// is (`reading_places`, the sample as its page).
    pub(crate) fn podcast_place(&self) -> Option<(String, (usize, f32))> {
        let reading = self.aloud.as_ref()?;
        Some((reading.sound.clone()?, (reading.status().played, 0.)))
    }

    /// Stop what is read aloud or talked over, and take the sentence it marked off the page. Where a kept
    /// podcast was left is remembered: 播客 goes on from there.
    pub fn stop_reading(&mut self, cx: &mut App) {
        if let Some((sound, place)) = self.podcast_place() {
            self.file_places.insert(sound.clone(), place);
            let _ = self.store.save_reading_places(&[(sound, place)]);
        }
        (self.aloud, self.aloud_shown, self.seek_held) = (None, None, None);
        if let Some(pdf) = &self.pdf { pdf.update(cx, |pdf, cx| pdf.follow_spoken(None, cx)) }
    }

    /// The podcast heard on while its paper is not on show: the paper, its file and how it stands; the
    /// bar at the window's foot shows it, and leads back.
    pub fn podcast_away(&self, cx: &App) -> Option<(&Paper, String, crate::read_aloud::Status)> {
        let reading = self.aloud.as_ref().filter(|r| r.kind == crate::read_aloud::Kind::Podcast)?;
        let viewing = self.pdf_on_show();
        let on_show = self.selected.is_some() && viewing && self.pdf.as_ref().is_some_and(|pdf| self.playing_here(pdf.read(cx).resource_id()).is_some());
        if on_show { return None }
        let paper = self.files.iter().find(|f| f.id == reading.resource).and_then(|f| self.papers.iter().find(|p| p.id == f.paper_id))?;
        Some((paper, reading.resource.clone(), self.aloud_shown.clone()?))
    }

    /// What is read of the file `file` (`PaperFile::id`) while it is on show: 朗读 of that file, or 播客 of
    /// its paper (made from whichever of its files, or played again from what was kept).
    pub fn playing_here(&self, file: &str) -> Option<crate::read_aloud::Kind> {
        let reading = self.aloud.as_ref()?;
        if reading.resource == file { return Some(reading.kind) }
        let paper = |id: &str| self.files.iter().find(|f| f.id == id).map(|f| f.paper_id.as_str());
        (reading.kind == crate::read_aloud::Kind::Podcast && paper(file).is_some() && paper(file) == paper(&reading.resource)).then_some(reading.kind)
    }
}

impl Workspace {
    /// 朗读: read `selection` aloud, or (`None`) the file on show from the page in view to its reference
    /// list. The pages are the text kept for the file, else read off it on the reading's thread.
    pub(crate) fn read_aloud(&mut self, selection: Option<(usize, String)>, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.speech_provider().cloned().filter(|p| !p.key().is_empty()) else { return self.ask_for_speech(cx) };
        let (key, voice) = (provider.key(), self.settings.speech_voice.clone());
        let Some(lib) = self.lib.as_mut() else { return };
        let Some(pdf) = lib.pdf.clone() else { return };
        let (resource, page) = { let view = pdf.read(cx); (view.resource_id().to_string(), view.current_page()) };
        // The text kept for the file when there is some; the file itself is read on the reading's thread.
        let kept = match lib.store.fulltext_pages(&resource) { Ok(Some(Ok(pages))) => Some(pages), _ => None };
        let path = lib.files.iter().find(|f| f.id == resource).map(|f| lib.store.file_path(f));
        // Its paper's record finds the title on the first page (an attachment's will not have it).
        let paper = lib.files.iter().find(|f| f.id == resource).and_then(|f| lib.papers.iter().find(|p| p.id == f.paper_id)).cloned().unwrap_or_default();
        let plan = move || -> Result<crate::read_aloud::Plan, String> {
            use wisp_core::speech;
            let pieces = match selection {
                // A selection marks no page on the button; its sentences are marked on its page all the same.
                Some((on, text)) => speech::pieces(&speech::readable(&text), speech::PIECE).into_iter().map(|piece| (0, on, piece)).collect(),
                None => speech::from_pages(&pages_of(kept, path)?, page, &paper).into_iter().map(|(page, piece)| (page, page, piece)).collect::<Vec<_>>(),
            };
            let voice = speech::voice(&voice).to_string();
            Ok(crate::read_aloud::Plan::of(pieces.into_iter().map(|(mark, page, text)| crate::read_aloud::Piece { mark, page, voice: voice.clone(), text, pause: false }).collect()))
        };
        let (base, proxy) = (provider.base_url.clone(), provider.proxy.clone());
        let reading = crate::read_aloud::Reading::start(resource, crate::read_aloud::Kind::Aloud, plan, move |voice, text| wisp_core::speech::request(&base, &key, &proxy, voice, text));
        self.follow_reading(reading, cx);
    }

    /// 播客: two hosts, a woman and a man, talk the paper on show over. A model (the assistant's) writes
    /// their script from the file's text on the reading's thread; each turn is then spoken in its host's
    /// voice and played as it comes. Once all the sound came it is kept as the paper's attachments: a WAV
    /// file and the script.
    pub(crate) fn podcast(&mut self, resource: &str, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.speech_provider().cloned().filter(|p| !p.key().is_empty()) else { return self.ask_for_speech(cx) };
        let key = provider.key();
        let profile = self.settings.speaking_profile_for(wisp_core::settings::Task::Chat);
        let language = match self.settings.language { wisp_core::settings::Language::Zh => "中文", wisp_core::settings::Language::En => "英文" };
        let Some(lib) = self.lib.as_mut() else { return };
        let resource = resource.to_string();
        let Some(paper) = lib.files.iter().find(|f| f.id == resource).and_then(|f| lib.papers.iter().find(|p| p.id == f.paper_id)).cloned() else { return };
        let kept = match lib.store.fulltext_pages(&resource) { Ok(Some(Ok(pages))) => Some(pages), _ => None };
        let path = lib.files.iter().find(|f| f.id == resource).map(|f| lib.store.file_path(f));
        let plan = move || -> Result<crate::read_aloud::Plan, String> {
            use wisp_core::podcast;
            // A file that gives no text still has its title and abstract to talk about.
            let pages = pages_of(kept, path).unwrap_or_default();
            let request = podcast::request(&profile, language, &paper, &pages)?;
            // The script streams in on a thread of its own; each turn is spoken as soon as it is whole.
            let (sender, events) = async_channel::unbounded();
            let protocol = profile.protocol;
            std::thread::Builder::new().name("podcast-script".into()).spawn(move || wisp_net::stream_chat(request, protocol, sender)).map_err(|error| error.to_string())?;
            let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<podcast::Turn>::new()));
            let kept = heard.clone();
            let mut reader = podcast::Reader::default();
            let mut ended = false;
            let turns = std::iter::from_fn(move || loop {
                if ended { return None }
                let whole = match events.recv_blocking() {
                    Ok(wisp_core::ai::ChatEvent::Text(text)) => reader.push(&text),
                    Ok(wisp_core::ai::ChatEvent::Retrying(..) | wisp_core::ai::ChatEvent::Used(_)) => continue,
                    Ok(wisp_core::ai::ChatEvent::Error(error)) => {
                        ended = true;
                        return Some(vec![Err(tf!("播客稿没有写成：{}", "The podcast script was not written: {}", error))]);
                    }
                    // A script cut off by its length is still a podcast up to there.
                    Ok(wisp_core::ai::ChatEvent::Done(_)) | Err(_) => {
                        ended = true;
                        let last = reader.finish();
                        if last.is_empty() && heard.lock().map_or(true, |turns| turns.is_empty()) { return Some(vec![Err(podcast::NO_SCRIPT.to_string())]) }
                        last
                    }
                };
                if whole.is_empty() { continue }
                let mut out = vec![];
                let Ok(mut turns) = heard.lock() else { return None };
                for turn in whole {
                    let (voice, pieces) = podcast::pieces(&turn);
                    let mark = turns.len() + 1;
                    out.extend(pieces.into_iter().enumerate().map(|(at, text)| Ok(crate::read_aloud::Piece { mark, page: 0, voice: voice.to_string(), text, pause: at == 0 })));
                    turns.push(turn);
                }
                return Some(out);
            });
            // The script kept beside the sound names its hosts in the interface's language.
            let (title, english) = (paper.title.clone(), crate::i18n::is_english());
            Ok(crate::read_aloud::Plan { pieces: Box::new(turns.flatten()), transcript: Some(Box::new(move || podcast::transcript(&title, &kept.lock().map(|turns| turns.clone()).unwrap_or_default(), english))) })
        };
        let (base, proxy) = (provider.base_url.clone(), provider.proxy.clone());
        let reading = crate::read_aloud::Reading::start(resource, crate::read_aloud::Kind::Podcast, plan, move |voice, text| wisp_core::speech::request(&base, &key, &proxy, voice, text));
        self.follow_reading(reading, cx);
    }

    /// Keep a reading (朗读 or 播客) of the file on show, and look four times a second how it stands:
    /// drawn again only when that changed, and over when it is. 朗读 belongs to the viewer: the sentence
    /// heard is marked on its page, and it stops when the viewer shows another file or none. A podcast
    /// plays on when its paper is left (a bar at the window's foot leads back), and its sound is kept
    /// once all of it came.
    fn follow_reading(&mut self, reading: crate::read_aloud::Reading, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_mut() else { return };
        lib.stop_reading(cx);
        reading.set_speed(self.settings.play_speed);
        lib.aloud_shown = Some(reading.status());
        lib.aloud = Some(reading);
        let library = lib.info.id.clone();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let going = this.update(cx, |this, cx| {
                let Some(lib) = this.lib.as_mut().filter(|lib| lib.info.id == library) else { return false };
                let Some(reading) = lib.aloud.as_ref() else { return false };
                if let Some((sound, transcript)) = reading.take_recording() {
                    let resource = reading.resource.clone();
                    this.keep_podcast(&resource, sound, transcript, cx);
                }
                let Some(lib) = this.lib.as_mut() else { return false };
                let Some(reading) = lib.aloud.as_ref() else { return false };
                let what = if reading.kind == crate::read_aloud::Kind::Podcast { t("播客") } else { t("朗读") };
                // The viewer went to another file, or was put away: the reading was its.
                let aloud = reading.kind == crate::read_aloud::Kind::Aloud;
                let viewer = lib.pdf.clone().filter(|pdf| pdf.read(cx).resource_id() == reading.resource);
                if aloud && viewer.is_none() {
                    lib.stop_reading(cx);
                    cx.notify();
                    return false;
                }
                let status = reading.status();
                if status.over {
                    // A kept podcast heard to its end: the playlist goes on with the next one.
                    let next = reading.replay.clone().filter(|_| status.error.is_none()).and_then(|sound| {
                        let list = podcast_list(lib);
                        let at = list.iter().position(|(_, file, _)| file.id == sound)?;
                        list.get(at + 1).map(|(_, file, _)| file.id.clone())
                    });
                    lib.stop_reading(cx);
                    if let Some(error) = status.error { this.fail(tf!("{}停了：{}", "{} stopped: {}", what, crate::i18n::reasons(&error)), cx) }
                    if let Some(next) = next { this.play_podcast(&next, cx) }
                    cx.notify();
                    return false;
                }
                if aloud && let Some(viewer) = viewer {
                    let heard = status.heard.as_ref().map(|heard| (heard.page, heard.sentence.clone()));
                    viewer.update(cx, |pdf, cx| pdf.follow_spoken(heard, cx));
                }
                if lib.aloud_shown.as_ref() != Some(&status) {
                    lib.aloud_shown = Some(status);
                    cx.notify();
                }
                true
            });
            if !matches!(going, Ok(true)) { break }
        })
        .detach();
        cx.notify();
    }

    /// A podcast heard whole, kept with the paper of the file it was made from: the sound as a WAV file
    /// and its script, as two attachments.
    fn keep_podcast(&mut self, resource: &str, sound: Vec<u8>, transcript: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(paper) = lib.files.iter().find(|f| f.id == resource).and_then(|f| lib.papers.iter().find(|p| p.id == f.paper_id)) else { return };
        let (paper_id, name) = (paper.id.clone(), wisp_core::podcast::file_name(&paper.title, crate::i18n::is_english()));
        let dir = std::env::temp_dir().join(format!("wisp-podcast-{}", std::process::id()));
        let (audio, text) = (dir.join(format!("{name}.wav")), dir.join(format!("{name}.md")));
        let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&audio, wisp_core::podcast::wav(&sound))).and_then(|_| std::fs::write(&text, transcript));
        if let Err(error) = written { return self.fail(tf!("播客没有存下来：{}", "The podcast was not saved: {}", error), cx) }
        if self.change(cx, |lib| { lib.store.import_file(&paper_id, &audio, "other")?; lib.store.import_file(&paper_id, &text, "other")?; Ok(()) }) {
            self.say(t("播客的声音和稿子已存为这篇的附件"), cx);
            // The podcast heard is the one kept now: where it is left is remembered under its sound.
            if let Some(lib) = self.lib.as_mut() {
                let sound = podcast_files(lib, &paper_id).0.map(|f| f.id.clone());
                if let Some(reading) = lib.aloud.as_mut().filter(|r| r.resource == resource) { reading.sound = sound }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No DeepSeek service with a key to speak with: say so, and offer to set it up (`Cmd::SetUpSpeech`).
    fn ask_for_speech(&mut self, cx: &mut Context<Self>) {
        let has = self.settings.speech_provider().is_some();
        let message = if has { t("朗读和播客的声音来自 DeepSeek 的语音服务，按字数计费。已经添加了 DeepSeek，还差它的 API Key：填好就能听。") } else { t("朗读和播客的声音来自 DeepSeek 的语音服务，按字数计费。先添加 DeepSeek 并填上它的 API Key（在 platform.deepseek.com 申请），就能听了。") };
        self.modal = Some(Modal::Confirm { title: t("还没有配置语音服务").into(), message: message.into(), action: if has { t("去填写 API Key").into() } else { t("配置 DeepSeek").into() }, cmd: Cmd::SetUpSpeech });
        cx.notify();
    }

    /// Play a kept podcast again (its sound's attachment), with the script kept beside it. It stands for
    /// the paper's PDF, as the podcast made from it did: its buttons there say it plays, and the bar leads back to it.
    pub(crate) fn play_podcast(&mut self, sound: &str, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let Some(file) = lib.files.iter().find(|f| f.id == sound) else { return };
        let script = podcast_files(lib, &file.paper_id).1.map(|f| lib.store.file_path(f));
        let resource = lib.pdf_file(&file.paper_id).map_or(file.id.clone(), |f| f.id.clone());
        let at = lib.file_places.get(&file.id).map_or(0, |place| place.0);
        let reading = crate::read_aloud::Reading::replay(resource, file.id.clone(), lib.store.file_path(file), script, at);
        self.follow_reading(reading, cx);
    }

    /// Save a copy of the podcast kept for a paper: its sound or its script, where the person picks.
    pub(crate) fn save_podcast(&mut self, paper: &str, audio: bool, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.as_ref() else { return };
        let kept = podcast_files(lib, paper);
        let Some(file) = (if audio { kept.0 } else { kept.1 }) else { return self.fail(t("这篇还没有存下的播客：播完一次（全部生成）后才会存为附件。"), cx) };
        let (source, name) = (lib.store.file_path(file), file.title.clone());
        let directory = dirs::download_dir().or_else(dirs::document_dir).or_else(dirs::home_dir).unwrap_or_else(|| self.root.clone());
        let picked = cx.prompt_for_new_path(&directory, Some(&name));
        let extension = if audio { "wav" } else { "md" };
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = picked.await else { return };
            if path.extension().is_none() { path.set_extension(extension); }
            let copied = std::fs::copy(&source, &path);
            this.update(cx, |this, cx| match copied {
                Ok(_) => this.say(tf!("已保存到 {}", "Saved to {}", path.display()), cx),
                Err(error) => this.fail(tf!("无法写入 {}：{}", "Cannot write {}: {}", path.display(), error), cx),
            })
            .ok();
        })
        .detach();
    }
}

/// 播客's playlist: each paper's kept podcast (its sound, and its script when there is one), the latest made first.
pub(crate) fn podcast_list(lib: &Library) -> Vec<(&Paper, &PaperFile, Option<&PaperFile>)> {
    let mut list: Vec<(&Paper, &PaperFile, Option<&PaperFile>)> = lib.papers.iter().filter_map(|paper| {
        let (sound, script) = podcast_files(lib, &paper.id);
        Some((paper, sound?, script))
    }).collect();
    list.sort_by(|a, b| b.1.added_at.cmp(&a.1.added_at));
    list
}

/// The podcast kept for a paper, the latest of each: its sound and its script (`podcast::kept`).
pub(crate) fn podcast_files<'a>(lib: &'a Library, paper: &str) -> (Option<&'a PaperFile>, Option<&'a PaperFile>) {
    let latest = |audio: bool| lib.paper_files(paper).into_iter().filter(|f| wisp_core::podcast::kept(&f.title, &f.format) == Some(audio)).max_by(|a, b| a.added_at.cmp(&b.added_at));
    (latest(true), latest(false))
}

/// The pages of a file read aloud or talked over: the text kept for it, else read off the file (on the
/// reading's thread), else none.
pub(crate) fn pages_of(kept: Option<Vec<String>>, path: Option<std::path::PathBuf>) -> Result<Vec<String>, String> {
    match (kept, path) {
        (Some(pages), _) => Ok(pages),
        (None, Some(path)) => wisp_core::fulltext::file_pages(&path).map_err(|error| tf!("读不出这个 PDF 的文字：{}", "Could not read this PDF's text: {}", crate::i18n::text_failure(&error))),
        (None, None) => Ok(vec![]),
    }
}
