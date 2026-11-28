//! Local collection tools. The parsers, deduplication and attachment copier
//! are the same ones used by the window; no network calls are made here.

use super::{number, text, texts};
use crate::{fulltext, import, libraries, lookup, zotero};
use crate::model::{Paper, clean_strings};
use crate::libraries::LibraryInfo;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(super) fn create_library(root: &Path, args: &Value) -> Result<Value> {
    let color = if args["color"].is_null() { "blue" } else { text(args, "color")? };
    let info = libraries::create(root, text(args, "name")?, text(args, "topic")?, color)?;
    Ok(json!({"id": info.id, "name": info.name, "topic": info.topic, "color": info.color, "directory": info.dir, "papers": 0}))
}

/// A `.wispstudy` file from another Wisp Study (or `.wisppaper`, from before the rename), taken in as a new library.
pub(super) fn import_library(root: &Path, args: &Value) -> Result<Value> {
    let path = input_path(args)?;
    let info = crate::archive::import(root, &path)?;
    Ok(json!({"id": info.id, "name": info.name, "topic": info.topic, "color": info.color, "directory": info.dir, "papers": info.paper_count}))
}

/// The whole library as a `.wispstudy` file at a path the person chose; an existing file is left alone.
pub(super) fn export_library(info: &LibraryInfo, args: &Value) -> Result<Value> {
    let path = PathBuf::from(text(args, "path")?);
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(crate::archive::EXTENSION)) { bail!("请选择 .wispstudy 文件路径。") }
    if path.exists() { bail!("{} 已存在，不会覆盖。", path.display()) }
    let attachments = args["attachments"].as_bool().unwrap_or(true);
    let manifest = crate::archive::export(info, &path, attachments)?;
    Ok(json!({"path": path, "papers": manifest.papers, "files": manifest.files, "bytes": std::fs::metadata(&path).map_or(0, |m| m.len()), "attachments": attachments}))
}

fn input_path(args: &Value) -> Result<PathBuf> {
    let value = text(args, "path")?;
    if value.is_empty() { bail!("请提供本机文件或文件夹路径。") }
    let path = std::fs::canonicalize(value).with_context(|| format!("无法打开路径：{value}"))?;
    Ok(path)
}

pub(super) fn import_references(store: &mut Store, args: &Value) -> Result<Value> {
    let path = input_path(args)?;
    let bytes = std::fs::read(&path).with_context(|| format!("无法读取 {}", path.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let papers = import::parse_references(&String::from_utf8_lossy(&bytes), &format!("导入 · {name}")).map_err(anyhow::Error::msg)?;
    let total = papers.len();
    let result = store.import_records(papers)?;
    let mut out = json!({
        "path": path, "total": total, "added": result.added.len(), "skipped": result.skipped.len(),
        "addedPapers": result.added.iter().map(|p| json!({"id": p.id, "title": p.title})).collect::<Vec<_>>(),
        "skippedPapers": result.skipped.iter().map(|p| json!({"title": p.title, "existingId": p.existing_id, "inTrash": p.in_trash})).collect::<Vec<_>>(),
    });
    if !result.invalid.is_empty() { out["invalid"] = super::invalid_records(&result.invalid) }
    Ok(out)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(flatten)]
    record: zotero::Row,
    /// Text extraction failure does not mean that the PDF was not copied.
    #[serde(skip_serializing_if = "String::is_empty")]
    text_error: String,
}

fn report(path: &Path, rows: Vec<Row>) -> Value {
    json!({
        "path": path, "total": rows.len(),
        "added": rows.iter().filter(|r| r.record.added).count(),
        "skipped": rows.iter().filter(|r| !r.record.added && (r.record.paper_id.is_some() || r.record.in_trash)).count(),
        "inTrash": rows.iter().filter(|r| r.record.in_trash).count(),
        "filesAdded": rows.iter().map(|r| r.record.files_added).sum::<usize>(),
        "filesExisting": rows.iter().map(|r| r.record.files_existing).sum::<usize>(),
        "failed": rows.iter().filter(|r| !r.record.problems.is_empty()).count(),
        "textless": rows.iter().filter(|r| !r.text_error.is_empty()).count(),
        "results": rows,
    })
}

pub(super) fn import_zotero(store: &mut Store, args: &Value) -> Result<Value> {
    let path = input_path(args)?;
    let wanted = text(args, "zoteroLibrary")?;
    // Only a library named asks the database what it holds; every library needs no list.
    let listed = if wanted.trim().is_empty() { vec![] } else { zotero::libraries(&path)? };
    let library = zotero::choose(&listed, wanted)?;
    let named = super::texts(args, "zoteroCollections")?;
    // Only the collections of the library chosen can be named.
    let collections: Vec<zotero::Collection> = if named.is_empty() { vec![] } else { zotero::collections(&path)?.into_iter().filter(|c| library.is_none_or(|l| c.library == l)).collect() };
    let only = zotero::choose_collections(&collections, &named)?;
    let imported = zotero::read(&path, library, &only)?;
    let rows = imported.entries.into_iter().map(|entry| Row { record: zotero::import_entry(store, entry), ..Default::default() }).collect();
    let mut result = report(&path, rows);
    // Zotero copies attachments here; text is extracted by the reading tools.
    result.as_object_mut().unwrap().remove("textless");
    result["otherAttachments"] = json!(imported.other_attachments);
    if let Some(chosen) = listed.iter().find(|l| Some(l.id) == library) { result["zoteroLibrary"] = json!(chosen.name) }
    if !only.is_empty() { result["zoteroCollections"] = json!(only.iter().filter_map(|id| collections.iter().find(|c| c.id == *id)).map(|c| &c.path).collect::<Vec<_>>()) }
    Ok(result)
}

/// What `import_zotero` can take from a database: one library at a time, or some of its collections.
pub(super) fn zotero_libraries(args: &Value) -> Result<Value> {
    let path = input_path(args)?;
    Ok(json!({"path": path, "libraries": zotero::libraries(&path)?, "collections": zotero::collections(&path)?}))
}

pub(super) fn import_pdf(store: &mut Store, args: &Value) -> Result<Value> {
    let path = input_path(args)?;
    let paper_id = text(args, "paperId")?;
    if path.is_dir() && !paper_id.is_empty() { bail!("paperId 只能用于单个 PDF，不能用于文件夹。") }
    // Validate an explicit destination before starting any work.
    let target = if paper_id.is_empty() { None } else { Some(store.get_paper(paper_id)?) };
    let scanned = if path.is_dir() { import::scan_pdf_folder(&path) } else { import::PdfFolder { files: vec![(path.clone(), vec![])], errors: vec![] } };
    if scanned.files.is_empty() && scanned.errors.is_empty() { bail!("文件夹 {} 中没有 PDF。", path.display()) }
    let mut rows: Vec<Row> = scanned.errors.into_iter().map(|(path, error)| Row { path: Some(path), record: zotero::Row { problems: vec![error], ..Default::default() }, ..Default::default() }).collect();
    for (file, tags) in scanned.files {
        let mut row = Row { path: Some(file.clone()), ..Default::default() };
        if let Err(error) = import_one_pdf(store, &file, &tags, target.as_ref(), &mut row) { row.record.problems.push(format!("{error:#}")) }
        rows.push(row);
    }
    Ok(report(&path, rows))
}

fn import_one_pdf(store: &mut Store, path: &Path, tags: &[String], target: Option<&Paper>, row: &mut Row) -> Result<()> {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) { bail!("请选择 PDF 文件：{}", path.display()) }
    // Reject a missing file or a disguised HTML download before adding a
    // paper. A genuine PDF without readable text is still worth collecting.
    let mut header = vec![];
    std::fs::File::open(path)?.take(1024).read_to_end(&mut header)?;
    if !crate::openaccess::is_pdf(&header) { bail!("文件没有 PDF 标识：{}", path.display()) }
    let pages = fulltext::file_pages(path);
    // The same file under another name is the paper that holds it, whether or not it names a DOI.
    let held = match target {
        Some(_) => None,
        None => store.paper_holding(path)?.map(|id| store.get_paper(&id)).transpose()?,
    };
    let paper = target.cloned().or(held).unwrap_or_else(|| Paper {
        title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        doi: pages.as_ref().ok().and_then(|pages| lookup::find_doi(&pages.iter().take(2).cloned().collect::<Vec<_>>().join("\n"))).unwrap_or_default(),
        source: "PDF 导入".into(),
        tags: tags.to_vec(),
        ..Default::default()
    });
    row.record = zotero::import_entry(store, zotero::Entry { paper, pdfs: vec![Ok(path.to_path_buf())] });
    let Some(id) = row.record.paper_id.as_deref() else { return Ok(()) };
    if !row.record.problems.is_empty() { return Ok(()) }
    let mut paper = store.get_paper(id)?;
    if tags.iter().any(|tag| !paper.tags.contains(tag)) {
        paper.tags = clean_strings(paper.tags.iter().chain(tags));
        store.save_paper(&mut paper)?;
    }
    // A repeated attachment returns the existing resource ID. Match IDs,
    // not filenames: two different attachments may have the same name.
    for file in store.paper_files(id)? {
        if row.record.file_ids.contains(&file.id) && store.needs_structure(&file.id)? {
            store.save_fulltext_pages(&file.id, &pages)?;
        }
    }
    if let Err(error) = pages { row.text_error = error }
    Ok(())
}

/// A Word file written elsewhere, read for what it cites (`restyle::inspect`): `fetch` asks Crossref about the
/// entries neither the library nor their own text settles, `entries` are records the caller read better than the rules.
fn docx_cited(store: &Store, args: &Value, fetch: Option<crate::sources::Fetch>) -> Result<(PathBuf, Vec<u8>, crate::restyle::Inspected)> {
    let path = input_path(args)?;
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx")) { bail!("这里只读 Word 的 .docx 文件：{}", path.display()) }
    let bytes = std::fs::read(&path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut given: Vec<(usize, crate::csl::Record)> = vec![];
    for entry in args["entries"].as_array().into_iter().flatten() {
        let (Some(n), Some(record)) = (entry["n"].as_u64(), entry["csl"].as_object()) else { bail!("entries 的每一项应有 n（参考文献表里的第几条）和 csl（这一条的 CSL-JSON 对象）。") };
        given.push((n as usize, record.clone()));
    }
    let papers = store.list_papers()?;
    let inspected = crate::restyle::inspect(&bytes, &papers, &given, fetch)?;
    Ok((path, bytes, inspected))
}

pub(super) fn read_docx_citations(store: &Store, args: &Value, fetch: crate::sources::Fetch) -> Result<Value> {
    let (path, _, inspected) = docx_cited(store, args, args["lookup"].as_bool().unwrap_or(false).then_some(fetch))?;
    let mut report = inspected.report();
    report["path"] = json!(path);
    let unresolved = report["unresolved"].as_array().map_or(0, Vec::len);
    report["next"] = json!(if unresolved > 0 {
        "unresolved 里的条目没能识别：它们会原样留在参考文献表末尾，引用它们的地方也保持原样。可以带 lookup: true 再读一次（联网查 Crossref），或者你自己读懂这一条的文字后用 entries 给出它的 CSL-JSON（只转写条目里写着的信息，不要补写没有的）。把识别结果和保持原样的引用如实告诉用户，再用 restyle_docx 输出。"
    } else {
        "把识别结果（来源、条目数、保持原样的引用）告诉用户，问清要用的引用样式，再用 restyle_docx 输出新文件。"
    });
    Ok(report)
}

/// The works a Word file's list cites, taken into the library (`restyle::works`): what could be read becomes a paper,
/// deduplicated as any import is; what could not is listed with its words for the caller to read and give back.
pub(super) fn import_docx_references(store: &mut Store, args: &Value, fetch: crate::sources::Fetch) -> Result<Value> {
    let only: Vec<usize> = args["entries"].as_array().into_iter().flatten().filter_map(|entry| entry["n"].as_u64()).map(|n| n as usize).collect();
    // Records given anew are taken as they are: nothing else is asked about again.
    let fetch = (args["lookup"].as_bool().unwrap_or(true) && only.is_empty()).then_some(fetch);
    let (path, _, inspected) = docx_cited(store, args, fetch)?;
    let works = crate::restyle::works(&inspected, fetch, &only);
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let papers: Vec<Paper> = works.iter().filter_map(|work| work.paper.clone()).map(|paper| Paper { source: format!("导入 · {name}"), ..paper }).collect();
    let result = store.import_papers(papers)?;
    let short = |text: &str| if text.chars().count() > 240 { format!("{}…", text.chars().take(240).collect::<String>()) } else { text.to_string() };
    let unresolved: Vec<Value> = works.iter().filter(|work| work.paper.is_none()).map(|work| json!({"n": work.n, "text": short(&work.text), "why": work.note})).collect();
    // Read by rule alone and not found anywhere: the person is to look at these.
    let unchecked: Vec<Value> = works.iter().filter(|work| work.paper.is_some() && work.origin == crate::restyle::TEXT).map(|work| json!({"n": work.n, "title": work.paper.as_ref().map_or("", |paper| paper.title.as_str()), "text": short(&work.text)})).collect();
    let mut out = json!({
        "path": path, "source": inspected.source, "entries": inspected.entries.len(),
        "added": result.added.len(), "skipped": result.skipped.len(),
        "addedPapers": result.added.iter().map(|p| json!({"id": p.id, "title": p.title, "doi": p.doi})).collect::<Vec<_>>(),
        "skippedPapers": result.skipped.iter().map(|p| json!({"title": p.title, "existingId": p.existing_id, "inTrash": p.in_trash})).collect::<Vec<_>>(),
        "unresolved": unresolved, "checkAgainst": unchecked, "notes": inspected.notes,
    });
    out["next"] = json!(match (out["unresolved"].as_array().map_or(0, Vec::len), out["checkAgainst"].as_array().map_or(0, Vec::len)) {
        (0, 0) => "告诉用户收进了多少篇、跳过了多少篇（已在库里的）。",
        (0, _) => "告诉用户收进了多少篇、跳过了多少篇；checkAgainst 里的条目只按文字读出、没能联网核对，请用户核对。",
        _ => "告诉用户收进了多少篇、跳过了多少篇，unresolved 里的条目没能识别、没有收进来。你能读懂其中的条目时，用 entries 给出它们的 CSL-JSON 再调用一次（只转写条目里写着的信息）；读不懂的如实告诉用户。checkAgainst 里的条目请用户核对。",
    });
    Ok(out)
}

/// A file a manuscript starts from, read as the window reads one handed to 写作 (`manuscript::from_word`,
/// `from_markdown`): a Word file is kept whole beside the manuscript, any other only as its words.
struct ManuscriptFile {
    name: String,
    draft: crate::manuscript::Draft,
    kind: &'static str,
    bytes: Option<Vec<u8>>,
    /// The pictures a Markdown file shows from files beside it, read (`manuscript::read_pictures`): they come
    /// into the figure library with it.
    pictures: Vec<crate::manuscript::Picture>,
}

/// The manuscript a folder holds: the one Word or Markdown file in it, or, with none there, in the folders
/// right under it. A readme is not one. With several, the caller is told which, to name one.
fn manuscript_in(folder: &Path) -> Result<PathBuf> {
    let listed = |folder: &Path| -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(['.', '~']))).collect();
        paths.sort();
        paths
    };
    let drafts = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths.iter().filter(|path| path.is_file() && path.extension().is_some_and(|e| ["docx", "md", "markdown"].iter().any(|known| e.eq_ignore_ascii_case(known))) && !path.file_stem().is_some_and(|stem| stem.eq_ignore_ascii_case("readme"))).cloned().collect()
    };
    let here = listed(folder);
    let mut found = drafts(&here);
    if found.is_empty() { found = here.iter().filter(|path| path.is_dir()).flat_map(|under| drafts(&listed(under))).collect() }
    match found.len() {
        0 => bail!("文件夹 {} 里没有稿件文件（.docx、.md）。", folder.display()),
        1 => Ok(found.remove(0)),
        n => bail!("文件夹 {} 里有 {n} 个可以作稿件的文件，请用 path 指明是哪一个：{}{}", folder.display(), found.iter().take(12).map(|path| path.strip_prefix(folder).unwrap_or(path).display().to_string()).collect::<Vec<_>>().join("、"), if n > 12 { "…" } else { "" }),
    }
}

fn manuscript_file(args: &Value) -> Result<ManuscriptFile> {
    let path = input_path(args)?;
    let path = if path.is_dir() { manuscript_in(&path)? } else { path };
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match extension.as_str() {
        "docx" | "md" | "markdown" | "txt" => {}
        "doc" => bail!("旧的 .doc 文件读不了：请先在 Word 里另存为 .docx。"),
        _ => bail!("稿件可以从 Word（.docx）、Markdown（.md）或纯文本（.txt）文件导入：{}", path.display()),
    }
    let bytes = std::fs::read(&path).with_context(|| format!("无法读取 {}", path.display()))?;
    if extension == "docx" {
        let read = crate::docx::read(&bytes)?;
        return Ok(ManuscriptFile { draft: crate::manuscript::from_word(&read, &name), name, kind: "docx", bytes: Some(bytes), pictures: vec![] });
    }
    let draft = crate::manuscript::from_markdown(&String::from_utf8_lossy(&bytes), &name);
    // No picture is written anew here: a kind that is not kept stays a line of the file's.
    let pictures = crate::manuscript::read_pictures(&draft, path.parent().unwrap_or(Path::new("")), &|_| None);
    Ok(ManuscriptFile { draft, name, kind: "markdown", bytes: None, pictures })
}

impl ManuscriptFile {
    /// The headings of a Word file without heading styles that it is cut at: those that look most like ones.
    fn guessed(&self) -> Vec<&str> {
        self.draft.headings.iter().filter(|guess| guess.likely).map(|guess| guess.text.as_str()).collect()
    }

    /// What the file holds besides its sections, as the window says it after taking one in.
    fn besides(&self, store: &Store) -> Vec<String> {
        let draft = &self.draft;
        let mut lines = vec![];
        let coming = self.pictures.iter().filter(|picture| picture.bytes.is_ok()).count();
        if coming > 0 { lines.push(format!("文件旁边的 {coming} 张图一起收进图片库，稿件里还在原处。")) }
        // Each reason once, with the pictures it is said of. One the library's own export wrote is here already.
        let mut left: Vec<(crate::manuscript::Left, Vec<&str>)> = vec![];
        for picture in &self.pictures {
            let Err(why) = picture.bytes else { continue };
            if why == crate::manuscript::Left::Missing && crate::manuscript::library_picture(&picture.address) && store.root().join(&picture.address).is_file() { continue }
            match left.iter_mut().find(|(known, _)| *known == why) {
                Some((_, addresses)) => addresses.push(&picture.address),
                None => left.push((why, vec![&picture.address])),
            }
        }
        for (why, addresses) in left {
            lines.push(format!("{} 张图没有带进来（{}）：{}{}", addresses.len(), why.said(), addresses.iter().take(6).copied().collect::<Vec<_>>().join("、"), if addresses.len() > 6 { "…" } else { "" }));
        }
        if !draft.comments.is_empty() { lines.push(format!("文件里的 {} 条批注一起带进来。", draft.comments.len())) }
        let kept: Vec<String> = [(draft.tables, "个表格"), (draft.pictures, "张图")].iter().filter(|(count, _)| *count > 0).map(|(count, what)| format!("{count} {what}")).collect();
        if !kept.is_empty() { lines.push(format!("{}留在 Word 文件里，导出时还在原处。", kept.join("、"))) }
        if draft.tracked { lines.push("文件里未接受的修订按接受后的文字读入。".to_string()) }
        if !draft.headings.is_empty() { lines.push("这个 Word 文件没有用标题样式：按看起来像标题的段落分节，导入后可以在写作页大纲里一节的菜单中调整：把一段设为标题，或并入上一节。".to_string()) }
        lines
    }

    /// The manuscript of the writing page that already is this file: a Word file by its bytes, any other by its
    /// name while not a word of it was changed.
    fn already(&self, store: &Store) -> Result<Option<crate::manuscript::Manuscript>> {
        // What a text says, line by line. Its pictures are shown from the figure library since: the addresses are not
        // compared. Nor is which lines stand right under each other: a build before `manuscript::kept_together`
        // read a table's rows, a list's items and the lines of code as paragraphs apart.
        let said = |text: &str| crate::manuscript::paragraphs(&crate::manuscript::without_addresses(text)).map(str::to_string).collect::<Vec<_>>();
        for manuscript in store.manuscripts()? {
            let Some(source) = store.manuscript_source(&manuscript.id)? else { continue };
            let same = match &self.bytes {
                Some(bytes) => {
                    let kept = store.source_file(&source);
                    source.kind == self.kind && std::fs::metadata(&kept).is_ok_and(|kept| kept.len() == bytes.len() as u64) && std::fs::read(&kept).is_ok_and(|kept| kept == *bytes)
                }
                None => source.kind == self.kind && source.name == self.name && manuscript.sections.len() == self.draft.sections.len() && manuscript.sections.iter().zip(&self.draft.sections).all(|(kept, read)| said(&kept.body) == said(&read.paragraphs.join("\n\n"))),
            };
            if same { return Ok(Some(manuscript)) }
        }
        Ok(None)
    }
}

fn manuscript_json(manuscript: &crate::manuscript::Manuscript) -> Value {
    json!({
        "id": manuscript.id, "title": manuscript.shown_title(), "language": manuscript.language,
        "sections": crate::compose::outline_of(manuscript),
    })
}

/// What `import_manuscript` would bring to the writing page, for the card that asks the person.
pub(super) fn preview_manuscript(store: &Store, args: &Value) -> Result<Vec<String>> {
    let file = manuscript_file(args)?;
    if let Some(manuscript) = file.already(store)? { return Ok(vec![format!("「{}」已经是写作页的稿件《{}》，不会再导入一份。", file.name, manuscript.shown_title())]) }
    let names: Vec<&str> = file.draft.sections.iter().map(|section| section.name.as_str()).collect();
    let (shown, more) = (names.iter().take(12).copied().collect::<Vec<_>>().join("、"), names.len().saturating_sub(12));
    let mut lines = vec![format!("把「{}」导入写作页，成为一份新稿件；原文件不会改动。", file.name), format!("{} 节：{shown}{}", names.len(), if more > 0 { "…" } else { "" })];
    lines.extend(file.besides(store));
    Ok(lines)
}

/// A manuscript of the writing page made of a file written elsewhere (`Store::import_manuscript`), as 写作's own
/// 导入 makes one: sections at the file's headings, its comments, a Word file's copy to export from. The file is
/// not changed, and one that already is a manuscript is not taken in again.
pub(super) fn import_manuscript(store: &mut Store, args: &Value) -> Result<Value> {
    let mut file = manuscript_file(args)?;
    if let Some(manuscript) = file.already(store)? {
        return Ok(json!({"already": true, "file": file.name, "manuscript": manuscript_json(&manuscript), "next": "这个文件已经是写作页的稿件，没有再导入一份。告诉用户它在写作页；要读它用 read_manuscript，要留审阅意见用 comment_manuscript。"}));
    }
    let notes = file.besides(store);
    // The pictures first: the manuscript is written with their addresses in the figure library.
    let brought = store.bring_pictures(&mut file.draft, std::mem::take(&mut file.pictures))?;
    let made = store.import_manuscript(&file.draft, file.kind, &file.name, file.bytes.as_deref())?;
    // The empty manuscript 写作 opened the library with gives way to the real one.
    let retired = store.retire_blank_manuscripts(&made.id)?;
    let draft = &file.draft;
    Ok(json!({
        "file": file.name,
        "manuscript": manuscript_json(&made),
        "comments": draft.comments.len(), "tables": draft.tables, "pictures": draft.pictures, "trackedChanges": draft.tracked,
        "figures": {"added": brought.added, "known": brought.known, "left": brought.left.iter().map(|(address, why)| json!({"address": address, "reason": why.said()})).collect::<Vec<_>>()},
        "guessedHeadings": file.guessed(),
        "retiredBlank": retired.len(),
        "notes": notes,
        "next": "稿件已在写作页，原文件没有改动。告诉用户题目、分了几节和 notes 里的情况；figures.added 和 figures.known 是收进图片库（或原来就在）并显示在稿件里的图，figures.left 里的图没有带进来，照 reason 告诉用户（可以转成 PNG 后在写作页的资料库里上传）。正文里的引用还是文件里写的文字，没有变成文献库的引用。之后它是写作页的稿件：读它用 read_manuscript，留审阅意见用 comment_manuscript，改它用 edit_section（整节重写用 write_section）。",
    }))
}

pub(super) fn restyle_docx(store: &Store, info: &LibraryInfo, args: &Value, fetch: crate::sources::Fetch) -> Result<Value> {
    let root = info.dir.parent().map(Path::to_path_buf).unwrap_or_else(|| info.dir.clone());
    let style = match (text(args, "csl")?, text(args, "style")?) {
        ("", "") => bail!("需要 style（引用样式的 key，见 list_cite_styles）或 csl（一份 CSL 定义）。"),
        ("", key) if !key.contains(':') => bail!("style 应为 CSL 样式的 key，例如 csl:nature、csl:apa、file:my-journal.csl；见 list_cite_styles。"),
        ("", key) => crate::csl::load(key, &root)?,
        (xml, _) => crate::csl::parse(xml, &root)?,
    };
    let (path, bytes, inspected) = docx_cited(store, args, args["lookup"].as_bool().unwrap_or(false).then_some(fetch))?;
    let output = match text(args, "output")? {
        "" => path.with_file_name(format!("{}-restyled.docx", path.file_stem().unwrap_or_default().to_string_lossy())),
        output => PathBuf::from(output),
    };
    if !output.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx")) { bail!("输出路径应是 .docx 文件：{}", output.display()) }
    // The file the person gave is never written over, nor any other.
    if output.exists() { bail!("{} 已存在，不会覆盖。请换一个文件名。", output.display()) }
    let options = crate::restyle::Options { track: args["track"].as_bool().unwrap_or(false), author: match text(args, "author")? { "" => "Wisp Study".into(), author => author.to_string() }, date: crate::time::now_iso(), language: text(args, "language")?.to_string() };
    let done = crate::restyle::restyle(&bytes, &inspected, &style, &options)?;
    std::fs::write(&output, &done.bytes).with_context(|| format!("无法写入 {}", output.display()))?;
    let report = inspected.report();
    Ok(json!({
        "output": output,
        "source": path,
        "style": if style.name.is_empty() { style.key.clone() } else { style.name.clone() },
        "tracked": options.track,
        "citations": {"rewritten": done.rewritten, "keptAsWritten": report["citations"]["keptAsWritten"], "sample": done.sample.iter().map(|(before, after)| json!({"before": before, "after": after})).collect::<Vec<_>>()},
        "references": {"entries": done.entries, "leftAsWritten": done.unresolved, "sample": done.list_sample},
        "unresolved": report["unresolved"],
        "notes": report["notes"],
        "next": "新文件已写好，原文件没有改动。告诉用户文件在哪里、改写了多少处引用、哪些保持原样及原因；请用户在 Word 里打开核对。",
    }))
}

/// The deck the arguments of `make_slides` describe, and what could not be put on a slide as asked.
fn deck_of(store: &Store, args: &Value) -> Result<(crate::pptx::Deck, Vec<String>)> {
    use crate::pptx::{Deck, Picture, Point, Slide};
    let paper = match text(args, "paperId")? {
        "" => None,
        id => Some(store.get_paper(id)?),
    };
    let mut deck = Deck { title: text(args, "title")?.to_string(), subtitle: text(args, "subtitle")?.to_string(), language: text(args, "language")?.to_string(), date: crate::time::now_iso(), slides: vec![] };
    if let Some(paper) = &paper {
        if deck.title.is_empty() { deck.title = paper.title.clone() }
        if deck.subtitle.is_empty() { deck.subtitle = [paper.citation(), paper.journal.clone()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ") }
    }
    if deck.title.is_empty() { bail!("需要 title（幻灯片的标题），或用 paperId 指明讲的是哪篇文献。") }
    let Some(items) = args["slides"].as_array().filter(|items| !items.is_empty()) else { bail!("需要 slides：每页一个对象，写明 title 和 bullets，要放图时给 figure。") };
    let mut notes = vec![];
    for (n, item) in items.iter().enumerate() {
        let number = n + 1;
        let field = |key: &str| match &item[key] {
            Value::Null => Ok(""),
            Value::String(value) => Ok(value.trim()),
            _ => bail!("第 {number} 页的 {key} 应为字符串。"),
        };
        let lines: Vec<&str> = match &item["bullets"] {
            Value::Null => vec![],
            // A model may write the points as one text, a line each.
            Value::String(all) => all.lines().collect(),
            Value::Array(all) => all.iter().map(|one| one.as_str().with_context(|| format!("第 {number} 页的 bullets 应为字符串的列表。"))).collect::<Result<_>>()?,
            _ => bail!("第 {number} 页的 bullets 应为字符串的列表。"),
        };
        let mut slide = Slide { title: field("title")?.into(), points: lines.into_iter().map(Point::read).filter(|point| !point.text.is_empty()).collect(), caption: field("caption")?.into(), source: field("source")?.into(), notes: field("notes")?.into(), picture: None };
        // The first point stands under none.
        if let Some(first) = slide.points.first_mut() { first.level = 0 }
        let picture = match (field("figure")?, field("image")?) {
            ("", "") => None,
            ("", image) => Some((image.to_string(), PathBuf::from(image))),
            (id, "") => {
                let figure = store.get_figure(id).map_err(|_| anyhow::anyhow!("第 {number} 页：图片库里没有 {id}。用 list_figures 查图片的 ID。"))?;
                // What the figure library knows of it: its legend under it, and the paper and page it was cut from at the foot.
                if slide.caption.is_empty() { slide.caption = [figure.label.as_str(), figure.legend.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" | ") }
                if slide.source.is_empty() && let Ok(from) = store.get_paper(&figure.paper_id) {
                    let page = if figure.page > 0 { format!("p. {}", figure.page) } else { String::new() };
                    slide.source = [from.citation(), figure.label.clone(), page].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ");
                }
                Some((id.to_string(), store.figure_path(&figure)))
            }
            _ => bail!("第 {number} 页：figure 和 image 只给一个。"),
        };
        if let Some((name, path)) = picture {
            let bytes = std::fs::read(&path).with_context(|| format!("第 {number} 页：无法读取图片 {}", path.display()))?;
            match Picture::read(bytes) {
                Ok(picture) => slide.picture = Some(picture),
                // The slide is made without it, and said.
                Err(why) => notes.push(format!("第 {number} 页没有放图 {name}：{why}。")),
            }
        }
        if slide.source.is_empty() && let Some(paper) = &paper { slide.source = paper.citation() }
        if slide.title.is_empty() && slide.points.is_empty() && slide.picture.is_none() { bail!("第 {number} 页是空的：至少给 title、bullets 或一张图。") }
        deck.slides.push(slide);
    }
    Ok((deck, notes))
}

/// A PowerPoint file given as a reference, read.
fn reference_at(path: &str) -> Result<crate::slideref::Reference> {
    let file = Path::new(path);
    if !file.extension().is_some_and(|e| e.eq_ignore_ascii_case("pptx") || e.eq_ignore_ascii_case("potx")) { bail!("参考文件应是 .pptx 或 .potx 文件：{path}（旧的 .ppt 请先在 PowerPoint 里另存为 .pptx）") }
    // Read whole: a deck of a talk, not an archive of films.
    if std::fs::metadata(file).is_ok_and(|about| about.len() > 400 << 20) { bail!("{path} 太大（超过 400 MB），读不了。") }
    let bytes = std::fs::read(file).with_context(|| format!("无法读取 {path}"))?;
    crate::slideref::Reference::read(&bytes).with_context(|| path.to_string())
}

/// The layouts of a reference that new slides stand on, by the names PowerPoint lists them under.
fn reference_layouts(reference: &crate::slideref::Reference) -> Value {
    let name = |at: usize| reference.layouts.get(at).map(|layout| if layout.name.is_empty() { layout.kind.clone() } else { layout.name.clone() }).unwrap_or_default();
    json!({"cover": name(reference.cover), "section": name(reference.opening), "content": name(reference.content)})
}

const DRAWN: &str = "参考文件里的大多数页不是用版式里的标题和正文占位符排的（文字是直接画在页面上的文本框）。新幻灯片建在它的母版和版式上，背景和主题是它的，但每一页的排法不会和它原来的页一样。";

/// 副表: rows of a table file (`table`), an attachment of a paper or a file by its path. The whole sheet is passed
/// through on every call, so that a word is found wherever it stands and the table's size is said; what is handed
/// back is a batch of rows a model can read, cut where it would get long.
pub(super) fn read_table(store: &Store, args: &Value) -> Result<Value> {
    // What the cells of one answer may come to, in bytes, and one cell of them, in characters.
    const BUDGET: usize = 36_000;
    const CELL: usize = 300;
    let (resource, path) = (text(args, "resourceId")?, text(args, "path")?);
    let (file, mut out) = match (resource.is_empty(), path.is_empty()) {
        (true, true) => bail!("请给出 resourceId（文献的表格附件，见 get_paper 的 files）或 path（本机表格文件的路径）。"),
        (false, false) => bail!("resourceId 和 path 只给一个：读文献的附件给 resourceId，读本机的文件给 path。"),
        (false, true) => {
            let Some(file) = store.list_files()?.into_iter().find(|file| file.id == resource) else { bail!("附件不存在：{resource}") };
            (store.file_path(&file), json!({"resourceId": file.id, "paperId": file.paper_id, "file": file.title}))
        }
        (true, false) => {
            let path = input_path(args)?;
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            (path.clone(), json!({"path": path, "file": name}))
        }
    };
    let book = crate::table::open(&file).map_err(anyhow::Error::msg)?;
    // A sheet by its name, or by where it stands among the sheets; a number is taken as written.
    let wanted = match &args["sheet"] {
        Value::Number(n) => n.to_string(),
        _ => text(args, "sheet")?.to_string(),
    };
    let named = book.sheets.iter().any(|name| !name.is_empty());
    let at = if wanted.is_empty() {
        0
    } else {
        let by_name = book.sheets.iter().position(|name| named && name.trim().to_lowercase() == wanted.to_lowercase());
        let by_place = || wanted.parse::<usize>().ok().filter(|n| (1..=book.sheets.len()).contains(n)).map(|n| n - 1);
        match by_name.or_else(by_place) {
            Some(at) => at,
            None if named => bail!("这个文件里没有表「{wanted}」。它的表：{}", book.sheets.join("、")),
            None => bail!("这个文件只有一个表，不用给 sheet。"),
        }
    };
    let sheet = book.sheet(at).map_err(anyhow::Error::msg)?;
    // The names of the columns are among the first rows, if anywhere.
    let mut first = vec![];
    sheet.rows(&mut |n, cells| {
        first.push((n, cells));
        first.len() < 20
    });
    let header = crate::table::header(&first).map(|at| first.swap_remove(at));
    // The columns asked for: by the name the header gives one, or by its letters.
    let mut columns = vec![];
    for name in texts(args, "columns")? {
        let by_name = header.as_ref().and_then(|(_, cells)| cells.iter().position(|cell| cell.trim().to_lowercase() == name.to_lowercase()));
        match by_name.or_else(|| crate::table::column_at(&name)) {
            Some(column) => columns.push(column),
            None => {
                let known = header.as_ref().map_or(String::new(), |(n, cells)| format!("表头（第 {n} 行）：{}", cells.iter().filter(|cell| !cell.trim().is_empty()).take(60).map(|cell| cell.trim()).collect::<Vec<_>>().join("、")));
                bail!("没有名为「{name}」的列。{known}")
            }
        }
    }
    let clip = |cell: &str| if cell.chars().count() > CELL { format!("{}…", cell.chars().take(CELL).collect::<String>()) } else { cell.to_string() };
    let pick = |cells: &[String]| -> Vec<String> {
        if columns.is_empty() { cells.iter().map(|cell| clip(cell)).collect() } else { columns.iter().map(|&column| cells.get(column).map(|cell| clip(cell)).unwrap_or_default()).collect() }
    };
    let from = number(args, "from", 1, usize::MAX)?.max(1);
    let count = number(args, "rows", 50, 500)?.max(1);
    let find = text(args, "find")?.to_lowercase();
    let (mut last, mut width, mut matches, mut used) = (0usize, 0usize, 0usize, 0usize);
    let (mut rows, mut next): (Vec<Value>, Option<usize>) = (vec![], None);
    sheet.rows(&mut |n, cells| {
        (last, width) = (n, width.max(cells.len()));
        if !find.is_empty() {
            if !cells.iter().any(|cell| cell.to_lowercase().contains(&find)) { return true }
            matches += 1;
        }
        // A full answer still counts the rest of the table.
        if n < from || next.is_some() { return true }
        let picked = pick(&cells);
        let size = 20 + picked.iter().map(|cell| cell.len() + 3).sum::<usize>();
        if rows.len() >= count || (!rows.is_empty() && used + size > BUDGET) {
            next = Some(n);
            return true;
        }
        used += size;
        rows.push(json!({"row": n, "cells": picked}));
        true
    });
    if last == 0 { bail!("这个表是空的。") }
    if find.is_empty() && from > last { bail!("这个表只到第 {last} 行。") }
    if named {
        out["sheets"] = json!(book.sheets);
        out["sheet"] = json!(book.sheets[at]);
    }
    out["rowCount"] = json!(last);
    out["columnCount"] = json!(width);
    if !columns.is_empty() { out["columns"] = json!(columns.iter().map(|&column| crate::table::column_name(column)).collect::<Vec<_>>()) }
    // The header goes along when the rows read do not show it: a row found by a word says nothing without its columns' names.
    if let Some((n, cells)) = &header
        && !rows.iter().any(|row| row["row"] == *n)
    {
        out["header"] = json!({"row": n, "cells": pick(cells)});
    }
    if !find.is_empty() { out["matches"] = json!(matches) }
    let mut note = String::new();
    if !find.is_empty() && matches == 0 { note.push_str(&format!("这个表里没有哪一行含「{}」：换一种写法（别名、编号）再找，或看看文件里别的表。", text(args, "find")?)) }
    if let Some(next) = next {
        out["next"] = json!(next);
        note.push_str(&format!("还有行没有读：带 from: {next} 再读一次{}。", if find.is_empty() { "" } else { "（find 不变）" }));
    }
    if !note.is_empty() { out["note"] = json!(note) }
    out["rows"] = json!(rows);
    Ok(out)
}

/// What a PowerPoint file says and how it looks, for making slides after it. Nothing is written.
pub(super) fn read_slides(args: &Value) -> Result<Value> {
    let path = text(args, "path")?;
    let reference = reference_at(path)?;
    let total = reference.slides.len();
    let from = (args["from"].as_u64().unwrap_or(1).max(1) as usize).min(total.max(1));
    let count = args["count"].as_u64().unwrap_or(40).clamp(1, 200) as usize;
    let clip = |text: &str, most: usize| if text.chars().count() > most { format!("{}…", text.chars().take(most).collect::<String>()) } else { text.to_string() };
    let slides: Vec<Value> = reference.slides.iter().enumerate().skip(from - 1).take(count).map(|(n, slide)| {
        // A text under another stands in by two spaces a level, as the bullets of make_slides do.
        let mut left = 1500usize;
        let mut lines = vec![];
        for (level, line) in &slide.lines {
            if left == 0 { lines.push("…".to_string()); break }
            let line = clip(line, left.min(400));
            left = left.saturating_sub(line.chars().count());
            lines.push(format!("{}{line}", "  ".repeat(*level)));
        }
        let mut one = json!({"n": n + 1, "title": slide.title, "text": lines});
        if let Some(layout) = slide.layout.and_then(|at| reference.layouts.get(at)) { one["layout"] = json!(layout.name) }
        if slide.pictures > 0 { one["pictures"] = json!(slide.pictures) }
        if !slide.notes.is_empty() { one["notes"] = json!(clip(&slide.notes, 600)) }
        one
    }).collect();
    let theme = &reference.theme;
    let shape = match reference.wide / reference.high {
        ratio if (ratio - 16.0 / 9.0).abs() < 0.02 => "16:9".to_string(),
        ratio if (ratio - 4.0 / 3.0).abs() < 0.02 => "4:3".to_string(),
        ratio if (ratio - 1.6).abs() < 0.02 => "16:10".to_string(),
        _ => format!("{:.0} × {:.0} pt", reference.wide, reference.high),
    };
    let shown = from - 1 + slides.len();
    let mut next = "这是参考，不是要讲的内容。做幻灯片时把这个文件的路径填进 make_slides 的 reference：新文件建在它的母版和版式上（asReference 里是会用到的三个版式），内容由你按文献来写；要沿用它的讲法时，照 slides 里的分节、每页的要点数和语气来排。".to_string();
    if shown < total { next.push_str(&format!("这里只列到第 {shown} 页，共 {total} 页：要看后面的，带 from: {} 再读一次。", shown + 1)) }
    let mut out = json!({
        "path": path,
        "slideCount": total,
        "shape": shape,
        "theme": {"name": theme.name, "headingFont": theme.heading, "bodyFont": theme.body, "chineseFont": theme.chinese, "text": theme.text, "background": theme.background, "accent": theme.accent},
        "layouts": reference.layouts.iter().filter(|layout| layout.used > 0).map(|layout| json!({"name": layout.name, "slides": layout.used})).collect::<Vec<_>>(),
        "asReference": reference_layouts(&reference),
        "slides": slides,
        "next": next,
    });
    if reference.drawn { out["caution"] = json!(DRAWN) }
    Ok(out)
}

/// Slides as a .pptx file where the caller says, or in the person's documents under the deck's title; no file is written over.
pub(super) fn make_slides(store: &Store, args: &Value) -> Result<Value> {
    let (deck, mut notes) = deck_of(store, args)?;
    let reference = match text(args, "reference")? {
        "" => None,
        path => Some((path, reference_at(path)?)),
    };
    let output = match text(args, "output")? {
        "" => {
            let folder = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| store.root().to_path_buf());
            let stem: String = crate::archive::file_stem(&deck.title, "slides").chars().take(80).collect();
            (1..).map(|n| folder.join(if n == 1 { format!("{stem}.pptx") } else { format!("{stem} ({n}).pptx") })).find(|path| !path.exists()).unwrap_or_default()
        }
        output => PathBuf::from(output),
    };
    if !output.extension().is_some_and(|e| e.eq_ignore_ascii_case("pptx")) { bail!("输出路径应是 .pptx 文件：{}", output.display()) }
    if output.exists() { bail!("{} 已存在，不会覆盖。请换一个文件名。", output.display()) }
    let written = match &reference {
        Some((_, reference)) => crate::pptx::write_on(&deck, reference)?,
        None => crate::pptx::write(&deck),
    };
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&output).with_context(|| format!("无法创建 {}（已有文件不会覆盖）", output.display()))?;
    std::io::Write::write_all(&mut file, &written.bytes).with_context(|| format!("无法写入 {}", output.display()))?;
    let pages = |numbers: &[usize]| numbers.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("、");
    if !written.continued.is_empty() { notes.push(format!("第 {} 页的要点一页放不下，接着排在了下一页（标题带「续」）。", pages(&written.continued))) }
    if !written.clipped.is_empty() { notes.push(format!("第 {} 页的图注太长，幻灯片上只留了前三行，全文在这一页的备注里。", pages(&written.clipped))) }
    if reference.as_ref().is_some_and(|(_, reference)| reference.drawn) { notes.push(DRAWN.to_string()) }
    let mut out = json!({
        "output": output,
        "title": deck.title,
        "slides": written.pages,
        "pictures": deck.slides.iter().filter(|slide| slide.picture.is_some()).count(),
        "notes": notes,
        "next": "文件已写好。告诉用户文件在哪里（路径照原样写出，不要写成链接）、共几页、放了几张图，以及 notes 里说的情况；请用户在 PowerPoint、Keynote 或 WPS 里打开看版面，要改哪一页告诉你。",
    });
    // Which file the look is from, and on which of its layouts the slides stand.
    if let Some((path, reference)) = &reference { out["reference"] = json!({"path": path, "layouts": reference_layouts(reference)}) }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools;

    struct Fixture { root: PathBuf, id: String }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(crate::model::new_id("wisp-local-tools"));
            let created = tools::call(&root, "create_library", &json!({"name": "植物 单细胞", "topic": "根尖", "color": "green"}), &tools::tests::offline, &tools::tests::no_download).unwrap();
            Self { root, id: created["id"].as_str().unwrap().into() }
        }

        fn call(&self, name: &str, mut args: Value) -> Result<Value, String> {
            if name != "create_library" { args["library"] = json!(self.id) }
            tools::call(&self.root, name, &args, &tools::tests::offline, &tools::tests::no_download)
        }

        fn store(&self) -> Store { Store::open(&self.root.join(&self.id)).unwrap() }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
    }

    fn pdf(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!("BT /F1 12 Tf 20 150 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        let mut bytes = "%PDF-1.4\n".to_string();
        let mut offsets = vec![];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = bytes.len();
        bytes.push_str("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets { bytes.push_str(&format!("{offset:010} 00000 n \n")) }
        bytes.push_str(&format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"));
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn creates_libraries_and_imports_reference_files_without_overwriting_edits() {
        let fixture = Fixture::new();
        assert_eq!(fixture.call("library_status", json!({})).unwrap()["topic"], "根尖");
        for args in [json!({"name": " "}), json!({"name": "x", "color": "invalid"}), json!({"name": 42})] {
            assert!(fixture.call("create_library", args).is_err());
        }
        assert_eq!(libraries::list(&fixture.root).len(), 1);
        let bib = fixture.root.join("引用 文献.bib");
        std::fs::write(&bib, "\u{FEFF}@article{a, title={Root atlas}, doi={10.1234/atlas}, year={2024}}\n@article{b, title={Leaf atlas}}").unwrap();
        let first = fixture.call("import_references", json!({"path": bib})).unwrap();
        assert_eq!((first["total"].as_u64(), first["added"].as_u64(), first["skipped"].as_u64()), (Some(2), Some(2), Some(0)));
        let id = first["addedPapers"][0]["id"].as_str().unwrap();
        let mut store = fixture.store();
        let mut held = store.get_paper(id).unwrap();
        held.notes = "Keep my notes".into();
        store.save_paper(&mut held).unwrap();
        store.trash_paper(first["addedPapers"][1]["id"].as_str().unwrap()).unwrap();
        drop(store);
        let second = fixture.call("import_references", json!({"path": bib})).unwrap();
        assert_eq!((second["added"].as_u64(), second["skipped"].as_u64(), second["skippedPapers"][1]["inTrash"].as_bool()), (Some(0), Some(2), Some(true)));
        let ris = fixture.root.join("references.ris");
        std::fs::write(&ris, "TY  - JOUR\nTI  - A changed title\nDO  - 10.1234/atlas\nER  - \nTY  - JOUR\nTI  - New RIS paper\nER  - \n").unwrap();
        let third = fixture.call("import_references", json!({"path": ris})).unwrap();
        assert_eq!((third["added"].as_u64(), third["skipped"].as_u64()), (Some(1), Some(1)));
        assert_eq!(fixture.store().get_paper(id).unwrap().notes, "Keep my notes");
        std::fs::write(&ris, "Not a reference file").unwrap();
        assert!(fixture.call("import_references", json!({"path": ris})).is_err());
        assert!(fixture.call("import_references", json!({"path": fixture.root.join("absent.bib")})).is_err());
        assert!(fixture.call("import_references", json!({"path": ""})).is_err());
        assert_eq!(fixture.store().list_papers().unwrap().len(), 2); // The other record stays in trash.
    }

    #[test]
    fn pdf_folder_reports_duplicates_trash_invalid_files_and_textless_pdfs() {
        let fixture = Fixture::new();
        let folder = fixture.root.join("PDF collection");
        let source = folder.join("植物/根/paper.pdf");
        pdf(&source, "Root cells. DOI: 10.1234/atlas");
        let bytes = std::fs::read(&source).unwrap();
        let mut store = fixture.store();
        let existing = store.add_paper(Paper { title: "Root atlas".into(), doi: "10.1234/atlas".into(), notes: "My notes".into(), ..Default::default() }).unwrap();
        drop(store);
        pdf(&folder.join("scan.pdf"), "");
        pdf(&folder.join(".hidden/hidden.pdf"), "Hidden paper");
        std::fs::write(folder.join("bad.pdf"), "<html>not a PDF</html>").unwrap();
        std::fs::write(folder.join("notes.txt"), "Not a PDF").unwrap();
        let first = fixture.call("import_pdf", json!({"path": folder})).unwrap();
        assert_eq!((first["total"].as_u64(), first["added"].as_u64(), first["skipped"].as_u64(), first["failed"].as_u64()), (Some(3), Some(1), Some(1), Some(1)));
        assert_eq!((first["filesAdded"].as_u64(), first["textless"].as_u64()), (Some(2), Some(1)));
        let held = fixture.store().get_paper(&existing.id).unwrap();
        assert_eq!((held.title.as_str(), held.notes.as_str()), ("Root atlas", "My notes"));
        assert_eq!(held.tags, ["植物", "根"]);
        let read = fixture.call("read_fulltext", json!({"paperId": existing.id})).unwrap();
        assert!(read["pages"][0]["text"].as_str().unwrap().contains("Root cells"));
        let repeated = fixture.call("import_pdf", json!({"path": source})).unwrap();
        assert_eq!((repeated["added"].as_u64(), repeated["filesAdded"].as_u64(), repeated["filesExisting"].as_u64()), (Some(0), Some(0), Some(1)));
        assert_eq!(bytes, std::fs::read(&source).unwrap());
        fixture.store().trash_paper(&existing.id).unwrap();
        let trashed = fixture.call("import_pdf", json!({"path": source})).unwrap();
        assert_eq!((trashed["inTrash"].as_u64(), trashed["filesAdded"].as_u64()), (Some(1), Some(0)));
        assert!(fixture.call("import_pdf", json!({"path": folder, "paperId": existing.id})).is_err());
        let empty = fixture.root.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(fixture.call("import_pdf", json!({"path": empty})).unwrap_err().contains("没有 PDF"));
        assert!(!import::scan_pdf_folder(&fixture.root.join("missing")).errors.is_empty());
    }

    #[test]
    fn the_same_pdf_under_another_name_is_not_a_second_paper() {
        let fixture = Fixture::new();
        let (first, second, other) = (fixture.root.join("in/Zhu 2021.pdf"), fixture.root.join("in/下载 (1).pdf"), fixture.root.join("in/other.pdf"));
        // No DOI on its pages: the record is named after the file, and nothing but the file says the two are one.
        pdf(&first, "Root cells without a number");
        std::fs::copy(&first, &second).unwrap();
        pdf(&other, "Leaf cells without a number");
        let added = fixture.call("import_pdf", json!({"path": first})).unwrap();
        assert_eq!((added["added"].as_u64(), added["filesAdded"].as_u64()), (Some(1), Some(1)));
        let id = added["results"][0]["paperId"].as_str().unwrap().to_string();
        let again = fixture.call("import_pdf", json!({"path": second})).unwrap();
        assert_eq!((again["added"].as_u64(), again["skipped"].as_u64(), again["filesAdded"].as_u64(), again["filesExisting"].as_u64()), (Some(0), Some(1), Some(0), Some(1)));
        assert_eq!(again["results"][0]["paperId"].as_str(), Some(id.as_str()));
        // A folder holding both and a third: one new paper.
        let folder = fixture.call("import_pdf", json!({"path": fixture.root.join("in")})).unwrap();
        assert_eq!((folder["total"].as_u64(), folder["added"].as_u64(), folder["skipped"].as_u64()), (Some(3), Some(1), Some(2)));
        let store = fixture.store();
        assert_eq!(store.list_papers().unwrap().len(), 2);
        assert_eq!((store.paper_holding(&second).unwrap(), store.paper_files(&id).unwrap().len()), (Some(id.clone()), 1));
        // In the trash, the paper still stands for its file.
        fixture.store().trash_paper(&id).unwrap();
        let trashed = fixture.call("import_pdf", json!({"path": second})).unwrap();
        assert_eq!((trashed["added"].as_u64(), trashed["inTrash"].as_u64()), (Some(0), Some(1)));
    }

    #[test]
    fn attaches_a_pdf_to_an_existing_paper_without_confusing_same_named_files() {
        let fixture = Fixture::new();
        let original = fixture.root.join("first/paper.pdf");
        let other = fixture.root.join("second/paper.pdf");
        pdf(&original, "Original full text");
        pdf(&other, "Supplementary text");
        let mut store = fixture.store();
        let paper = store.add_paper(Paper { title: "My paper".into(), ..Default::default() }).unwrap();
        let first = store.import_file(&paper.id, &original, "fulltext").unwrap();
        drop(store);
        let result = fixture.call("import_pdf", json!({"path": other, "paperId": paper.id})).unwrap();
        assert_eq!((result["added"].as_u64(), result["filesAdded"].as_u64(), result["failed"].as_u64()), (Some(0), Some(1), Some(0)));
        assert!(fixture.store().needs_structure(&first.id).unwrap());
        let read = fixture.call("read_fulltext", json!({"paperId": paper.id})).unwrap();
        assert!(read["pages"][0]["text"].as_str().unwrap().contains("Original full text"));
        let again = fixture.call("import_pdf", json!({"path": other, "paperId": paper.id})).unwrap();
        assert_eq!(again["filesExisting"], 1);
        assert!(fixture.call("import_pdf", json!({"path": other, "paperId": "missing"})).is_err());
        assert_eq!(fixture.store().list_papers().unwrap().len(), 1);
    }
}
