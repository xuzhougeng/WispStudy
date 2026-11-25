//! 技能: instructions the person keeps for the assistant, each a folder
//! with a `SKILL.md` (a `name` and `description` between `---` lines, then
//! the instructions), the way Claude's skills are written. The assistant is
//! told the names and descriptions and reads one with `use_skill` when a
//! request calls for it; the person calls one by name, `/name` at the start
//! of a message (`ai::read_input`), and then its instructions go with the
//! request. A skill is words only: nothing in its folder is run.

use crate::connector::Tool;
use serde_json::json;
use std::path::{Path, PathBuf};

/// The tool a skill is read with.
pub const TOOL: &str = "use_skill";
const FILE: &str = "SKILL.md";

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The instructions: the file without its front matter.
    pub body: String,
    pub dir: PathBuf,
}

/// The folder the skills are kept in: beside the settings file.
pub fn folder(settings_file: &Path) -> PathBuf {
    settings_file.parent().unwrap_or(Path::new(".")).join("skills")
}

/// A `SKILL.md` as (name, description, instructions); a field the front matter lacks is empty.
pub fn parse(text: &str) -> (String, String, String) {
    let text = text.trim_start_matches('\u{feff}');
    let Some((front, body)) = text.strip_prefix("---").and_then(|rest| rest.split_once("\n---")) else { return (String::new(), String::new(), text.trim().to_string()) };
    let (mut name, mut description, mut field) = (String::new(), String::new(), "");
    for line in front.lines() {
        let said = |value: &str| value.trim().trim_matches(['"', '\'']).to_string();
        // An indented line goes on with the field above it (`description: >`).
        if line.starts_with([' ', '\t']) {
            let more = said(line);
            if field == "description" && !more.is_empty() {
                if !description.is_empty() { description.push(' ') }
                description.push_str(&more);
            }
        } else if let Some(value) = line.strip_prefix("name:") {
            (name, field) = (said(value), "name");
        } else if let Some(value) = line.strip_prefix("description:") {
            (description, field) = (said(value.trim().trim_start_matches(['>', '|', '-'])), "description");
        } else {
            field = "";
        }
    }
    // The rest of the closing line of dashes is not part of the instructions.
    (name, description, body.split_once('\n').map_or("", |(_, rest)| rest).trim().to_string())
}

/// The skills in `dir`, by name; a folder without a `SKILL.md` is none.
pub fn list(dir: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| {
        let dir = entry.path();
        let (name, description, body) = parse(&std::fs::read_to_string(dir.join(FILE)).ok()?);
        let name = if name.is_empty() { dir.file_name()?.to_string_lossy().into_owned() } else { name };
        Some(Skill { name, description, body, dir })
    }).collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Copy a skill into `dir`: a folder holding a `SKILL.md`, or one Markdown
/// file, which becomes the `SKILL.md` of a folder named after it. A skill
/// already there under that folder name is left alone.
pub fn import(dir: &Path, from: &Path) -> Result<String, String> {
    let file = if from.is_dir() { from.join(FILE) } else { from.to_path_buf() };
    if !file.is_file() { return Err(format!("{} 里没有 {FILE}。", from.display())) }
    let name = from.file_stem().map(|stem| stem.to_string_lossy().into_owned()).filter(|stem| !stem.is_empty()).ok_or("这个路径没有名字。")?;
    let to = dir.join(&name);
    if to.exists() { return Err(format!("已经有名为 {name} 的技能文件夹。")) }
    let failed = |err: std::io::Error| format!("无法保存技能：{err}");
    if from.is_dir() { copy_folder(from, &to).map_err(failed)? } else {
        std::fs::create_dir_all(&to).map_err(failed)?;
        std::fs::copy(&file, to.join(FILE)).map_err(failed)?;
    }
    Ok(name)
}

fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() { copy_folder(&entry.path(), &target)? } else { std::fs::copy(entry.path(), target).map(|_| ())? }
    }
    Ok(())
}

/// A file or a folder of a skill, as its page lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Where it is under the skill's folder, the names apart by `/`.
    pub path: String,
    pub folder: bool,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// How deep it lies: 0 in the skill's folder itself.
    pub fn depth(&self) -> usize {
        self.path.matches('/').count()
    }
}

/// The most entries a skill's page lists, and how deep it looks: a folder
/// that holds more (a checkout, a link that leads back into itself) is not read to its end.
const MOST: usize = 400;
const DEEPEST: usize = 8;

/// What a skill's folder holds, each folder followed by what is in it:
/// `SKILL.md` first, then the folders, then the other files, by name.
/// Hidden files are left out.
pub fn files(dir: &Path) -> Vec<Entry> {
    fn walk(dir: &Path, under: &str, depth: usize, found: &mut Vec<Entry>) {
        let mut here: Vec<(bool, String)> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A link is what it leads to.
            let kind = std::fs::metadata(entry.path()).ok()?;
            (!name.starts_with('.')).then_some((kind.is_dir(), name))
        }).collect();
        let rank = |(folder, name): &(bool, String)| (!(depth == 0 && name == FILE), !*folder, name.to_lowercase());
        here.sort_by_key(rank);
        for (folder, name) in here {
            if found.len() >= MOST { return }
            let path = format!("{under}{name}");
            found.push(Entry { path: path.clone(), folder });
            if folder && depth < DEEPEST { walk(&dir.join(&name), &format!("{path}/"), depth + 1, found) }
        }
    }
    let mut found = vec![];
    walk(dir, "", 0, &mut found);
    found
}

/// What a skill's page shows of one of its files.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// Its text; `cut` when that is only its beginning.
    Text { text: String, cut: bool },
    /// Not text: how many bytes it holds.
    Binary(u64),
    Failed(String),
}

/// As much of a file as the page shows.
const SHOWN_BYTES: usize = 200_000;

/// Whether a file is Markdown by its name: drawn as a page, with its source one click away.
pub fn markdown(path: &str) -> bool {
    let name = path.to_lowercase();
    name.ends_with(".md") || name.ends_with(".markdown")
}

/// The file at `path` (as `files` names it) of the skill in `dir`, read for its page.
pub fn show(dir: &Path, path: &str) -> Shown {
    use std::io::Read;
    if path.split('/').any(|name| name.is_empty() || name == "..") { return Shown::Failed(format!("{path} 不在这项技能的文件夹里。")) }
    let failed = |err: std::io::Error| Shown::Failed(format!("无法读取 {path}：{err}"));
    let file = match std::fs::File::open(dir.join(path)) {
        Ok(file) => file,
        Err(err) => return failed(err),
    };
    let size = file.metadata().map_or(0, |known| known.len());
    let mut bytes = vec![];
    if let Err(err) = file.take(SHOWN_BYTES as u64 + 1).read_to_end(&mut bytes) { return failed(err) }
    let cut = bytes.len() > SHOWN_BYTES;
    bytes.truncate(SHOWN_BYTES);
    if bytes.contains(&0) { return Shown::Binary(size) }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // Cut in the middle of a character: what stands before it is the text.
        Err(err) if cut && err.utf8_error().error_len().is_none() => {
            let whole = err.utf8_error().valid_up_to();
            String::from_utf8_lossy(&err.as_bytes()[..whole]).into_owned()
        }
        Err(_) => return Shown::Binary(size),
    };
    Shown::Text { text: text.trim_start_matches('\u{feff}').replace("\r\n", "\n"), cut }
}

/// Where the skills taken out of `folder` are kept.
const REMOVED: &str = "removed";

/// Take the skill in `dir` out of `folder`: its folder moves to `removed/`
/// there, under a name nothing has yet. Nothing is deleted; where it went is returned.
pub fn remove(folder: &Path, dir: &Path) -> Result<PathBuf, String> {
    let name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).ok_or("这个路径没有名字。")?;
    let kept = folder.join(REMOVED);
    let failed = |err: std::io::Error| format!("无法移除技能：{err}");
    std::fs::create_dir_all(&kept).map_err(failed)?;
    let to = (1..).map(|n| if n == 1 { kept.join(&name) } else { kept.join(format!("{name}-{n}")) }).find(|to| !to.exists()).unwrap_or_else(|| kept.join(&name));
    std::fs::rename(dir, &to).map_err(failed)?;
    Ok(to)
}

/// Put a skill taken out back among those of `folder`, under the folder name it had.
pub fn restore(folder: &Path, removed: &Path, name: &str) -> Result<(), String> {
    let to = folder.join(name);
    if to.exists() { return Err(format!("已经有名为 {name} 的技能文件夹。")) }
    std::fs::rename(removed, to).map_err(|err| format!("无法恢复技能：{err}"))
}

/// The tool that reads a skill, its description listing those on offer; none without skills.
pub fn tool(skills: &[Skill]) -> Option<Tool> {
    if skills.is_empty() { return None }
    let listed: String = skills.iter().map(|skill| format!("\n- {}：{}", skill.name, skill.description)).collect();
    Some(Tool {
        name: TOOL.into(),
        tool: TOOL.into(),
        description: format!("读取用户保存的一项技能（一份做某类事情的说明）。当前请求符合下面某项技能的描述时，先用它读出说明，再照着做。可用的技能：{listed}"),
        input_schema: json!({"type": "object", "properties": {"name": {"type": "string", "enum": skills.iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>()}}, "required": ["name"]}),
        read_only: true,
    })
}

/// What `use_skill` answers for `name`.
pub fn read(skills: &[Skill], name: &str) -> Result<String, String> {
    skills.iter().find(|skill| skill.name == name).map(|skill| skill.body.clone()).ok_or_else(|| format!("没有名为 {name} 的技能。"))
}

/// Whether `/name` can call the skill: a name typed as one word.
pub fn called(skill: &Skill) -> bool {
    !skill.name.is_empty() && !skill.name.contains(|c: char| c.is_whitespace() || c == '/')
}

/// The request a skill called by name stands for: its instructions, then
/// what the person wrote after the name.
pub fn request(skill: &Skill, argument: &str) -> Result<String, String> {
    let (name, body) = (&skill.name, skill.body.trim());
    if body.is_empty() { return Err(format!("技能 {name} 的 {FILE} 里还没有写说明。")) }
    let asked = if argument.is_empty() { String::new() } else { format!("\n\n要做的事：{argument}") };
    Ok(format!("用技能「{name}」来做。它的说明就在下面，不必再用 {TOOL} 读：\n\n<skill name=\"{name}\">\n{body}\n</skill>{asked}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_file_is_read_with_and_without_front_matter() {
        assert_eq!(parse("---\nname: journal-club\ndescription: \"Prepare a journal club\"\nlicense: MIT\n---\n\n# Steps\n1. Read."), ("journal-club".into(), "Prepare a journal club".into(), "# Steps\n1. Read.".into()));
        assert_eq!(parse("---\nname: a\ndescription: >\n  first line\n  second line\nallowed-tools:\n  - x\n---\nBody"), ("a".into(), "first line second line".into(), "Body".into()));
        assert_eq!(parse("Just do it.\n"), (String::new(), String::new(), "Just do it.".into()));
    }

    #[test]
    fn skills_are_imported_listed_and_read() {
        let root = std::env::temp_dir().join(format!("wisp-skills-{}", crate::model::new_id("T")));
        let (dir, from) = (root.join("skills"), root.join("from"));
        std::fs::create_dir_all(from.join("review-notes/refs")).unwrap();
        std::fs::write(from.join("review-notes/SKILL.md"), "---\nname: review-notes\ndescription: 整理读书笔记\n---\n按题目分组。").unwrap();
        std::fs::write(from.join("review-notes/refs/a.md"), "x").unwrap();
        std::fs::write(from.join("plain.md"), "只说结论。").unwrap();
        assert_eq!(import(&dir, &from.join("review-notes")), Ok("review-notes".into()));
        assert_eq!(import(&dir, &from.join("plain.md")), Ok("plain".into()));
        assert!(import(&dir, &from.join("plain.md")).is_err() && import(&dir, &from).is_err());
        assert!(dir.join("review-notes/refs/a.md").is_file());
        let skills = list(&dir);
        assert_eq!(skills.iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>(), ["plain", "review-notes"]);
        let tool = tool(&skills).unwrap();
        assert!(tool.description.contains("- review-notes：整理读书笔记") && tool.input_schema["properties"]["name"]["enum"][0] == "plain");
        assert_eq!(read(&skills, "review-notes"), Ok("按题目分组。".into()));
        assert!(read(&skills, "none").is_err() && super::tool(&[]).is_none() && list(&root.join("nowhere")).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_skills_folder_is_listed_shown_and_taken_out() {
        let root = std::env::temp_dir().join(format!("wisp-skill-files-{}", crate::model::new_id("T")));
        let dir = root.join("skills/deep");
        std::fs::create_dir_all(dir.join("references/more")).unwrap();
        for (path, text) in [("SKILL.md", "---\nname: deep\n---\n做法。"), ("references/writer.md", "\u{feff}# 写\r\n第二行"), ("references/more/z.txt", "z"), ("a-script.py", "print(1)"), (".DS_Store", "x")] {
            std::fs::write(dir.join(path), text).unwrap();
        }
        std::fs::write(dir.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1, 2]).unwrap();
        let listed = files(&dir);
        assert_eq!(listed.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(), ["SKILL.md", "references", "references/more", "references/more/z.txt", "references/writer.md", "a-script.py", "logo.png"]);
        assert_eq!(listed.iter().map(|entry| (entry.name(), entry.depth(), entry.folder)).nth(3), Some(("z.txt", 2, false)));
        assert!(listed[1].folder && !listed[0].folder);
        assert_eq!(show(&dir, "references/writer.md"), Shown::Text { text: "# 写\n第二行".into(), cut: false });
        assert_eq!(show(&dir, "logo.png"), Shown::Binary(7));
        assert!(matches!(show(&dir, "../deep/SKILL.md"), Shown::Failed(_)) && matches!(show(&dir, "none.md"), Shown::Failed(_)));
        // A long file is shown from its start, and never cut inside a character.
        std::fs::write(dir.join("long.md"), "字".repeat(SHOWN_BYTES / 3 + 10)).unwrap();
        let Shown::Text { text, cut } = show(&dir, "long.md") else { panic!("a long text is still text") };
        assert!(cut && text.chars().all(|c| c == '字') && text.len() == SHOWN_BYTES / 3 * 3);
        assert!(markdown("references/Writer.MD") && !markdown("a-script.py"));
        // Taken out, it is no skill any more, and nothing is lost; put back, it is one again.
        let folder = root.join("skills");
        let kept = remove(&folder, &dir).unwrap();
        assert!(list(&folder).is_empty() && kept.join("references/writer.md").is_file() && kept == folder.join("removed/deep"));
        assert_eq!(restore(&folder, &kept, "deep"), Ok(()));
        assert_eq!(list(&folder).len(), 1);
        std::fs::create_dir_all(folder.join("removed/deep")).unwrap();
        assert_eq!(remove(&folder, &dir), Ok(folder.join("removed/deep-2")));
        assert!(restore(&folder, &folder.join("removed/deep"), "removed").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
