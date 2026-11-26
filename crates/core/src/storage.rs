//! 存储空间: what the data folder takes on the disk, library by library, and
//! what in each library takes it (its PDFs, its database, its pictures …).
//!
//! Sizes are the files' lengths, read without following links. Nothing here
//! writes: measuring is safe while the app or `wisp` uses the libraries.

use crate::libraries::{self, LibraryInfo};
use crate::store::DB_FILE;
use std::path::{Path, PathBuf};

/// What a part of a library's folder holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// `papers/`: the PDFs and other files of the papers, those in the trash among them.
    Files,
    /// The SQLite file and its journal: records, notes, extracted text, conversations.
    Database,
    /// `figures/`: 图片库.
    Figures,
    /// `manuscripts/`: 写作.
    Manuscripts,
    /// `removed/`: attachments taken off a paper and the records of undone imports.
    Removed,
    /// Everything else in the folder.
    Other,
}

impl Part {
    pub const ALL: [Part; 6] = [Part::Files, Part::Database, Part::Figures, Part::Manuscripts, Part::Removed, Part::Other];

    /// The name shown for it, in Chinese; the window translates it where it is drawn.
    pub fn label(self) -> &'static str {
        match self {
            Part::Files => "文献文件",
            Part::Database => "数据库",
            Part::Figures => "图片库",
            Part::Manuscripts => "稿件",
            Part::Removed => "已移除的附件",
            Part::Other => "其他",
        }
    }

    /// Which part an entry directly under a library's folder belongs to.
    fn of(name: &str) -> Part {
        match name {
            "papers" => Part::Files,
            crate::figure::FOLDER => Part::Figures,
            crate::manuscript::FOLDER => Part::Manuscripts,
            "removed" => Part::Removed,
            // The database, its `-wal` and `-shm`.
            _ if name.starts_with(DB_FILE) => Part::Database,
            _ => Part::Other,
        }
    }
}

/// One library and what it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct LibrarySpace {
    pub id: String,
    pub name: String,
    /// Its colour key (`libraries::COLORS`).
    pub color: String,
    /// A conversation from the home page not made a library yet.
    pub draft: bool,
    pub dir: PathBuf,
    pub paper_count: usize,
    /// The bytes of each part, in the order of `Part::ALL`.
    pub parts: [u64; 6],
}

impl LibrarySpace {
    pub fn total(&self) -> u64 {
        self.parts.iter().sum()
    }

    pub fn part(&self, part: Part) -> u64 {
        self.parts[Part::ALL.iter().position(|p| *p == part).unwrap_or(0)]
    }
}

/// A removed library (or home-page conversation) in `removed/` and what it takes on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct RemovedLibrarySpace {
    /// Its folder name inside `removed/`, used as a stable identifier.
    pub folder: String,
    pub id: String,
    pub name: String,
    /// Its colour key (`libraries::COLORS`).
    pub color: String,
    /// A conversation from the home page not made a library yet.
    pub draft: bool,
    pub dir: PathBuf,
    pub paper_count: usize,
    /// When it was removed (milliseconds since the Unix epoch), if encoded in its folder name.
    pub removed_at: Option<i64>,
    /// The bytes of each part, in the order of `Part::ALL`.
    pub parts: [u64; 6],
}

impl RemovedLibrarySpace {
    pub fn total(&self) -> u64 {
        self.parts.iter().sum()
    }
}

/// The whole data folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Space {
    /// Every library, conversations not yet made libraries among them, the largest first.
    pub libraries: Vec<LibrarySpace>,
    /// Removed libraries and home-page conversations in `removed/`, the largest first.
    pub removed_libraries: Vec<RemovedLibrarySpace>,
    /// `removed/` under the data folder in total: the removed libraries plus anything else left there.
    pub removed: u64,
    /// Everything else under the data folder (citation styles, files left there).
    pub other: u64,
}

impl Space {
    pub fn total(&self) -> u64 {
        self.libraries.iter().map(LibrarySpace::total).sum::<u64>() + self.removed + self.other
    }

    /// Bytes inside `removed/` that do not belong to a recognized removed library.
    pub fn removed_other(&self) -> u64 {
        let known: u64 = self.removed_libraries.iter().map(RemovedLibrarySpace::total).sum();
        self.removed.saturating_sub(known)
    }
}

/// Measure the data folder at `root`. A folder that cannot be read counts as empty.
pub fn measure(root: &Path) -> Space {
    let found = libraries::list(root);
    let mut space = Space::default();
    for info in &found {
        space.libraries.push(measure_library(info));
    }
    space.libraries.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.name.cmp(&b.name)));
    let removed_found = libraries::list_removed(root);
    for item in &removed_found {
        let parts = measure_parts(&item.info.dir);
        space.removed_libraries.push(RemovedLibrarySpace {
            folder: item.folder.clone(),
            id: item.info.id.clone(),
            name: item.info.name.clone(),
            color: item.info.color.clone(),
            draft: item.info.draft,
            dir: item.info.dir.clone(),
            paper_count: item.info.paper_count,
            removed_at: item.removed_at,
            parts,
        });
    }
    space.removed_libraries.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| b.removed_at.cmp(&a.removed_at)).then_with(|| a.name.cmp(&b.name)));
    let Ok(entries) = std::fs::read_dir(root) else { return space };
    for entry in entries.flatten() {
        let path = entry.path();
        if found.iter().any(|info| info.dir == path) { continue }
        if entry.file_name() == "removed" {
            let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|meta| !meta.file_type().is_symlink() && meta.is_dir());
            if is_dir && let Ok(removed_entries) = std::fs::read_dir(&path) {
                for sub in removed_entries.flatten() {
                    let sub_path = sub.path();
                    if let Some(lib) = space.removed_libraries.iter().find(|lib| lib.dir == sub_path) {
                        space.removed += lib.total();
                    } else {
                        space.removed += size_of(&sub_path);
                    }
                }
            } else {
                space.removed += size_of(&path);
            }
        } else {
            space.other += size_of(&path);
        }
    }
    space
}

fn measure_parts(dir: &Path) -> [u64; 6] {
    let mut parts = [0u64; 6];
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let part = Part::of(&entry.file_name().to_string_lossy());
            let index = Part::ALL.iter().position(|p| *p == part).unwrap_or(Part::ALL.len() - 1);
            parts[index] += size_of(&entry.path());
        }
    }
    parts
}

fn measure_library(info: &LibraryInfo) -> LibrarySpace {
    let parts = measure_parts(&info.dir);
    LibrarySpace { id: info.id.clone(), name: info.name.clone(), color: info.color.clone(), draft: info.draft, dir: info.dir.clone(), paper_count: info.paper_count, parts }
}

/// The bytes of a file, or of everything under a folder. A link counts as itself, not what it points to.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() { return meta.len() }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|entry| size_of(&entry.path())).sum()
}

/// A size as people read it: 0 B, 512 B, 1.5 KB, 23.4 MB, 1.02 GB (powers of 1024).
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 { return format!("{bytes} B") }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    // Three figures: 1.50 GB, 23.4 MB, 512 KB.
    let digits = if value < 10. { 2 } else if value < 100. { 1 } else { 0 };
    format!("{value:.digits$} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::new_id;

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn each_library_is_measured_by_its_parts() {
        let root = std::env::temp_dir().join(format!("wisp-space-{}", new_id("T")));
        let small = libraries::create(&root, "小库", "", "blue").unwrap();
        let large = libraries::create(&root, "大库", "", "green").unwrap();
        write(&large.dir.join("papers").join("P-1").join("a.pdf"), 5000);
        write(&large.dir.join("papers").join("P-2").join("b.pdf"), 3000);
        write(&large.dir.join(crate::figure::FOLDER).join("f.png"), 700);
        write(&large.dir.join(crate::manuscript::FOLDER).join("m.json"), 300);
        write(&large.dir.join("removed").join("old.pdf"), 200);
        write(&small.dir.join("papers").join("P-3").join("c.pdf"), 100);
        write(&root.join("removed").join("L-gone").join("papers").join("d.pdf"), 4000);
        write(&root.join(crate::csl::FOLDER).join("apa.csl"), 50);

        let space = measure(&root);
        let names: Vec<&str> = space.libraries.iter().map(|lib| lib.name.as_str()).collect();
        assert_eq!(names, ["大库", "小库"]);
        let big = &space.libraries[0];
        assert_eq!((big.part(Part::Files), big.part(Part::Figures), big.part(Part::Manuscripts), big.part(Part::Removed)), (8000, 700, 300, 200));
        // The database was made with the library; library.json is the rest.
        assert!(big.part(Part::Database) > 0 && big.part(Part::Other) > 0);
        assert_eq!(big.total(), big.parts.iter().sum::<u64>());
        assert_eq!(space.libraries[1].part(Part::Files), 100);
        assert_eq!((space.removed, space.other), (4000, 50));
        assert_eq!(space.total(), space.libraries.iter().map(LibrarySpace::total).sum::<u64>() + 4050);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_folder_is_empty() {
        let space = measure(Path::new("/no/such/wisp/folder"));
        assert_eq!(space.total(), 0);
        assert!(space.libraries.is_empty());
    }

    #[test]
    fn removed_libraries_are_measured_beside_loose_removed_files() {
        let root = std::env::temp_dir().join(format!("wisp-space-rem-{}", new_id("T")));
        let active = libraries::create(&root, "在用", "", "blue").unwrap();
        let gone = libraries::create(&root, "已移走", "", "amber").unwrap();
        write(&gone.dir.join("papers").join("P-1").join("a.pdf"), 6000);
        let moved = libraries::remove(&root, &gone).unwrap();
        let folder = moved.file_name().unwrap().to_string_lossy().to_string();
        write(&root.join("removed").join("stray.bin"), 500);

        let space = measure(&root);
        assert_eq!(space.libraries.len(), 1);
        assert_eq!(space.libraries[0].id, active.id);
        assert_eq!(space.removed_libraries.len(), 1);
        let rem = &space.removed_libraries[0];
        assert_eq!((rem.folder.as_str(), rem.name.as_str(), rem.color.as_str()), (folder.as_str(), "已移走", "amber"));
        assert_eq!(rem.parts[0], 6000);
        assert_eq!(space.removed, rem.total() + 500);
        assert_eq!(space.removed_other(), 500);
        assert_eq!(space.total(), space.libraries[0].total() + space.removed);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1536), "1.50 KB");
        assert_eq!(human(24_536_678), "23.4 MB");
        assert_eq!(human(512 * 1024), "512 KB");
        assert_eq!(human(1_100_000_000), "1.02 GB");
    }
}
