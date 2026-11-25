//! Citation styles as CSL, the definition files Zotero, Mendeley and
//! Pandoc share (`.csl`, from the Zotero style repository): how a
//! manuscript exported to Word cites in a journal's own form.
//!
//! A style is one of three: built in (`csl:<name>`, the set `hayagriva`
//! carries and the few shipped beside it, `SHIPPED`), a file the person
//! added to `styles/` in the data folder (`file:<file name>`), or one Zotero
//! itself has installed (`zotero:<file name>`, read where it is, never
//! written). A dependent style, which only names its parent, is that parent
//! when it can be found among the three. Styles that cite in footnotes are
//! refused: a manuscript here has no footnotes.
//!
//! A paper's volume, issue and pages are its record's (`entry`); one whose
//! record has none has none in any style. `render` gives the text of each citation and of the list with
//! what is set in italics or raised, and which part of a citation names
//! which paper (`cite::Rich`), so the Word file can lead from one to the
//! other.

use crate::cite::Rich;
use crate::docx::Look;
use crate::model::Paper;
use anyhow::{Context as _, Result, anyhow, bail};
use hayagriva::archive::ArchivedStyle;
use hayagriva::citationberg::json as csl_json;
use hayagriva::citationberg::{self, DatePartName, FontStyle, FontVariant, FontWeight, IndependentStyle, LayoutRenderingElement, Locale, LocaleCode, NamesChild, StyleClass, TextDecoration, VerticalAlign};
use hayagriva::types::{Date, EntryType, FormatString, MaybeTyped, Numeric, PageRanges, Person};
use hayagriva::{BibliographyDriver, BibliographyRequest, CitationItem, CitationRequest, ElemChild, ElemChildren, ElemMeta, Entry, Formatting, LocatorPayload, SpecificLocator};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::OnceLock;

/// The folder of the data root that holds the styles the person added.
pub const FOLDER: &str = "styles";

/// Styles that come with the app beside the ones `hayagriva` carries: the name a key says (`csl:<name>`) and the
/// file as the CSL project's style repository has it (CC BY-SA 3.0, THIRD_PARTY.md).
const SHIPPED: [(&str, &str); 3] = [
    ("science", include_str!("../styles/science.csl")),
    ("pnas", include_str!("../styles/pnas.csl")),
    ("the-new-england-journal-of-medicine", include_str!("../styles/the-new-england-journal-of-medicine.csl")),
];

/// The built-in styles offered first, the ones most manuscripts are asked for: those that number the works in the
/// order they are cited.
pub const NUMBERED: [&str; 19] = [
    "nature",
    "science",
    "cell",
    "pnas",
    "the-lancet",
    "the-new-england-journal-of-medicine",
    "american-medical-association",
    "british-medical-journal",
    "american-chemical-society",
    "vancouver",
    "ieee",
    "elsevier-vancouver",
    "elsevier-with-titles",
    "springer-basic",
    "springer-vancouver",
    "public-library-of-science",
    "multidisciplinary-digital-publishing-institute",
    "biomed-central",
    "gb-7714-2015-numeric",
];

/// And those that cite by author and year.
pub const BY_AUTHOR: [&str; 6] = ["apa", "chicago-author-date", "elsevier-harvard", "springer-basic-author-date", "frontiers", "gb-7714-2015-author-date"];

/// The name a built-in style is listed by where its own title is long or says little (three styles begin
/// 「China National Standard GB/T 7714-2015」 and are cut off alike in a menu): by the name its key says. In
/// Chinese; a place that shows one translates it, as with the label tables of `model`.
const SHORT: [(&str, &str); 26] = [
    ("nature", "Nature"),
    ("science", "Science"),
    ("pnas", "PNAS"),
    ("cell", "Cell"),
    ("the-lancet", "The Lancet"),
    ("the-new-england-journal-of-medicine", "NEJM"),
    ("american-medical-association", "AMA（JAMA）"),
    ("british-medical-journal", "BMJ"),
    ("american-chemical-society", "ACS"),
    ("vancouver", "Vancouver（NLM）"),
    ("ieee", "IEEE"),
    ("elsevier-vancouver", "Elsevier 顺序编码（Vancouver）"),
    ("elsevier-with-titles", "Elsevier 顺序编码（带题目）"),
    ("elsevier-harvard", "Elsevier 著者-出版年（Harvard）"),
    ("springer-basic", "Springer 顺序编码"),
    ("springer-vancouver", "Springer 顺序编码（Vancouver）"),
    ("springer-basic-author-date", "Springer 著者-出版年"),
    ("public-library-of-science", "PLOS"),
    ("frontiers", "Frontiers"),
    ("multidisciplinary-digital-publishing-institute", "MDPI"),
    ("biomed-central", "BMC"),
    ("apa", "APA 第 7 版"),
    ("chicago-author-date", "Chicago 著者-出版年"),
    ("gb-7714-2015-numeric", "GB/T 7714-2015 顺序编码"),
    ("gb-7714-2015-author-date", "GB/T 7714-2015 著者-出版年"),
    ("gb-7714-2005-numeric", "GB/T 7714-2005 顺序编码"),
];

/// The built-in styles that cite in footnotes, which a manuscript here has none of (`load` refuses them): known by
/// name, so that a list of styles to choose from leaves them out without reading every style.
const IN_NOTES: [&str; 5] = ["chicago-notes", "chicago-shortened-notes", "gb-7714-2015-note", "modern-humanities-research-association-notes", "turabian-fullnote-8"];

/// Whether a key names a built-in style that cites in footnotes (`IN_NOTES`).
pub fn in_notes(key: &str) -> bool {
    key.strip_prefix("csl:").and_then(Builtin::by_name).is_some_and(|style| IN_NOTES.contains(&style.name()))
}

/// A style that can be chosen: the key the settings keep, and its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub key: String,
    pub name: String,
}

/// A style read and ready to cite with.
pub struct Loaded {
    pub key: String,
    pub name: String,
    style: IndependentStyle,
}

impl Loaded {
    /// Whether it numbers the works it cites, as its file says of itself (`citation-format`).
    pub fn numbered(&self) -> bool {
        matches!(self.style.info.category.iter().find_map(|category| match category {
            citationberg::StyleCategory::CitationFormat { format } => Some(*format),
            _ => None,
        }), Some(citationberg::CitationFormat::Numeric))
    }
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Loaded({})", self.key)
    }
}

impl PartialEq for Loaded {
    fn eq(&self, other: &Loaded) -> bool {
        self.key == other.key
    }
}

/// A built-in style: one `hayagriva` carries, or one shipped beside them (its place in `SHIPPED`).
#[derive(Clone, Copy)]
enum Builtin {
    Archived(ArchivedStyle),
    Shipped(usize),
}

impl Builtin {
    /// By the name a key says: any of the names `hayagriva` knows a style by (`apa`), or a shipped one's.
    fn by_name(name: &str) -> Option<Builtin> {
        ArchivedStyle::by_name(name).map(Builtin::Archived).or_else(|| SHIPPED.iter().position(|(shipped, _)| *shipped == name).map(Builtin::Shipped))
    }

    /// By the `<id>` a style's file gives itself.
    fn by_id(id: &str) -> Option<Builtin> {
        ArchivedStyle::by_id(id).map(Builtin::Archived).or_else(|| SHIPPED.iter().position(|(_, xml)| tagged(xml, "id").as_deref() == Some(id)).map(Builtin::Shipped))
    }

    fn all() -> Vec<Builtin> {
        ArchivedStyle::all().iter().copied().map(Builtin::Archived).chain((0..SHIPPED.len()).map(Builtin::Shipped)).collect()
    }

    /// The one name its key says.
    fn name(self) -> &'static str {
        match self {
            Builtin::Archived(style) => style.names()[0],
            Builtin::Shipped(n) => SHIPPED[n].0,
        }
    }

    /// Its title, as its file gives it.
    fn title(self) -> String {
        match self {
            Builtin::Archived(style) => style.display_name().to_string(),
            Builtin::Shipped(n) => tagged(SHIPPED[n].1, "title").unwrap_or_else(|| SHIPPED[n].0.to_string()),
        }
    }

    fn style(self) -> Option<citationberg::Style> {
        match self {
            Builtin::Archived(style) => Some(style.get()),
            Builtin::Shipped(n) => citationberg::Style::from_xml(SHIPPED[n].1).ok(),
        }
    }

    fn xml(self) -> Result<String> {
        match self {
            // Written out, it names the namespace of CSL as a file of the style repository does: Zotero takes no file without it.
            Builtin::Archived(style) => style.get().to_xml().map(|xml| crate::adjust::in_namespace(&xml)).map_err(|err| anyhow!("内置样式 {} 写不出来：{err}", self.name())),
            Builtin::Shipped(n) => Ok(SHIPPED[n].1.to_string()),
        }
    }

    fn listed(self) -> Listed {
        Listed { key: format!("csl:{}", self.name()), name: self.title() }
    }
}

fn named(names: &[&str]) -> Vec<Listed> {
    names.iter().filter_map(|name| Builtin::by_name(name)).map(Builtin::listed).collect()
}

/// `NUMBERED`, as they are listed.
pub fn common_numbered() -> Vec<Listed> {
    named(&NUMBERED)
}

/// `BY_AUTHOR`, as they are listed.
pub fn common_by_author() -> Vec<Listed> {
    named(&BY_AUTHOR)
}

/// The built-in styles offered first: `NUMBERED`, then `BY_AUTHOR`.
pub fn common() -> Vec<Listed> {
    let mut all = common_numbered();
    all.extend(common_by_author());
    all
}

/// Every built-in style, by name.
pub fn bundled() -> Vec<Listed> {
    let mut all: Vec<Listed> = Builtin::all().into_iter().map(Builtin::listed).collect();
    all.sort_by_key(|style| style.name.to_lowercase());
    all
}

/// The short name a built-in style is listed by (`SHORT`), in Chinese; none for a style listed by its own title.
pub fn short_name(key: &str) -> Option<&'static str> {
    let name = Builtin::by_name(key.strip_prefix("csl:")?)?.name();
    SHORT.iter().find(|(short, _)| Builtin::by_name(short).is_some_and(|style| style.name() == name)).map(|(_, said)| *said)
}

/// Where Zotero keeps the styles it has installed, unless the person moved its data.
pub fn zotero_folder() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Zotero").join("styles")).filter(|folder| folder.is_dir())
}

/// What stands between `<name>` and `</name>` the first time, as a style's own file says its title and id.
fn tagged(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = start + xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..end].trim().replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&apos;", "'").replace("&quot;", "\"")).filter(|text| !text.is_empty())
}

fn files(folder: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(folder) else { return vec![] };
    let mut found: Vec<(String, String)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csl")))
        .filter_map(|entry| Some((entry.file_name().to_str()?.to_string(), std::fs::read_to_string(entry.path()).ok()?)))
        .collect();
    found.sort();
    found
}

/// The styles beside the built-in ones: those added to the data folder,
/// then those Zotero has that are neither built in nor added.
pub fn added(root: &Path) -> Vec<Listed> {
    added_from(root, zotero_folder().as_deref())
}

fn added_from(root: &Path, zotero: Option<&Path>) -> Vec<Listed> {
    let name = |file: &str, xml: &str| tagged(xml, "title").unwrap_or_else(|| file.trim_end_matches(".csl").to_string());
    let own = files(&root.join(FOLDER));
    let mut out: Vec<Listed> = own.iter().map(|(file, xml)| Listed { key: format!("file:{file}"), name: name(file, xml) }).collect();
    let ids: Vec<String> = own.iter().filter_map(|(_, xml)| tagged(xml, "id")).collect();
    for (file, xml) in zotero.map(files).unwrap_or_default() {
        let id = tagged(&xml, "id").unwrap_or_default();
        if Builtin::by_id(&id).is_some() || ids.contains(&id) || own.iter().any(|(mine, _)| *mine == file) { continue }
        out.push(Listed { key: format!("zotero:{file}"), name: name(&file, &xml) });
    }
    out
}

/// The name a key is shown by; none for a key that names no style any more.
pub fn name_of(key: &str, root: &Path) -> Option<String> {
    match key.strip_prefix("csl:") {
        Some(name) => Builtin::by_name(name).map(Builtin::title),
        None => added(root).into_iter().find(|style| style.key == key).map(|style| style.name),
    }
}

/// The file a key of a style outside the built-in ones names.
fn file_of(key: &str, root: &Path, zotero: Option<&Path>) -> Option<PathBuf> {
    let (folder, file) = match (key.strip_prefix("file:"), key.strip_prefix("zotero:")) {
        (Some(file), _) => (root.join(FOLDER), file),
        (_, Some(file)) => (zotero?.to_path_buf(), file),
        _ => return None,
    };
    // A key is a file's name, never a way out of the folder.
    if file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.') { return None }
    Some(folder.join(file))
}

/// A style's file as the style to cite with: itself, or the parent a dependent one names.
fn independent(xml: &str, root: &Path, zotero: Option<&Path>) -> Result<IndependentStyle> {
    let style = citationberg::Style::from_xml(xml).map_err(|err| anyhow!("这不是能读的 CSL 样式文件：{err}"))?;
    let style = match style {
        citationberg::Style::Independent(style) => style,
        citationberg::Style::Dependent(dependent) => {
            let parent = dependent.parent_link.href.clone();
            let file = format!("{}.csl", parent.trim_end_matches('/').rsplit('/').next().unwrap_or_default());
            let beside = [Some(root.join(FOLDER)), zotero.map(Path::to_path_buf)].into_iter().flatten().find_map(|folder| std::fs::read_to_string(folder.join(&file)).ok());
            let found = match (Builtin::by_id(&parent), beside) {
                (Some(builtin), _) => builtin.style(),
                (None, Some(xml)) => citationberg::Style::from_xml(&xml).ok(),
                (None, None) => None,
            };
            match found {
                Some(citationberg::Style::Independent(style)) => style,
                _ => bail!("这个样式只是「{}」的别名，本身没有格式定义。请一并添加 {file}（在 Zotero 样式库里搜同名样式）。", tagged(xml, "title").unwrap_or_default()),
            }
        }
    };
    if style.settings.class == StyleClass::Note { bail!("这是脚注式的引用样式，稿件里没有脚注，暂不支持。") }
    Ok(mended(style))
}

/// A style with the two things `hayagriva` (0.10.1) reads wrong said another way that means the same, so that what
/// is written is what the style says. Every style cited with goes through here, built in or a file.
///
/// A `<names>` with child elements and no `<name>` among them (`<names variable="author"><label/><substitute>…`,
/// which CSL allows: the names are then written with the options inherited from `<style>`) is taken for one that
/// writes nothing (`Names::will_have_info`), so NLM's entries had no authors, and where such names were written
/// all the same (their `<substitute>` had something to say) the label stood before them (`, editorJones M`). It
/// gets a `<name/>` in front. One with no child elements at all is left alone: in a `<substitute>` that is the
/// short form that takes the `<name>` of the element it stands in for.
///
/// A date's `delimiter` is written before a part the date turns out not to have (`2024 ;10(3)` of a year alone
/// in a form of year, month and day): `spaced` says it as affixes of the parts.
fn mended(mut style: IndependentStyle) -> IndependentStyle {
    for one in &mut style.macros { mend(&mut one.children) }
    mend(&mut style.citation.layout.elements);
    if let Some(list) = &mut style.bibliography { mend(&mut list.layout.elements) }
    for locale in &mut style.locale { locale.date.iter_mut().for_each(spaced) }
    style
}

fn mend(elements: &mut [LayoutRenderingElement]) {
    for element in elements {
        match element {
            LayoutRenderingElement::Names(names) => {
                if !names.children.is_empty() && names.name().is_none() { names.children.insert(0, NamesChild::Name(Default::default())) }
                for child in &mut names.children {
                    if let NamesChild::Substitute(instead) = child { mend(&mut instead.children) }
                }
            }
            // One that names a form of the locale takes that form's delimiter, not its own.
            LayoutRenderingElement::Date(date) if !date.is_localized() => spaced(date),
            LayoutRenderingElement::Group(group) => mend(&mut group.children),
            LayoutRenderingElement::Choose(choose) => {
                mend(&mut choose.if_.children);
                for branch in &mut choose.else_if { mend(&mut branch.children) }
                if let Some(otherwise) = &mut choose.otherwise { mend(&mut otherwise.children) }
            }
            _ => {}
        }
    }
}

/// A date's delimiter as affixes of its parts, on the side of each that faces the coarsest one: a date that has a
/// part has the coarser ones too (a day comes with its month, and every date has a year), so each part that is
/// written brings the one delimiter between it and its neighbour, and a part that is missing brings none. A part
/// set in a look of its own (bold, say) now has its delimiter in that look.
fn spaced(date: &mut citationberg::Date) {
    let Some(delimiter) = date.delimiter.clone().filter(|delimiter| !delimiter.is_empty() && date.date_part.len() > 1) else { return };
    let rank = |name: DatePartName| match name {
        DatePartName::Year => 0,
        DatePartName::Month => 1,
        DatePartName::Day => 2,
    };
    let Some(coarsest) = (0..date.date_part.len()).min_by_key(|n| rank(date.date_part[*n].name)) else { return };
    for (n, part) in date.date_part.iter_mut().enumerate() {
        if n < coarsest { part.affixes.suffix = Some(format!("{}{delimiter}", part.affixes.suffix.as_deref().unwrap_or_default())) }
        if n > coarsest { part.affixes.prefix = Some(format!("{delimiter}{}", part.affixes.prefix.as_deref().unwrap_or_default())) }
    }
    date.delimiter = None;
}

/// The style a key names.
pub fn load(key: &str, root: &Path) -> Result<Loaded> {
    load_from(key, root, zotero_folder().as_deref())
}

fn load_from(key: &str, root: &Path, zotero: Option<&Path>) -> Result<Loaded> {
    if let Some(name) = key.strip_prefix("csl:") {
        let builtin = Builtin::by_name(name).ok_or_else(|| anyhow!("没有这个内置引用样式：{name}"))?;
        let Some(citationberg::Style::Independent(style)) = builtin.style() else { bail!("内置样式 {name} 读不了") };
        if style.settings.class == StyleClass::Note { bail!("这是脚注式的引用样式，稿件里没有脚注，暂不支持。") }
        return Ok(Loaded { key: key.to_string(), name: builtin.title(), style: mended(style) });
    }
    let path = file_of(key, root, zotero).ok_or_else(|| anyhow!("不认识这个引用样式：{key}"))?;
    let xml = std::fs::read_to_string(&path).with_context(|| format!("引用样式文件不在了：{}", path.display()))?;
    let name = tagged(&xml, "title").unwrap_or_else(|| key.to_string());
    Ok(Loaded { key: key.to_string(), name, style: independent(&xml, root, zotero)? })
}

/// Take a `.csl` file into the data folder's styles, once it is found to be one that can be cited with.
pub fn add(root: &Path, file: &Path) -> Result<Listed> {
    let xml = std::fs::read_to_string(file).with_context(|| format!("读不了 {}", file.display()))?;
    let name = file.file_name().and_then(|name| name.to_str()).ok_or_else(|| anyhow!("文件名读不了"))?;
    save(root, name, &xml)
}

/// Keep a style's XML under a file name in the data folder's styles, once
/// it is found to be one that can be cited with. A file of that name that
/// says something else is kept first, in `styles/removed/`.
pub fn save(root: &Path, name: &str, xml: &str) -> Result<Listed> {
    independent(xml, root, zotero_folder().as_deref())?;
    let name = name.trim();
    let name = if name.to_lowercase().ends_with(".csl") { name.to_string() } else { format!("{name}.csl") };
    if name.len() <= 4 || name.starts_with('.') || name.contains(['/', '\\', ':']) { bail!("样式的文件名不能用：{name}") }
    let folder = root.join(FOLDER);
    std::fs::create_dir_all(&folder).with_context(|| format!("无法创建样式文件夹 {}", folder.display()))?;
    let file = folder.join(&name);
    if let Ok(before) = std::fs::read_to_string(&file) && before != xml {
        let kept = folder.join("removed");
        std::fs::create_dir_all(&kept)?;
        std::fs::write(kept.join(format!("{}-{name}", crate::time::now_iso().replace(':', "-"))), before)?;
    }
    std::fs::write(&file, xml).with_context(|| format!("无法保存 {name}"))?;
    Ok(Listed { key: format!("file:{name}"), name: tagged(xml, "title").unwrap_or_else(|| name.trim_end_matches(".csl").to_string()) })
}

/// Take a style the person added out of the list: its file goes to `styles/removed/`, and what it said is
/// given back, so that `save` under the same name puts it back as it was.
pub fn remove(root: &Path, key: &str) -> Result<(String, String)> {
    let name = key.strip_prefix("file:").ok_or_else(|| anyhow!("只有自己添加的引用样式可以移除。"))?;
    let file = file_of(key, root, None).ok_or_else(|| anyhow!("不认识这个引用样式：{key}"))?;
    let xml = std::fs::read_to_string(&file).with_context(|| format!("引用样式文件不在了：{}", file.display()))?;
    let kept = root.join(FOLDER).join("removed");
    std::fs::create_dir_all(&kept)?;
    std::fs::rename(&file, kept.join(format!("{}-{name}", crate::time::now_iso().replace(':', "-")))).with_context(|| format!("无法移走 {name}"))?;
    Ok((name.to_string(), xml))
}

/// The file of a style the person added, to show it where it is kept.
pub fn file(root: &Path, key: &str) -> Option<PathBuf> {
    key.starts_with("file:").then(|| file_of(key, root, None)).flatten()
}

/// A style's definition as its file says it, a built-in one written out: what a changed style starts from.
pub fn xml_of(key: &str, root: &Path) -> Result<String> {
    if let Some(name) = key.strip_prefix("csl:") {
        return Builtin::by_name(name).ok_or_else(|| anyhow!("没有这个内置引用样式：{name}"))?.xml();
    }
    let path = file_of(key, root, zotero_folder().as_deref()).ok_or_else(|| anyhow!("不认识这个引用样式：{key}"))?;
    std::fs::read_to_string(&path).with_context(|| format!("引用样式文件不在了：{}", path.display()))
}

/// A style from its XML alone, kept nowhere: for trying one out.
pub fn parse(xml: &str, root: &Path) -> Result<Loaded> {
    Ok(Loaded { key: String::new(), name: tagged(xml, "title").unwrap_or_default(), style: independent(xml, root, zotero_folder().as_deref())? })
}

/// The style the settings name: the two simple ones of `cite::Style`, or a CSL style.
pub fn style(key: &str, root: &Path) -> Result<crate::cite::Style> {
    if key.contains(':') { Ok(crate::cite::Style::Csl(std::sync::Arc::new(load(key, root)?))) } else { Ok(crate::cite::Style::from_key(key)) }
}

// ---- citing

fn locales() -> &'static [Locale] {
    static LOCALES: OnceLock<Vec<Locale>> = OnceLock::new();
    // A locale's date forms are mended as a style's own are (`mended`): Lithuanian and Latvian ones have a delimiter.
    LOCALES.get_or_init(|| {
        let mut all = hayagriva::archive::locales();
        for locale in &mut all { locale.date.iter_mut().for_each(spaced) }
        all
    })
}

/// The locale a manuscript's language asks a style's terms in (`et al.`, `and`); the style's own for English.
fn locale(language: &str) -> Option<LocaleCode> {
    let code = match language {
        "zh" => "zh-CN",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "es" => "es-ES",
        "ja" => "ja-JP",
        _ => return None,
    };
    Some(LocaleCode(code.into()))
}

/// A paper as a CSL processor reads one: an article, in its journal when it has one.
/// The volume and issue are the journal's, the pages the article's.
fn entry(paper: &Paper) -> Entry {
    let mut entry = Entry::new(&paper.id, EntryType::Article);
    entry.set_title(FormatString::with_value(paper.title.trim()));
    let authors: Vec<Person> = crate::model::author_names(&paper.authors).into_iter().map(|(family, given)| Person { name: family, given_name: Some(given).filter(|given| !given.is_empty()), prefix: None, suffix: None, comma_suffix: false, alias: None }).collect();
    if !authors.is_empty() { entry.set_authors(authors) }
    if let Some(year) = paper.year { entry.set_date(Date::from_year(year)) }
    if !paper.doi.trim().is_empty() { entry.set_doi(paper.doi.trim().to_string()) }
    if !paper.pmid.trim().is_empty() { entry.set_pmid(paper.pmid.trim().to_string()) }
    if !paper.journal.trim().is_empty() {
        let mut journal = Entry::new("journal", EntryType::Periodical);
        journal.set_title(FormatString::with_value(paper.journal.trim()));
        if let Some(volume) = numbered(&paper.volume) { journal.set_volume(volume) }
        if let Some(issue) = numbered(&paper.issue) { journal.set_issue(issue) }
        entry.set_parents(vec![journal]);
    }
    // A range a style can shorten or give an en dash; an article number as it is written.
    if !paper.pages.trim().is_empty() { entry.set_page_range(MaybeTyped::<PageRanges>::from_str(paper.pages.trim()).unwrap_or_else(|never| match never {})) }
    entry
}

/// A volume or issue: a number where it is one (`585`, `12-13`), else the words as written (`S1`).
fn numbered(value: &str) -> Option<MaybeTyped<Numeric>> {
    let value = value.trim();
    (!value.is_empty()).then(|| MaybeTyped::from_str(value).unwrap_or_else(|never| match never {}))
}

fn look(formatting: &Formatting) -> Look {
    Look {
        italic: matches!(formatting.font_style, FontStyle::Italic),
        bold: matches!(formatting.font_weight, FontWeight::Bold),
        sup: matches!(formatting.vertical_align, VerticalAlign::Sup),
        sub: matches!(formatting.vertical_align, VerticalAlign::Sub),
        small_caps: matches!(formatting.font_variant, FontVariant::SmallCaps),
        underline: matches!(formatting.text_decoration, TextDecoration::Underline),
        ..Default::default()
    }
}

/// What a processor rendered, as one line of text with its looks; the parts that are one cited item are linked to `ids[n]`.
fn flatten(children: &ElemChildren, ids: &[&str], out: &mut Rich) {
    let push = |out: &mut Rich, text: &str, formatting: Option<&Formatting>| {
        let start = out.text.len();
        // One paragraph of a Word file: a style's line breaks are spaces.
        out.text.extend(text.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }));
        let look = formatting.map(look).unwrap_or_default();
        if look == Look::default() || out.text.len() == start { return }
        match out.looks.last_mut() {
            Some((range, last)) if *last == look && range.end == start => range.end = out.text.len(),
            _ => out.looks.push((start..out.text.len(), look)),
        }
    };
    for child in &children.0 {
        match child {
            ElemChild::Text(text) => push(out, &text.text, Some(&text.formatting)),
            ElemChild::Link { text, .. } => push(out, &text.text, Some(&text.formatting)),
            ElemChild::Markup(text) => push(out, text, None),
            ElemChild::Transparent { .. } => {}
            ElemChild::Elem(elem) => {
                let start = out.text.len();
                flatten(&elem.children, ids, out);
                let Some(ElemMeta::Entry(n)) = elem.meta else { continue };
                let said = &out.text[start..];
                let (lead, kept) = (said.len() - said.trim_start().len(), said.trim().len());
                if let Some(id) = ids.get(n).filter(|_| kept > 0) { out.links.push((start + lead..start + lead + kept, id.to_string())) }
            }
        }
    }
}

/// The text without the spaces around it, and its ranges kept inside it.
fn trimmed(mut rich: Rich) -> Rich {
    while rich.text.contains("  ") { rich = squeezed(rich) }
    let end = rich.text.trim_end().len();
    rich.text.truncate(end);
    let lead = rich.text.len() - rich.text.trim_start().len();
    rich.text.drain(..lead);
    let fit = |range: &std::ops::Range<usize>| range.start.saturating_sub(lead).min(end - lead)..range.end.saturating_sub(lead).min(end - lead);
    rich.looks = rich.looks.iter().map(|(range, look)| (fit(range), *look)).filter(|(range, _)| !range.is_empty()).collect();
    rich.links = rich.links.iter().map(|(range, to)| (fit(range), to.clone())).filter(|(range, _)| !range.is_empty()).collect();
    rich
}

/// Two spaces in a row, which a line break beside a space leaves, as one.
fn squeezed(rich: Rich) -> Rich {
    let Some(at) = rich.text.find("  ") else { return rich };
    let shift = |n: usize| if n > at { n - 1 } else { n };
    let mut text = rich.text;
    text.remove(at);
    Rich { text, looks: rich.looks.into_iter().map(|(range, look)| (shift(range.start)..shift(range.end), look)).collect(), links: rich.links.into_iter().map(|(range, to)| (shift(range.start)..shift(range.end), to)).collect() }
}

/// The citations of a manuscript in a style: each group of papers as it is
/// written in the text, in the order given, and the reference list with the
/// paper each entry is (empty for a style that has none).
pub fn render(style: &Loaded, groups: &[Vec<&Paper>], language: &str) -> (Vec<Rich>, Vec<(String, Rich)>) {
    let mut entries: HashMap<&str, Entry> = HashMap::new();
    for paper in groups.iter().flatten() { entries.entry(paper.id.as_str()).or_insert_with(|| entry(paper)); }
    let locale = locale(language);
    let mut driver = BibliographyDriver::new();
    for group in groups {
        let items = group.iter().map(|paper| CitationItem::with_entry(&entries[paper.id.as_str()])).collect();
        driver.citation(CitationRequest::new(items, &style.style, locale.clone(), locales(), None));
    }
    let rendered = driver.finish(BibliographyRequest::new(&style.style, locale, locales()));
    let ids: Vec<Vec<&str>> = groups.iter().map(|group| group.iter().map(|paper| paper.id.as_str()).collect()).collect();
    written(rendered, &ids)
}

/// What the processor made, as texts: each citation with its items linked to `ids`, and the list.
fn written(rendered: hayagriva::Rendered, ids: &[Vec<&str>]) -> (Vec<Rich>, Vec<(String, Rich)>) {
    let cited = rendered
        .citations
        .iter()
        .zip(ids)
        .map(|(citation, ids)| {
            let mut rich = Rich::default();
            flatten(&citation.citation, ids, &mut rich);
            trimmed(rich)
        })
        .collect();
    let list = rendered
        .bibliography
        .map(|bibliography| {
            bibliography
                .items
                .iter()
                .map(|item| {
                    let mut rich = Rich::default();
                    // The number a numbered list sets apart before each entry: `1.`, `[1]`.
                    if let Some(first) = &item.first_field {
                        flatten(&ElemChildren(vec![first.clone()]), &[], &mut rich);
                        rich.text.push(' ');
                    }
                    flatten(&item.content, &[], &mut rich);
                    (item.key.clone(), trimmed(rich))
                })
                .collect()
        })
        .unwrap_or_default();
    (cited, list)
}

// ---- works that are no papers of the library

/// A work as CSL-JSON says one (`type`, `title`, `author` with `family` and
/// `given`, `issued` with `date-parts`, `container-title`, `volume`, `page`,
/// `DOI`…): what Zotero writes into a Word file for each citation, and what
/// a reference read from a file's own list is made into. Its `id` is what a
/// citation names it by.
pub type Record = serde_json::Map<String, serde_json::Value>;

/// One work of a citation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cite {
    /// The record's `id`.
    pub id: String,
    /// The page a citation names: `p. 12` in most styles.
    pub locator: String,
    /// The authors are said in the sentence: `Zhang et al. (2024)`.
    pub narrative: bool,
}

/// A paper of the library as a record.
pub fn record_of(paper: &Paper) -> Record {
    let mut record = Record::new();
    let mut set = |name: &str, value: &str| {
        if !value.trim().is_empty() { record.insert(name.into(), value.trim().into()); }
    };
    set("id", &paper.id);
    set("type", "article-journal");
    set("title", &paper.title);
    set("container-title", &paper.journal);
    set("volume", &paper.volume);
    set("issue", &paper.issue);
    set("page", &paper.pages);
    set("DOI", &paper.doi);
    set("PMID", &paper.pmid);
    let authors: Vec<serde_json::Value> = crate::model::author_names(&paper.authors).into_iter().map(|(family, given)| if given.is_empty() { serde_json::json!({"family": family}) } else { serde_json::json!({"family": family, "given": given}) }).collect();
    if !authors.is_empty() { record.insert("author".into(), authors.into()); }
    if let Some(year) = paper.year { record.insert("issued".into(), serde_json::json!({"date-parts": [[year]]})); }
    record
}

/// A record as a paper of the library, the other way from `record_of`: what a Word file's reference becomes when
/// its works are taken in. None without a title. What a record says that a paper may not hold (a DOI or PMID of
/// another shape, a year out of range) is left out rather than costing the paper.
pub fn paper_of(record: &Record) -> Option<Paper> {
    use serde_json::Value;
    let text = |name: &str| -> String {
        match record.get(name) {
            Some(Value::String(text)) => text.trim().to_string(),
            Some(Value::Number(number)) => number.to_string(),
            Some(Value::Array(items)) => items.iter().find_map(Value::as_str).unwrap_or("").trim().to_string(),
            _ => String::new(),
        }
    };
    let title = crate::model::strip_markup(&text("title"));
    if title.trim().is_empty() { return None }
    let authors: Vec<String> = record
        .get("author")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| {
            let part = |name: &str| author[name].as_str().unwrap_or("").trim();
            match (part("family"), part("given"), part("literal")) {
                ("", _, "") => None,
                ("", _, literal) => Some(literal.to_string()),
                (family, "", _) => Some(family.to_string()),
                // 张 and 三 are one name written together; a Latin name is kept as `Zhang, Wei`, the family name first.
                (family, given, _) if family.chars().chain(given.chars()).all(|c| !c.is_ascii_alphabetic()) => Some(format!("{family}{given}")),
                (family, given, _) => Some(format!("{family}, {given}")),
            }
        })
        .collect();
    let year = match record.get("issued") {
        Some(Value::Object(issued)) => issued.get("date-parts").and_then(|parts| parts.get(0)?.get(0).cloned()).or_else(|| issued.get("raw").or(issued.get("literal")).cloned()),
        other => other.cloned(),
    };
    let year = match year {
        Some(Value::Number(year)) => year.as_i64().map(|year| year as i32),
        Some(Value::String(text)) => text.split(|c: char| !c.is_ascii_digit()).find(|digits| digits.len() == 4).and_then(|digits| digits.parse().ok()),
        _ => None,
    };
    let doi = crate::model::normalize_doi(&text("DOI"));
    let pmid = crate::model::normalize_pmid(&text("PMID"));
    let publication_status = match text("type").as_str() {
        "article" => "preprint",
        "article-journal" | "paper-conference" | "book" | "chapter" | "thesis" | "report" if !text("container-title").is_empty() || !text("publisher").is_empty() || !doi.is_empty() => "published",
        _ => "unknown",
    };
    Some(Paper {
        title,
        authors: authors.join("; "),
        year: year.filter(|year| (1800..=2200).contains(year)),
        journal: [text("container-title"), text("publisher")].into_iter().find(|name| !name.is_empty()).unwrap_or_default(),
        volume: text("volume"),
        issue: text("issue"),
        pages: crate::model::clean_pages(&text("page")),
        doi: if crate::model::valid_doi(&doi) { doi } else { String::new() },
        pmid: if pmid.bytes().all(|b| b.is_ascii_digit()) { pmid } else { String::new() },
        url: text("URL"),
        abstract_text: text("abstract"),
        publication_status: publication_status.into(),
        ..Default::default()
    })
}

/// A record as the processor takes one: every field it can read, the others left out. Zotero's records hold fields of
/// its own (`accessed` without a date, lists of keywords) that are no CSL, and one such field is not to cost the record.
fn item(record: &Record) -> csl_json::Item {
    let mut fields = std::collections::BTreeMap::new();
    for (name, value) in record {
        // A year alone, as a number or as text: `{"date-parts": [["2024"]]}` is what Zotero writes, `2024` what a person would.
        let value = match (name.as_str(), value) {
            ("issued", serde_json::Value::Number(year)) => serde_json::json!({"date-parts": [[year]]}),
            ("issued", serde_json::Value::String(year)) if year.trim().parse::<i32>().is_ok() => serde_json::json!({"date-parts": [[year.trim().parse::<i32>().unwrap_or(0)]]}),
            _ => value.clone(),
        };
        if let Ok(value) = serde_json::from_value::<csl_json::Value>(value) { fields.insert(name.clone(), value); }
    }
    csl_json::Item(fields)
}

fn id_of(record: &Record) -> String {
    match record.get("id") {
        Some(serde_json::Value::String(id)) => id.clone(),
        Some(serde_json::Value::Number(id)) => id.to_string(),
        _ => String::new(),
    }
}

/// Whether a style's citation says the year: an author-date style, in which `Zhang et al. (2024)` keeps its authors
/// in the sentence. A numbered style or one with labels puts its whole citation there.
fn says_year(style: &Loaded) -> bool {
    let mut probe = Record::new();
    probe.insert("id".into(), "probe".into());
    probe.insert("type".into(), "article-journal".into());
    probe.insert("title".into(), "Probe".into());
    probe.insert("author".into(), serde_json::json!([{"family": "Zzyzx", "given": "Q"}]));
    probe.insert("issued".into(), serde_json::json!({"date-parts": [[1987]]}));
    let item = item(&probe);
    let mut driver = BibliographyDriver::new();
    driver.citation(CitationRequest::new(vec![CitationItem::with_entry(&item)], &style.style, None, locales(), None));
    let rendered = driver.finish(BibliographyRequest::new(&style.style, None, locales()));
    let mut rich = Rich::default();
    if let Some(citation) = rendered.citations.first() { flatten(&citation.citation, &[], &mut rich) }
    rich.text.contains("1987")
}

/// The citations of a text whose works are records: each group as it is
/// written, in the order given, and the list with each entry's record id.
/// `uncited` are works the list is to hold though no group names them, after
/// the others. A work a group names that `records` does not hold is left out
/// of its citation.
pub fn render_records(style: &Loaded, groups: &[Vec<Cite>], uncited: &[String], records: &[Record], language: &str) -> (Vec<Rich>, Vec<(String, Rich)>) {
    let items: HashMap<String, csl_json::Item> = records.iter().map(|record| (id_of(record), item(record))).filter(|(id, _)| !id.is_empty()).collect();
    let locale = locale(language);
    let year_alone = groups.iter().flatten().any(|cite| cite.narrative) && says_year(style);
    let mut driver = BibliographyDriver::new();
    for group in groups {
        let cited = group
            .iter()
            .filter_map(|cite| {
                let mut item = CitationItem::with_entry(items.get(&cite.id)?);
                if !cite.locator.trim().is_empty() { item.locator = Some(SpecificLocator(citationberg::taxonomy::Locator::Page, LocatorPayload::Str(cite.locator.trim()))) }
                Some(item)
            })
            .collect();
        driver.citation(CitationRequest::new(cited, &style.style, locale.clone(), locales(), None));
    }
    for id in uncited {
        let Some(found) = items.get(id) else { continue };
        let mut item = CitationItem::with_entry(found);
        item.hidden = true;
        driver.citation(CitationRequest::new(vec![item], &style.style, locale.clone(), locales(), None));
    }
    let rendered = driver.finish(BibliographyRequest::new(&style.style, locale, locales()));
    let ids: Vec<Vec<&str>> = groups.iter().map(|group| group.iter().filter(|cite| items.contains_key(&cite.id)).map(|cite| cite.id.as_str()).collect()).collect();
    let (mut cited, list) = written(rendered, &ids);
    // The authors a sentence already names are not said again: the citation from its first year on, in the style's
    // own brackets. The processor's own year-only form drops the letter that tells two works of a year apart.
    for (rich, group) in cited.iter_mut().zip(groups).filter(|(_, group)| year_alone && !group.is_empty() && group.iter().all(|cite| cite.narrative)) {
        let years: Vec<String> = group.iter().filter_map(|cite| records.iter().find(|record| id_of(record) == cite.id)).filter_map(year_said).collect();
        let Some(at) = years.iter().filter_map(|year| rich.text.find(year.as_str())).min() else { continue };
        let open = rich.text.chars().next().filter(|c| !c.is_alphanumeric()).map_or(0, char::len_utf8);
        if at > open { *rich = cut(std::mem::take(rich), open..at) }
    }
    (cited, list)
}

/// The year a record is of, as its digits: `{"date-parts": [["2024", 3]]}`, or the year alone.
fn year_said(record: &Record) -> Option<String> {
    let issued = record.get("issued")?;
    let year = issued.get("date-parts").and_then(|parts| parts.get(0)?.get(0)).unwrap_or(issued);
    Some(year.to_string().trim_matches('"').trim().to_string()).filter(|year| year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()))
}

/// A text without a stretch of it, its ranges kept on what stays.
fn cut(rich: Rich, gone: std::ops::Range<usize>) -> Rich {
    let fit = |n: usize| if n >= gone.end { n - gone.len() } else { n.min(gone.start) };
    let mut text = rich.text;
    text.replace_range(gone.clone(), "");
    Rich { text, looks: rich.looks.into_iter().map(|(range, look)| (fit(range.start)..fit(range.end), look)).filter(|(range, _)| !range.is_empty()).collect(), links: rich.links.into_iter().map(|(range, to)| (fit(range.start)..fit(range.end), to)).filter(|(range, _)| !range.is_empty()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(id: &str, authors: &str, year: i32, title: &str, journal: &str) -> Paper {
        Paper { id: id.into(), authors: authors.into(), year: Some(year), title: title.into(), journal: journal.into(), ..Default::default() }
    }

    #[test]
    fn a_record_becomes_a_paper() {
        // There and back: what `record_of` writes, `paper_of` reads.
        let mut sent = paper("p1", "Zhang, Wei; Li, Na", 2021, "Root hairs take up water", "Plant Cell");
        (sent.volume, sent.issue, sent.pages, sent.doi) = ("33".into(), "2".into(), "10-19".into(), "10.1093/plcell/koab001".into());
        let back = paper_of(&record_of(&sent)).unwrap();
        assert_eq!((back.title.as_str(), back.authors.as_str(), back.year, back.journal.as_str(), back.placed(), back.doi.as_str(), back.publication_status.as_str()), ("Root hairs take up water", "Zhang, Wei; Li, Na", Some(2021), "Plant Cell", sent.placed(), "10.1093/plcell/koab001", "published"));
        // As Zotero writes one into a Word file: a year as text, a name in one piece, a Chinese name in two, and what no paper may hold left out.
        let record: Record = serde_json::from_value(serde_json::json!({"id": "x", "type": "article-journal", "title": "<i>Arabidopsis</i> roots", "container-title": ["J. Roots"], "author": [{"family": "张", "given": "三"}, {"literal": "The Root Consortium"}, {"given": "Nobody"}], "issued": {"date-parts": [["2019", 5]]}, "DOI": "not a doi", "PMID": "PMC123", "page": "e12"})).unwrap();
        let taken = paper_of(&record).unwrap();
        assert_eq!((taken.title.as_str(), taken.authors.as_str(), taken.year, taken.journal.as_str(), taken.pages.as_str(), taken.doi.as_str(), taken.pmid.as_str()), ("Arabidopsis roots", "张三; The Root Consortium", Some(2019), "J. Roots", "e12", "", ""));
        let mut normal = taken;
        normal.normalize();
        assert!(normal.validate().is_ok());
        // A year out of range is no year, and a work without a title is no paper.
        assert_eq!(paper_of(&serde_json::from_value(serde_json::json!({"title": "Old", "issued": {"raw": "1066"}})).unwrap()).unwrap().year, None);
        assert_eq!(paper_of(&serde_json::from_value(serde_json::json!({"author": [{"family": "X"}]})).unwrap()), None);
    }

    fn temp(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("wisp-csl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn the_common_styles_are_all_built_in_and_cite_in_the_text() {
        let root = temp("common");
        assert_eq!(common().len(), NUMBERED.len() + BY_AUTHOR.len());
        // Each is in the group that says how it cites, and none is listed twice.
        for one in common_numbered() { assert!(load_from(&one.key, &root, None).unwrap().numbered(), "{}", one.key) }
        for one in common_by_author() { assert!(!load_from(&one.key, &root, None).unwrap().numbered(), "{}", one.key) }
        let mut keys: Vec<String> = common().into_iter().map(|one| one.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), NUMBERED.len() + BY_AUTHOR.len());
        assert!(bundled().len() > 50);
        // A style that cites in footnotes is said to be one.
        assert!(load_from("csl:chicago-notes", &root, None).unwrap_err().to_string().contains("脚注"));
        assert!(load_from("csl:nothing", &root, None).is_err());
        assert!(load_from("file:../x.csl", &root, None).is_err());
    }

    #[test]
    fn the_built_in_styles_that_cite_in_footnotes_are_known_by_name() {
        let found: Vec<&str> = Builtin::all().into_iter().filter(|style| matches!(style.style(), Some(citationberg::Style::Independent(read)) if read.settings.class == StyleClass::Note)).map(Builtin::name).collect();
        assert_eq!(found, IN_NOTES);
        assert!(in_notes("csl:chicago-notes") && !in_notes("csl:nature") && !in_notes("file:chicago-notes.csl") && !in_notes("numeric"));
    }

    #[test]
    fn a_built_in_style_has_a_short_name_that_tells_it_apart() {
        // Every short name is of a style there is, and no two styles share one.
        let mut said: Vec<&str> = SHORT.iter().map(|(name, said)| { assert!(Builtin::by_name(name).is_some(), "{name}"); *said }).collect();
        said.sort();
        said.dedup();
        assert_eq!(said.len(), SHORT.len());
        // Every style offered first has one: its own title may be long (GB/T) or a manual's (APA).
        for one in common() { assert!(short_name(&one.key).is_some(), "{}", one.key) }
        // By whichever name the key says it, and the three GB/T styles apart.
        assert_eq!(short_name("csl:apa"), short_name("csl:american-psychological-association"));
        assert_eq!((short_name("csl:gb-7714-2015-numeric"), short_name("csl:gb-7714-2015-author-date"), short_name("csl:gb-7714-2005-numeric")), (Some("GB/T 7714-2015 顺序编码"), Some("GB/T 7714-2015 著者-出版年"), Some("GB/T 7714-2005 顺序编码")));
        assert_eq!((short_name("csl:alphanumeric"), short_name("file:nature.csl"), short_name("numeric")), (None, None, None));
    }

    #[test]
    fn the_styles_shipped_beside_hayagriva_s_cite_like_the_built_in_ones() {
        let root = temp("shipped");
        let (a, b) = (paper("a", "Zhang, Wei; Li, Qiang; Wang, Fang", 2024, "Single-cell atlas of roots", "Nature Plants"), paper("b", "Smith, John", 2019, "Second", "Cell"));
        for (name, _) in SHIPPED {
            let key = format!("csl:{name}");
            let style = load_from(&key, &root, None).unwrap();
            assert!(style.numbered() && !style.name.is_empty(), "{name}");
            let (cited, list) = render(&style, &[vec![&a], vec![&a, &b]], "en");
            assert!(cited[0].text.contains('1') && cited[1].text.contains('2'), "{name}: {:?}", cited);
            assert!(list.len() == 2 && list[0].1.text.contains("Zhang") && list[0].1.text.contains("Single-cell atlas of roots"), "{name}: {}", list[0].1.text);
            // Listed among the built-in ones under its own title, written out as its file, and found by the id a copy of it in Zotero's folder carries.
            assert!(bundled().iter().any(|one| one.key == key && one.name == style.name));
            assert_eq!(xml_of(&key, &root).unwrap(), SHIPPED.iter().find(|(shipped, _)| *shipped == name).unwrap().1);
            assert!(Builtin::by_id(&format!("http://www.zotero.org/styles/{name}")).is_some(), "{name}");
        }
        assert_eq!(load_from("csl:the-new-england-journal-of-medicine", &root, None).unwrap().name, "The New England Journal of Medicine");
    }

    #[test]
    fn a_style_the_person_added_is_removed_and_can_be_put_back() {
        let root = temp("remove");
        let xml = xml_of("csl:nature", &root).unwrap();
        let saved = save(&root, "mine", &xml).unwrap();
        assert_eq!((saved.key.as_str(), file(&root, &saved.key)), ("file:mine.csl", Some(root.join(FOLDER).join("mine.csl"))));
        let (name, said) = remove(&root, &saved.key).unwrap();
        assert_eq!((name.as_str(), said == xml), ("mine.csl", true));
        assert!(added_from(&root, None).is_empty() && std::fs::read_dir(root.join(FOLDER).join("removed")).unwrap().count() == 1);
        assert_eq!(save(&root, &name, &said).unwrap().key, "file:mine.csl");
        // Only what the person added: a built-in style and one of Zotero's stay.
        assert!(remove(&root, "csl:nature").is_err() && remove(&root, "zotero:x.csl").is_err() && remove(&root, "file:../x.csl").is_err());
        assert_eq!(file(&root, "csl:nature"), None);
    }

    #[test]
    fn a_numbered_style_numbers_raises_and_links_each_paper() {
        let root = temp("nature");
        let style = load_from("csl:nature", &root, None).unwrap();
        let (a, b, c) = (paper("a", "Zhang, Wei; Li, Qiang; Wang, Fang", 2024, "Single-cell atlas of roots", "Nature Plants"), paper("b", "Smith, John", 2019, "Second", "Cell"), paper("c", "Li, Qiang", 2020, "Third", "Science"));
        let (cited, list) = render(&style, &[vec![&a], vec![&a, &b, &c], vec![&c, &a]], "en");
        assert_eq!(cited.iter().map(|rich| rich.text.as_str()).collect::<Vec<_>>(), ["1", "1–3", "1,3"]);
        assert_eq!(cited[0].links, [(0..1, "a".to_string())]);
        // A range names its two ends; a list every number, whatever order the papers were cited in.
        assert_eq!(cited[1].links, [(0..1, "a".to_string()), (4..5, "c".to_string())]);
        assert_eq!(cited[2].links, [(0..1, "a".to_string()), (2..3, "c".to_string())]);
        assert!(cited[1].looks.iter().all(|(_, look)| look.sup) && cited[1].looks.iter().map(|(range, _)| range.len()).sum::<usize>() == cited[1].text.len());
        assert_eq!(list.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert!(list[0].1.text.starts_with("1. Zhang, W., Li, Q. & Wang, F. Single-cell atlas of roots."), "{}", list[0].1.text);
        // The journal is in italics.
        let (range, look) = list[0].1.looks.iter().find(|(_, look)| look.italic).unwrap();
        assert_eq!((&list[0].1.text[range.clone()], look.bold), ("Nature Plants", false));
    }

    #[test]
    fn an_entry_says_where_in_the_journal_the_paper_is() {
        let root = temp("placed");
        let mut numpy = paper("a", "Harris, Charles R.; Millman, K. Jarrod", 2020, "Array programming with NumPy", "Nature");
        (numpy.volume, numpy.issue, numpy.pages) = ("585".into(), "7825".into(), "357-362".into());
        let entry = |style: &str, paper: &Paper| render(&load_from(style, &root, None).unwrap(), &[vec![paper]], "en").1[0].1.clone();
        let apa = entry("csl:apa", &numpy);
        assert_eq!(apa.text, "Harris, C. R., & Millman, K. J. (2020). Array programming with NumPy. Nature, 585(7825), 357–362.");
        // APA: the journal and the volume in italics, the issue not.
        let italic: Vec<&str> = apa.looks.iter().filter(|(_, look)| look.italic).map(|(range, _)| &apa.text[range.clone()]).collect();
        assert_eq!(italic, ["Nature", "585"]);
        // Nature: the volume in bold (with its comma, as the style has it), no issue.
        let nature = entry("csl:nature", &numpy);
        assert_eq!(nature.text, "1. Harris, C. R. & Millman, K. J. Array programming with NumPy. Nature 585, 357–362 (2020).");
        let bold: Vec<&str> = nature.looks.iter().filter(|(_, look)| look.bold).map(|(range, _)| &nature.text[range.clone()]).collect();
        assert_eq!(bold, ["585,"]);
        // An article number and a supplement's issue are written as they are; what a record lacks is left out.
        (numpy.volume, numpy.issue, numpy.pages) = ("12".into(), "S1".into(), "e1234".into());
        assert_eq!(entry("csl:apa", &numpy).text, "Harris, C. R., & Millman, K. J. (2020). Array programming with NumPy. Nature, 12(S1), e1234.");
        (numpy.issue, numpy.pages) = (String::new(), String::new());
        assert_eq!(entry("csl:apa", &numpy).text, "Harris, C. R., & Millman, K. J. (2020). Array programming with NumPy. Nature, 12.");
    }

    #[test]
    fn an_author_date_style_tells_alike_papers_apart_and_sorts_its_list() {
        let root = temp("apa");
        let style = load_from("csl:apa", &root, None).unwrap();
        let (a, b, c) = (paper("a", "Zhang, Wei; Li, Qiang; Wang, Fang", 2024, "Single-cell atlas of roots", "Nature Plants"), paper("b", "Zhang, Wei; Li, Qiang; Wang, Fang", 2024, "Another atlas", "Cell"), paper("c", "Smith, John", 2019, "Third", "Science"));
        let (cited, list) = render(&style, &[vec![&a], vec![&b, &c]], "en");
        assert_eq!(cited[0].text, "(Zhang et al., 2024a)");
        assert_eq!(cited[1].text, "(Smith, 2019; Zhang et al., 2024b)");
        assert_eq!(cited[1].links.iter().map(|(range, to)| (&cited[1].text[range.clone()], to.as_str())).collect::<Vec<_>>(), [("Smith, 2019", "c"), ("Zhang et al., 2024b", "b")]);
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].0.as_str(), list[0].1.text.as_str()), ("c", "Smith, J. (2019). Third. Science."));
        // The letter a citation carries is the one its entry has.
        assert!(list.iter().any(|(id, entry)| id == "b" && entry.text.contains("(2024b). Another atlas.")), "{list:?}");
    }

    const OWN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">
  <info><title>Journal of Roots &amp; Shoots</title><id>http://www.zotero.org/styles/journal-of-roots</id><updated>2024-01-01T00:00:00+00:00</updated></info>
  <citation collapse="citation-number"><sort><key variable="citation-number"/></sort><layout prefix="{" suffix="}" delimiter=";"><text variable="citation-number"/></layout></citation>
  <bibliography><layout suffix="."><text variable="citation-number" suffix=") "/><names variable="author"><name initialize-with="" name-as-sort-order="all" sort-separator=" "/></names><text variable="title" prefix=": " font-weight="bold"/></layout></bibliography>
</style>"#;

    const ALIAS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" version="1.0" default-locale="en-US">
  <info><title>Plant Letters</title><id>http://www.zotero.org/styles/plant-letters</id><link href="http://www.zotero.org/styles/PARENT" rel="independent-parent"/><updated>2024-01-01T00:00:00+00:00</updated></info>
</style>"#;

    #[test]
    fn a_file_of_the_persons_or_of_zotero_is_a_style_and_an_alias_is_its_parent() {
        let (root, zotero, elsewhere) = (temp("own"), temp("zotero"), temp("elsewhere"));
        std::fs::write(elsewhere.join("journal-of-roots.csl"), OWN).unwrap();
        let listed = add(&root, &elsewhere.join("journal-of-roots.csl")).unwrap();
        assert_eq!(listed, Listed { key: "file:journal-of-roots.csl".into(), name: "Journal of Roots & Shoots".into() });
        // Zotero's: the one built in and the one added are not listed twice.
        std::fs::write(zotero.join("apa.csl"), OWN.replace("journal-of-roots", "apa")).unwrap();
        std::fs::write(zotero.join("journal-of-roots.csl"), OWN).unwrap();
        std::fs::write(zotero.join("plant-letters.csl"), ALIAS.replace("PARENT", "journal-of-roots")).unwrap();
        std::fs::write(zotero.join("lost.csl"), ALIAS.replace("PARENT", "nowhere").replace("plant-letters", "lost")).unwrap();
        std::fs::write(zotero.join("of-nature.csl"), ALIAS.replace("PARENT", "nature").replace("plant-letters", "of-nature")).unwrap();
        let keys: Vec<String> = added_from(&root, Some(&zotero)).into_iter().map(|style| style.key).collect();
        assert_eq!(keys, ["file:journal-of-roots.csl", "zotero:lost.csl", "zotero:of-nature.csl", "zotero:plant-letters.csl"]);

        let a = paper("a", "Zhang, Wei; Li, Qiang", 2024, "Atlas", "Cell");
        let b = paper("b", "Smith, John", 2019, "Second", "Cell");
        for key in ["file:journal-of-roots.csl", "zotero:plant-letters.csl"] {
            let style = load_from(key, &root, Some(&zotero)).unwrap();
            let (cited, list) = render(&style, &[vec![&a, &b]], "en");
            assert_eq!(cited[0].text, "{1;2}", "{key}");
            assert_eq!(list[0].1.text, "1) Zhang W, Li Q: Atlas.");
            assert_eq!(list[0].1.looks.iter().map(|(range, look)| (&list[0].1.text[range.clone()], look.bold)).collect::<Vec<_>>(), [(": Atlas", true)]);
        }
        assert_eq!(load_from("zotero:of-nature.csl", &root, Some(&zotero)).unwrap().name, "Plant Letters");
        let lost = load_from("zotero:lost.csl", &root, Some(&zotero)).unwrap_err().to_string();
        assert!(lost.contains("nowhere.csl"), "{lost}");
        assert!(add(&root, &elsewhere.join("missing.csl")).is_err());
        std::fs::write(elsewhere.join("not.csl"), "<html/>").unwrap();
        assert!(add(&root, &elsewhere.join("not.csl")).is_err());
    }

    #[test]
    fn every_built_in_style_names_the_authors_in_its_list() {
        let root = temp("authors");
        let mut without_a_list = vec![];
        for one in bundled().into_iter().filter(|one| !in_notes(&one.key)) {
            let style = load_from(&one.key, &root, None).unwrap();
            // A style may say how to cite and nothing of a list, and its list is then empty: Typst's own `alphanumeric`, which only makes the labels (`[Zha+24]`), is the one built in.
            if style.style.bibliography.is_none() { without_a_list.push(one.key); continue }
            let sample = crate::cite::sample(&crate::cite::Style::Csl(std::sync::Arc::new(style)));
            // Zhang is an author of the first work alone, so the entry that says the name is that work's, wherever a list by author puts it and in whatever case the style sets a name.
            let entries: Vec<&str> = sample.list.iter().map(|entry| entry.text.as_str()).collect();
            assert!(entries.len() == 3 && entries.iter().any(|entry| entry.to_lowercase().contains("zhang")), "{}: {entries:?}", one.key);
            // And no year has a space left after it, before the mark that follows, where the date's form had a month to say.
            let left_over = |entry: &&str| ["2024", "2019", "2021"].iter().any(|year| [';', ',', '.', ':', ')'].iter().any(|mark| entry.contains(&format!("{year} {mark}"))));
            assert!(!entries.iter().any(left_over), "{}: {entries:?}", one.key);
        }
        assert_eq!(without_a_list, ["csl:alphanumeric"]);
    }

    /// A style as a person might add one, written the two ways the processor reads wrong: the authors without a `<name>`, the date with a delimiter.
    const BARE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0" initialize-with="" name-as-sort-order="all" sort-separator=" " name-delimiter=", ">
  <info><title>Bare Names</title><id>http://www.zotero.org/styles/bare-names</id><updated>2024-01-01T00:00:00+00:00</updated></info>
  <macro name="author"><names variable="author"><label form="long" prefix=", "/><substitute><names variable="editor"/></substitute></names></macro>
  <citation><layout prefix="(" suffix=")" delimiter=","><text variable="citation-number"/></layout></citation>
  <bibliography><layout suffix="."><text variable="citation-number" suffix=". "/><group delimiter=". "><text macro="author"/><text variable="title"/><group delimiter=";"><date variable="issued" delimiter=" ">PARTS</date><text variable="volume"/></group></group></layout></bibliography>
</style>"#;

    #[test]
    fn names_without_a_name_are_written_and_a_date_leaves_no_delimiter_over() {
        let root = temp("mended");
        // Vancouver as NLM has it, the style both were found in: the authors (six of seven, as it says), and no space before the semicolon after a year alone.
        let nlm = crate::cite::sample(&crate::cite::Style::Csl(std::sync::Arc::new(load_from("csl:nlm-citation-sequence", &root, None).unwrap())));
        assert_eq!(nlm.list[0].text, "1. Zhang W, Li Q, Wang F, Smith J, Müller A, Tanaka H, et al. A single-cell atlas of root development. Nature Plants. 2024;10(3):357–62.");
        assert_eq!(nlm.list[2].text, "3. 王芳, 李强, 张伟. 水稻根系发育的单细胞转录组研究. 中国科学：生命科学. 2021;51(8):912–25.");
        assert_eq!(crate::cite::sample(&crate::cite::Style::Csl(std::sync::Arc::new(load_from("csl:vancouver-superscript", &root, None).unwrap()))).list[1].text, "2. Smith JA, Jones M. Auxin transport shapes the root tip. The Plant Cell. 2019;31(4):1021–33.");

        // A file of the person's is read the same way. A paper has a year alone; a record may say the month and the day.
        let mut atlas = paper("a", "Zhang, Wei; Li, Qiang", 2024, "Atlas", "Cell");
        atlas.volume = "12".into();
        let records: Vec<Record> = serde_json::from_str(r#"[{"id": "month", "type": "article-journal", "title": "Month", "volume": "3", "author": [{"family": "Smith", "given": "John"}], "issued": {"date-parts": [[2017, 3]]}},
            {"id": "day", "type": "book", "title": "Day", "editor": [{"family": "Jones", "given": "Mary"}, {"family": "Garcia", "given": "Maria"}], "issued": {"date-parts": [[2019, 6, 5]]}}]"#).unwrap();
        let cites = [vec![Cite { id: "month".into(), ..Default::default() }, Cite { id: "day".into(), ..Default::default() }]];
        let entries = |parts: &str| {
            let key = save(&root, "bare", &BARE.replace("PARTS", parts)).unwrap().key;
            let style = load_from(&key, &root, None).unwrap();
            let mut said: Vec<String> = render(&style, &[vec![&atlas]], "en").1.into_iter().map(|(_, entry)| entry.text).collect();
            said.extend(render_records(&style, &cites, &[], &records, "en").1.into_iter().map(|(_, entry)| entry.text));
            said
        };
        // The label stands after the names it is of, and editors who stand in for the authors are written as the authors would be.
        assert_eq!(entries(r#"<date-part name="year"/><date-part name="month" form="numeric"/><date-part name="day"/>"#), ["1. Zhang W, Li Q. Atlas. 2024;12.", "1. Smith J. Month. 2017 3;3.", "2. Jones M, Garcia M, editors. Day. 2019 6 5."]);
        // The year last: what comes before it brings the delimiter after itself.
        assert_eq!(entries(r#"<date-part name="day"/><date-part name="month" form="numeric"/><date-part name="year"/>"#), ["1. Zhang W, Li Q. Atlas. 2024;12.", "1. Smith J. Month. 3 2017;3.", "2. Jones M, Garcia M, editors. Day. 5 6 2019."]);
    }

    #[test]
    fn works_that_are_no_papers_are_cited_from_their_records() {
        let root = temp("records");
        let zotero: Record = serde_json::from_str(r#"{"id": "z1", "type": "article-journal", "title": "Single-cell atlas of roots", "container-title": "Nature Plants", "volume": "10", "issue": "3", "page": "45-67", "DOI": "10.1/x", "author": [{"family": "Zhang", "given": "Wei"}, {"family": "Li", "given": "Qiang"}, {"family": "Wang", "given": "Fang"}], "issued": {"date-parts": [["2024", 3, 1]]}, "accessed": {"season": []}, "keywords": ["a", "b"]}"#).unwrap();
        let mut held = record_of(&paper("p1", "Smith, John", 2019, "Second", "Cell"));
        held.insert("issued".into(), serde_json::json!(2019));
        let cite = |id: &str| Cite { id: id.into(), ..Default::default() };
        let apa = load_from("csl:apa", &root, None).unwrap();
        let (cited, list) = render_records(&apa, &[vec![cite("z1"), cite("p1")], vec![Cite { locator: "12".into(), ..cite("z1") }], vec![Cite { narrative: true, ..cite("p1") }], vec![cite("gone")]], &[], &[zotero.clone(), held.clone()], "en");
        assert_eq!(cited.iter().map(|rich| rich.text.as_str()).collect::<Vec<_>>(), ["(Smith, 2019; Zhang et al., 2024)", "(Zhang et al., 2024, p. 12)", "(2019)", ""]);
        assert_eq!(cited[2].links, [(1..5, "p1".to_string())]);
        assert_eq!(cited[0].links.iter().map(|(_, to)| to.as_str()).collect::<Vec<_>>(), ["p1", "z1"]);
        // What the record holds beyond a paper's is in the entry: volume, issue, pages.
        let entry = &list.iter().find(|(id, _)| id == "z1").unwrap().1.text;
        assert!(entry.contains("10(3), 45–67"), "{entry}");
        // A numbered style has no year to say alone, and a work nothing cites is listed after the others.
        let nature = load_from("csl:nature", &root, None).unwrap();
        let (cited, list) = render_records(&nature, &[vec![Cite { narrative: true, ..cite("p1") }]], &["z1".to_string()], &[zotero, held], "en");
        assert_eq!(cited[0].text, "1");
        assert_eq!(list.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["p1", "z1"]);
        assert!(list[1].1.text.starts_with("2. Zhang, W., Li, Q. & Wang, F."), "{}", list[1].1.text);
    }
}
