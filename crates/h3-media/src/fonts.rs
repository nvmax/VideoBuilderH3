//! The fonts of text overlays. Six free fonts (SIL Open Font License, text in `fonts/OFL.txt`) are part of the program and are written to a
//! `fonts` folder the first time text is drawn, because ffmpeg reads a font from a file. Anyone can put their own `.ttf` or `.otf` file in
//! the same folder and choose it by name. A text clip names its font by the name shown in the list (`Anton`), by the file stem
//! (`Anton-Regular`) or by the file name.
//!
//! The fonts installed on the computer can be used too. They are read from the system font folders and named after their family and style
//! (`Segoe UI`, `Segoe UI Bold`, `Arial Black`). A font in the `fonts` folder wins over an installed font of the same name. Font collections
//! (`.ttc`) are left out: a browser cannot load one for the preview, and ffmpeg would only read the first font in it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A font that is part of the program.
pub struct BundledFont {
    pub name: &'static str,
    pub file: &'static str,
    pub bytes: &'static [u8],
}

pub const DEFAULT_FONT: &str = "Anton";

pub const BUNDLED: [BundledFont; 6] = [
    BundledFont { name: "Anton", file: "Anton-Regular.ttf", bytes: include_bytes!("../fonts/Anton-Regular.ttf") },
    BundledFont { name: "Bebas Neue", file: "BebasNeue-Regular.ttf", bytes: include_bytes!("../fonts/BebasNeue-Regular.ttf") },
    BundledFont { name: "Oswald", file: "Oswald.ttf", bytes: include_bytes!("../fonts/Oswald.ttf") },
    BundledFont { name: "Montserrat", file: "Montserrat.ttf", bytes: include_bytes!("../fonts/Montserrat.ttf") },
    BundledFont { name: "Playfair Display", file: "PlayfairDisplay.ttf", bytes: include_bytes!("../fonts/PlayfairDisplay.ttf") },
    BundledFont { name: "Caveat", file: "Caveat.ttf", bytes: include_bytes!("../fonts/Caveat.ttf") },
];

/// A font as the list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct FontEntry {
    pub name: String,
    pub file: String,
    pub bundled: bool,
    /// Installed on the computer, not in the `fonts` folder.
    pub system: bool,
}

/// Make the bundled fonts present in `dir`, writing the ones that are missing or have a different size (an older program version's file).
pub fn ensure_fonts(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for font in &BUNDLED {
        let path = dir.join(font.file);
        if fs::metadata(&path).map(|m| m.len()).ok() == Some(font.bytes.len() as u64) {
            continue;
        }
        let partial = dir.join(format!(".{}.partial", font.file));
        fs::write(&partial, font.bytes)?;
        fs::rename(&partial, &path)?;
    }
    Ok(())
}

fn key(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn is_font_file(path: &Path) -> bool {
    matches!(path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("ttf" | "otf"))
}

/// The bundled fonts, then any other font file in `dir`, named after its file.
pub fn list_fonts(dir: &Path) -> Vec<FontEntry> {
    let mut fonts: Vec<FontEntry> = BUNDLED.iter().map(|f| FontEntry { name: f.name.to_string(), file: f.file.to_string(), bundled: true, system: false }).collect();
    let mut own: Vec<FontEntry> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_font_file(p))
        .filter_map(|p| {
            let file = p.file_name()?.to_string_lossy().into_owned();
            let name = p.file_stem()?.to_string_lossy().into_owned();
            (!file.starts_with('.') && !BUNDLED.iter().any(|f| f.file.eq_ignore_ascii_case(&file))).then_some(FontEntry { name, file, bundled: false, system: false })
        })
        .collect();
    own.sort_by_key(|font| font.name.to_lowercase());
    fonts.extend(own);
    fonts
}

/// A font installed on the computer.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemFont {
    pub name: String,
    pub path: PathBuf,
}

static SYSTEM_FONTS: Mutex<Option<Arc<Vec<SystemFont>>>> = Mutex::new(None);

/// The installed fonts. The system font folders are read the first time and again when `rescan` is set (a font installed while the program runs).
pub fn system_fonts(rescan: bool) -> Arc<Vec<SystemFont>> {
    let mut slot = SYSTEM_FONTS.lock().unwrap_or_else(|e| e.into_inner());
    if rescan || slot.is_none() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let bundled = BUNDLED.iter().map(|f| key(f.name)).collect();
        *slot = Some(Arc::new(system_font_list(&db, bundled)));
    }
    slot.clone().unwrap_or_default()
}

/// Fonts that only hold symbols or icons, which would fill the list with names nobody can write a title in.
fn is_symbol_font(family: &str) -> bool {
    let family = family.to_lowercase();
    ["wingdings", "webdings", "symbol", "marlett", "mdl2", "dingbats", "icons"].iter().any(|word| family.contains(word))
}

fn weight_label(weight: u16) -> &'static str {
    match weight {
        0..=149 => "Thin",
        150..=249 => "Extra Light",
        250..=324 => "Light",
        325..=374 => "Semilight",
        375..=449 => "",
        450..=549 => "Medium",
        550..=649 => "Semibold",
        650..=749 => "Bold",
        750..=849 => "Extra Bold",
        _ => "Black",
    }
}

fn stretch_label(stretch: fontdb::Stretch) -> &'static str {
    match stretch {
        fontdb::Stretch::UltraCondensed => "Ultra Condensed",
        fontdb::Stretch::ExtraCondensed => "Extra Condensed",
        fontdb::Stretch::Condensed => "Condensed",
        fontdb::Stretch::SemiCondensed => "Semi Condensed",
        fontdb::Stretch::Normal => "",
        fontdb::Stretch::SemiExpanded => "Semi Expanded",
        fontdb::Stretch::Expanded => "Expanded",
        fontdb::Stretch::ExtraExpanded => "Extra Expanded",
        fontdb::Stretch::UltraExpanded => "Ultra Expanded",
    }
}

/// The family, then the width, weight and slant when they are not the plain ones (`Segoe UI Bold Italic`). A word the family already has
/// (`Arial Black`) is not added again.
fn face_name(family: &str, stretch: &str, weight: u16, italic: bool) -> String {
    let have = family.to_lowercase();
    let mut name = family.to_string();
    for part in [stretch, weight_label(weight), if italic { "Italic" } else { "" }] {
        if !part.is_empty() && !have.contains(&part.to_lowercase()) {
            name.push(' ');
            name.push_str(part);
        }
    }
    name
}

/// One entry for each installed font file, named by `face_name`. `taken` holds the keys of names that are already used (the bundled fonts).
fn system_font_list(db: &fontdb::Database, mut taken: std::collections::HashSet<String>) -> Vec<SystemFont> {
    let mut fonts = Vec::new();
    for face in db.faces() {
        let (fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _)) = &face.source else { continue };
        if face.index != 0 || !is_font_file(path) {
            continue;
        }
        let Some((family, _)) = face.families.first() else { continue };
        let family = family.trim();
        if family.is_empty() || is_symbol_font(family) {
            continue;
        }
        let name = face_name(family, stretch_label(face.stretch), face.weight.0, face.style != fontdb::Style::Normal);
        if taken.insert(key(&name)) {
            fonts.push(SystemFont { name, path: path.clone() });
        }
    }
    fonts.sort_by_key(|font| font.name.to_lowercase());
    fonts
}

/// An installed font by its name, then by its file name or the file name without `.ttf` (the render asks for a font by file name after the plan
/// has resolved the name once). `wanted` is a `key`.
fn find_system_font(list: &[SystemFont], wanted: &str) -> Option<PathBuf> {
    let by_file = |f: &SystemFont| {
        f.path.file_name().is_some_and(|n| key(&n.to_string_lossy()) == wanted) || f.path.file_stem().is_some_and(|s| key(&s.to_string_lossy()) == wanted)
    };
    list.iter().find(|f| key(&f.name) == wanted).or_else(|| list.iter().find(|f| by_file(f))).map(|f| f.path.clone()).filter(|p| p.is_file())
}

fn with_system_fonts(mut fonts: Vec<FontEntry>, system: &[SystemFont]) -> Vec<FontEntry> {
    let taken: std::collections::HashSet<String> = fonts.iter().map(|f| key(&f.name)).collect();
    fonts.extend(system.iter().filter(|f| !taken.contains(&key(&f.name))).map(|f| FontEntry {
        name: f.name.clone(),
        file: f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        bundled: false,
        system: true,
    }));
    fonts
}

/// The fonts of `list_fonts`, then the fonts installed on the computer. `rescan` reads the system font folders again.
pub fn list_all_fonts(dir: &Path, rescan: bool) -> Vec<FontEntry> {
    with_system_fonts(list_fonts(dir), &system_fonts(rescan))
}

/// The file of the font a clip names, in `dir` or installed on the computer, or `None` when there is no such font.
pub fn resolve_font(dir: &Path, wanted: &str) -> Option<PathBuf> {
    let wanted = key(wanted.trim().trim_matches('"'));
    if wanted.is_empty() {
        return None;
    }
    list_fonts(dir)
        .into_iter()
        .find(|f| key(&f.name) == wanted || key(&f.file) == wanted || key(Path::new(&f.file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().as_str()) == wanted)
        .map(|f| dir.join(f.file))
        .filter(|p| p.is_file())
        .or_else(|| find_system_font(&system_fonts(false), &wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_fonts_are_written_once_and_found_by_any_of_their_names() {
        let dir = tempfile::tempdir().unwrap();
        let fonts = dir.path().join("fonts");
        ensure_fonts(&fonts).unwrap();
        for font in &BUNDLED {
            assert_eq!(fs::metadata(fonts.join(font.file)).unwrap().len(), font.bytes.len() as u64, "{}", font.name);
            assert_eq!(&font.bytes[..4], &[0, 1, 0, 0], "{} is a TrueType file", font.name);
        }
        let stamp = fs::metadata(fonts.join("Anton-Regular.ttf")).unwrap().modified().unwrap();
        ensure_fonts(&fonts).unwrap();
        assert_eq!(fs::metadata(fonts.join("Anton-Regular.ttf")).unwrap().modified().unwrap(), stamp, "nothing is written again");
        for name in ["Anton", "anton", " ANTON ", "Anton-Regular", "Anton-Regular.ttf", "anton regular"] {
            assert_eq!(resolve_font(&fonts, name), Some(fonts.join("Anton-Regular.ttf")), "{name}");
        }
        assert_eq!(resolve_font(&fonts, "Playfair Display"), Some(fonts.join("PlayfairDisplay.ttf")));
        assert_eq!(resolve_font(&fonts, "Bebas Neue"), Some(fonts.join("BebasNeue-Regular.ttf")));
        assert_eq!(resolve_font(&fonts, "Comic Sans"), None);
        assert_eq!(resolve_font(&fonts, ""), None);
    }

    #[test]
    fn a_damaged_or_old_font_file_is_replaced_and_a_users_own_font_is_listed_and_found() {
        let dir = tempfile::tempdir().unwrap();
        let fonts = dir.path().join("fonts");
        fs::create_dir_all(&fonts).unwrap();
        fs::write(fonts.join("Caveat.ttf"), b"broken").unwrap();
        fs::write(fonts.join("My Hand Font.otf"), b"x").unwrap();
        fs::write(fonts.join("notes.txt"), b"x").unwrap();
        ensure_fonts(&fonts).unwrap();
        assert_eq!(fs::metadata(fonts.join("Caveat.ttf")).unwrap().len(), BUNDLED[5].bytes.len() as u64);
        let list = list_fonts(&fonts);
        assert_eq!(list.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Anton", "Bebas Neue", "Oswald", "Montserrat", "Playfair Display", "Caveat", "My Hand Font"]);
        assert!(list[..6].iter().all(|f| f.bundled) && !list[6].bundled);
        assert_eq!(resolve_font(&fonts, "my hand font"), Some(fonts.join("My Hand Font.otf")));
        assert_eq!(resolve_font(&fonts, "My Hand Font.otf"), Some(fonts.join("My Hand Font.otf")));
        assert!(list_fonts(&dir.path().join("none")).iter().all(|f| f.bundled), "a folder that is not there lists the bundled fonts only");
    }

    #[test]
    fn an_installed_font_is_named_by_family_width_weight_and_slant_without_repeating_a_word_of_the_family() {
        assert_eq!(face_name("Segoe UI", "", 400, false), "Segoe UI");
        assert_eq!(face_name("Segoe UI", "", 700, true), "Segoe UI Bold Italic");
        assert_eq!(face_name("Segoe UI", "", 350, false), "Segoe UI Semilight");
        assert_eq!(face_name("Segoe UI Semibold", "", 600, false), "Segoe UI Semibold");
        assert_eq!(face_name("Arial Black", "", 900, false), "Arial Black");
        assert_eq!(face_name("Bahnschrift", "Condensed", 400, false), "Bahnschrift Condensed");
        assert_eq!(face_name("Bahnschrift", "Semi Condensed", 300, true), "Bahnschrift Semi Condensed Light Italic");
        for family in ["Wingdings 2", "Webdings", "Segoe MDL2 Assets", "Segoe Fluent Icons", "Marlett", "Symbol"] {
            assert!(is_symbol_font(family), "{family}");
        }
        assert!(!is_symbol_font("Segoe UI") && !is_symbol_font("Georgia"));
    }

    #[test]
    fn font_files_in_a_folder_become_installed_fonts_with_their_own_names_and_never_repeat_one_that_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        ensure_fonts(dir.path()).unwrap();
        let mut db = fontdb::Database::new();
        db.load_fonts_dir(dir.path());
        let all = system_font_list(&db, Default::default());
        let names: Vec<&str> = all.iter().map(|f| f.name.as_str()).collect();
        // A variable font is named after its default weight (the bundled Montserrat file starts at Thin), which is the weight ffmpeg draws.
        for font in &BUNDLED {
            assert!(names.iter().any(|n| key(n).starts_with(&key(font.name))), "{} is in {names:?}", font.name);
        }
        assert!(names.windows(2).all(|pair| pair[0].to_lowercase() <= pair[1].to_lowercase()), "sorted: {names:?}");
        assert!(all.iter().all(|f| f.path.is_file() && f.path.starts_with(dir.path())));
        let taken = system_font_list(&db, BUNDLED.iter().map(|f| key(f.name)).collect());
        assert!(taken.len() < all.len(), "the names that are taken are left out");
        assert!(taken.iter().all(|f| BUNDLED.iter().all(|b| key(&f.name) != key(b.name))), "the bundled names are not listed again: {taken:?}");
    }

    #[test]
    fn an_installed_font_is_found_by_its_name_and_a_font_of_the_same_name_in_the_folder_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sgoeui.ttf");
        fs::write(&file, b"x").unwrap();
        let installed = vec![SystemFont { name: "Segoe UI Bold".into(), path: file.clone() }, SystemFont { name: "Gone".into(), path: dir.path().join("gone.ttf") }];
        assert_eq!(find_system_font(&installed, &key("segoe ui BOLD")), Some(file.clone()));
        assert_eq!(find_system_font(&installed, &key("sgoeui.ttf")), Some(file.clone()), "by file name, which is how the render asks");
        assert_eq!(find_system_font(&installed, &key("SgoeUI")), Some(file.clone()), "by file name without the extension");
        assert_eq!(find_system_font(&installed, &key("Gone")), None, "a font file that was removed is not found");
        assert_eq!(find_system_font(&installed, &key("Segoe UI")), None);
        let own = with_system_fonts(vec![FontEntry { name: "Segoe UI Bold".into(), file: "mine.otf".into(), bundled: false, system: false }], &installed);
        assert_eq!(own.iter().map(|f| (f.name.as_str(), f.system)).collect::<Vec<_>>(), [("Segoe UI Bold", false), ("Gone", true)]);
        assert_eq!(own[1].file, "gone.ttf");
    }

    #[test]
    fn the_fonts_of_this_computer_are_listed_once_each_and_every_one_can_be_resolved() {
        let installed = system_fonts(true);
        let mut keys = std::collections::HashSet::new();
        for font in installed.iter() {
            assert!(!font.name.trim().is_empty() && font.path.is_file(), "{font:?}");
            assert!(keys.insert(key(&font.name)), "{} is listed twice", font.name);
            assert_eq!(find_system_font(&installed, &key(&font.name)), Some(font.path.clone()), "{}", font.name);
        }
        let dir = tempfile::tempdir().unwrap();
        if let Some(first) = installed.first() {
            assert_eq!(resolve_font(dir.path(), &first.name), Some(first.path.clone()));
            assert!(list_all_fonts(dir.path(), false).iter().any(|f| f.system && f.name == first.name));
        }
    }
}
