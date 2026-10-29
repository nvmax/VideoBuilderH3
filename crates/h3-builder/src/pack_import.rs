//! Choosing what to import from a folder of ready-made effect packs, instead of importing every video in it.
//!
//! A pack is a folder (`4_Action_Pack_Overlay_1`) with a `filter.xml` (in the pack's `Data` folder) that names its clip, how the clip is blended
//! (`<BlendType>`) and whether it is a transition (`<Transition>1`). Many packs also have a `.ml` file that names a second clip, the matte
//! (`AlphaLayer="1"`): its green channel says where the first clip is visible. A pack has a `<GUID>`, which is how the app remembers what it
//! imported, so the next scan of the same folder only lists what is new.
//!
//! The overlay import lists the packs whose folder name says Overlay and that this app can use: a clip with a matte (the two are joined into one
//! transparent clip) and a clip that only needs a blend mode (8 is screen, 9 overlay, 2 multiply). Other formats are counted and left out, so
//! nothing is copied that would not work. A folder without packs lists its video files instead, so a plain download folder works too.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use h3_core::pyval::Obj;
use serde_json::{json, Map, Value};

use crate::overlay_library::{self as library, urlencode};
use crate::payload::path_text;
use crate::Env;

type Reply = Result<Value, String>;

const FILTER_FILE: &str = "filter.xml";
const MAX_DEPTH: usize = 5;
const MAX_PLAIN_FILES: usize = 3000;
const CLIP_EXTENSIONS: [&str; 6] = ["mp4", "mov", "webm", "m4v", "mkv", "avi"];

/// The text between `<name>` and `</name>`.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(xml[start..end].trim())
}

/// The value of `name="..."` in the text of one tag, with any spaces around the `=`.
fn attr(text: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = text[from..].find(name) {
        let i = from + at;
        let rest = text[i + name.len()..].trim_start();
        let alone = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        if alone && rest.starts_with('=') {
            let rest = rest[1..].trim_start();
            let quote = rest.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &rest[1..];
                return Some(inner[..inner.find(quote)?].to_string());
            }
        }
        from = i + name.len();
    }
    None
}

/// The file of the layer that is flagged `AlphaLayer="1"` in the text of a `.ml` file.
fn alpha_layer(ml: &str) -> Option<String> {
    ml.split("<Layer").skip(1).find_map(|part| {
        let part = part.split('>').next()?;
        if attr(part, "AlphaLayer").as_deref() == Some("1") { attr(part, "File") } else { None }
    })
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned())
}

fn is_clip(path: &Path) -> bool {
    path.extension().is_some_and(|e| CLIP_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// A clip named in a pack's XML, in the pack's data folder. The name may carry a key color (`Clip#FFFFFF.mp4`) that the file itself does not.
fn find_clip(dir: &Path, named: &str) -> Option<PathBuf> {
    let direct = dir.join(named);
    if direct.is_file() {
        return Some(direct);
    }
    let (stem, ext) = named.rsplit_once('.')?;
    let plain = format!("{}.{ext}", stem.split('#').next().unwrap_or(stem));
    Some(dir.join(plain)).filter(|p| p.is_file())
}

/// How this app brings a pack in.
enum Plan {
    /// The clip as it is, with the blend mode the library should suggest for it.
    Plain(&'static str),
    /// The clip and its matte, joined into one transparent clip.
    Joined(PathBuf),
}

struct Pack {
    id: String,
    name: String,
    folder: PathBuf,
    image: Option<PathBuf>,
    video: Option<PathBuf>,
    matte: Option<PathBuf>,
    blend: Option<u32>,
    transition: bool,
}

impl Pack {
    /// An overlay: not a transition, and its folder or its name says Overlay.
    fn is_overlay(&self) -> bool {
        let folder = self.folder.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        !self.transition && (folder.contains("overlay") || self.name.to_lowercase().contains("overlay"))
    }

    /// How the pack can be imported, or why it cannot.
    fn plan(&self) -> Result<Plan, &'static str> {
        self.video.as_ref().filter(|v| is_clip(v)).ok_or("its clip is missing or in a format this app does not read yet")?;
        if let Some(matte) = &self.matte {
            return Ok(Plan::Joined(matte.clone()));
        }
        match self.blend {
            Some(8) => Ok(Plan::Plain("screen")),
            Some(9) => Ok(Plan::Plain("overlay")),
            Some(2) => Ok(Plan::Plain("multiply")),
            _ => Err("it needs a blend this app does not know yet"),
        }
    }

    fn size(&self) -> u64 {
        [&self.video, &self.matte].into_iter().flatten().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum()
    }
}

fn parse_pack(filter: &Path) -> Option<Pack> {
    let xml = read_text(filter)?;
    let data = filter.parent()?;
    let folder = if data.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("data")) { data.parent()? } else { data };
    let folder_name = folder.file_name()?.to_string_lossy().into_owned();
    let name = tag(&xml, "Name").filter(|n| !n.is_empty()).map_or_else(|| folder_name.clone(), str::to_string);
    let id = tag(&xml, "GUID").filter(|g| !g.is_empty()).map_or_else(|| folder_name.to_lowercase(), str::to_lowercase);
    let video = tag(&xml, "VideoFile").and_then(|file| find_clip(data, file));
    let matte = fs::read_dir(data)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ml")))
        .and_then(|ml| read_text(&ml))
        .and_then(|text| alpha_layer(&text))
        .and_then(|file| find_clip(data, &file));
    let image = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg")));
    Some(Pack {
        id,
        name,
        folder: folder.to_path_buf(),
        image,
        video,
        matte,
        blend: tag(&xml, "BlendType").and_then(|b| b.parse().ok()),
        transition: tag(&xml, "Transition") == Some("1"),
    })
}

/// Every `filter.xml` under `root`, a few folders deep.
fn find_filters(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let mut entries: Vec<_> = fs::read_dir(&dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if name.eq_ignore_ascii_case(FILTER_FILE) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The overlay packs under `root`, by id. A second pack with the same id is ignored.
fn overlay_packs(root: &Path) -> (Vec<Pack>, usize) {
    let (mut packs, mut others) = (Vec::new(), 0);
    let mut seen = HashMap::new();
    for filter in find_filters(root) {
        let Some(pack) = parse_pack(&filter) else { continue };
        if !pack.is_overlay() {
            others += 1;
        } else if seen.insert(pack.id.clone(), ()).is_none() {
            packs.push(pack);
        }
    }
    packs.sort_by_key(|p| p.folder.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    (packs, others)
}

fn ledger_path(env: &Env) -> PathBuf {
    env.overlay_library().join(".imported_packs.json")
}

/// What was imported before: pack id (or `file:<path>` for a plain video) to the library clip it became.
fn read_ledger(env: &Env) -> Map<String, Value> {
    fs::read(ledger_path(env)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.get("packs").and_then(Value::as_object).cloned()).unwrap_or_default()
}

fn write_ledger(env: &Env, packs: &Map<String, Value>) -> Result<(), String> {
    let path = ledger_path(env);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let temp = path.with_extension("json.partial");
    let body = serde_json::to_vec_pretty(&json!({"packs": packs})).map_err(|e| e.to_string())?;
    fs::write(&temp, body).map_err(|e| format!("{}: {e}", temp.display()))?;
    fs::rename(&temp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Imported before, and its clip is still in the library. A clip that was deleted from the library is offered again.
fn already_imported(env: &Env, ledger: &Map<String, Value>, id: &str) -> bool {
    ledger.get(id).and_then(|entry| entry.get("library_id")).and_then(Value::as_str).is_some_and(|library_id| env.overlay_library().join(library_id).is_file())
}

fn chosen_folder(payload: &Obj) -> Result<PathBuf, String> {
    let wanted = path_text(payload, "path");
    if wanted.is_empty() {
        return Err("path is required.".into());
    }
    let root = h3_core::abspath::absolute(Path::new(&wanted)).map_err(|e| e.to_string())?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder.", root.display()));
    }
    Ok(root)
}

fn image_url(root: &Path, pack: &Pack) -> Value {
    let Some(relative) = pack.image.as_ref().and_then(|image| image.strip_prefix(root).ok()) else { return Value::Null };
    json!(format!("/vrgdg/music_builder/overlay_library/packs/image?root={}&file={}", urlencode(&root.display().to_string()), urlencode(&relative.display().to_string().replace('\\', "/"))))
}

/// `POST /vrgdg/music_builder/overlay_library/packs/scan`: `{path}`. What can be imported from the folder, without the packs imported before.
/// A folder of packs gives `packs`, a folder without any gives its video `files`.
pub fn scan(env: &Env, payload: &Obj) -> Reply {
    let root = chosen_folder(payload)?;
    let ledger = read_ledger(env);
    if find_filters(&root).is_empty() {
        return scan_files(&root, &ledger, env);
    }
    let (packs, others) = overlay_packs(&root);
    let (mut listed, mut unsupported, mut hidden) = (Vec::new(), 0, 0);
    for pack in &packs {
        let plan = match pack.plan() {
            Ok(plan) => plan,
            Err(_) => {
                unsupported += 1;
                continue;
            }
        };
        if already_imported(env, &ledger, &pack.id) {
            hidden += 1;
            continue;
        }
        let note = match plan {
            Plan::Joined(_) => "A clip with a matte: they are joined into one transparent clip.".to_string(),
            Plan::Plain(blend) => format!("Blended as {blend}."),
        };
        listed.push(json!({"id": pack.id, "name": pack.name, "folder": pack.folder.file_name().map(|n| n.to_string_lossy().into_owned()), "size_bytes": pack.size(), "image_url": image_url(&root, pack), "note": note}));
    }
    Ok(json!({"mode": "packs", "root": root.display().to_string(), "packs": listed, "hidden": hidden, "unsupported": unsupported, "not_overlays": others}))
}

fn relative_id(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/")
}

fn scan_files(root: &Path, ledger: &Map<String, Value>, env: &Env) -> Reply {
    let (videos, refused) = library::videos_in(root);
    let (mut listed, mut hidden) = (Vec::new(), 0);
    for path in &videos {
        let id = relative_id(root, path);
        if already_imported(env, ledger, &format!("file:{id}")) {
            hidden += 1;
        } else if listed.len() < MAX_PLAIN_FILES {
            let folder = Path::new(&id).parent().map(|p| p.display().to_string()).unwrap_or_default();
            listed.push(json!({"id": id, "name": path.file_stem().map(|s| s.to_string_lossy().into_owned()), "folder": folder, "size_bytes": fs::metadata(path).map(|m| m.len()).unwrap_or(0)}));
        }
    }
    Ok(json!({"mode": "files", "root": root.display().to_string(), "files": listed, "hidden": hidden, "truncated": videos.len() - hidden > MAX_PLAIN_FILES, "refused": refused.len()}))
}

/// The clip and its matte as one clip with transparency: the matte's green channel becomes the alpha channel.
fn join_matte(env: &Env, video: &Path, matte: &Path, out: &Path) -> Result<(), String> {
    let tools = env.tools()?;
    let graph = "[0:v]format=rgb24[c];[1:v]format=gbrp,extractplanes=g[a];[c][a]alphamerge,format=yuva444p10le[v]";
    let args = h3_media::tools::args(&[
        "-y", "-v", "error", "-i", &video.display().to_string(), "-i", &matte.display().to_string(), "-filter_complex", graph, "-map", "[v]", "-an", "-c:v", "prores_ks", "-profile:v", "4444",
        "-qscale:v", "16", "-pix_fmt", "yuva444p10le", &out.display().to_string(),
    ]);
    tools.ffmpeg_checked(&args, &format!("ffmpeg could not join {} and its matte", video.display())).map(|_| ()).map_err(|e| e.to_string())
}

fn unique_name() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}_{nanos}", std::process::id())
}

fn import_pack(env: &Env, pack: &Pack) -> Result<(Value, bool), String> {
    let video = pack.video.as_ref().ok_or("its clip is missing")?;
    match pack.plan().map_err(str::to_string)? {
        Plan::Plain(blend) => library::import_file(env, video, "", Some(blend)),
        Plan::Joined(matte) => {
            let work = env.overlay_library().join(".tmp").join(unique_name());
            fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
            let joined = work.join(format!("{}.mov", library::sanitize_file_name(&pack.name)));
            let result = join_matte(env, video, &matte, &joined).and_then(|()| library::import_file(env, &joined, "", Some("normal")));
            let _ = fs::remove_dir_all(&work);
            result
        }
    }
}

fn remember(ledger: &mut Map<String, Value>, id: &str, name: &str, item: &Value) {
    let when = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    ledger.insert(id.to_string(), json!({"name": name, "library_id": item["id"], "imported_at": when}));
}

/// A video path inside `root` named by a scan: relative, and nowhere else.
fn inside(root: &Path, id: &str) -> Option<PathBuf> {
    let relative = Path::new(id);
    relative.components().all(|c| matches!(c, Component::Normal(_))).then(|| root.join(relative)).filter(|p| p.is_file())
}

/// `POST /vrgdg/music_builder/overlay_library/packs/import`: `{path, ids}`. Imports the chosen packs (or files) and remembers them.
pub fn import(env: &Env, payload: &Obj) -> Reply {
    let root = chosen_folder(payload)?;
    let ids: Vec<String> = payload.get("ids").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err("Choose at least one to import.".into());
    }
    let mut ledger = read_ledger(env);
    let (mut added, mut already, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    let packs_mode = !find_filters(&root).is_empty();
    let packs: HashMap<String, Pack> = if packs_mode { overlay_packs(&root).0.into_iter().map(|p| (p.id.clone(), p)).collect() } else { HashMap::new() };
    for id in &ids {
        let (name, result) = if packs_mode {
            match packs.get(id) {
                Some(pack) => (pack.name.clone(), import_pack(env, pack)),
                None => (id.clone(), Err("That pack is not in the folder.".to_string())),
            }
        } else {
            match inside(&root, id) {
                Some(path) => (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), library::import_file(env, &path, "", None)),
                None => (id.clone(), Err("That file is not in the folder.".to_string())),
            }
        };
        match result {
            Ok((item, copied)) => {
                remember(&mut ledger, &if packs_mode { id.clone() } else { format!("file:{id}") }, &name, &item);
                if copied { added.push(item) } else { already.push(item) }
            }
            Err(error) => failed.push(json!({"id": id, "name": name, "error": error})),
        }
    }
    write_ledger(env, &ledger)?;
    Ok(json!({"added": added, "already_there": already, "failed": failed, "items": library::list(env)?["items"]}))
}

/// The picture of a pack (`GET .../packs/image?root=&file=`), only when it is an image inside the folder that was scanned.
pub fn image_path(root: &str, file: &str) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root.trim()).map_err(|_| "That folder is not there.".to_string())?;
    let path = fs::canonicalize(root.join(file.trim())).map_err(|_| "That picture is not there.".to_string())?;
    let is_image = path.extension().is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg"));
    if path.starts_with(&root) && is_image && path.is_file() { Ok(path) } else { Err("That is not a picture of a pack.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_and_an_attribute_are_read_whatever_the_spacing() {
        let xml = "<Filter><GUID> AB-12 </GUID><Name>Film 4</Name><BlendType>8</BlendType></Filter>";
        assert_eq!((tag(xml, "GUID"), tag(xml, "Name"), tag(xml, "BlendType"), tag(xml, "Missing")), (Some("AB-12"), Some("Film 4"), Some("8"), None));
        let layer = r#"<Layer File = "A_L0.mp4" AlphaLayer="1" BlendType='6'/>"#;
        assert_eq!((attr(layer, "File").as_deref(), attr(layer, "AlphaLayer").as_deref(), attr(layer, "BlendType").as_deref(), attr(layer, "Alpha")), (Some("A_L0.mp4"), Some("1"), Some("6"), None));
    }

    #[test]
    fn the_matte_is_the_layer_flagged_as_alpha_and_a_plain_layer_is_not() {
        let both = "<Layers><Layer File=\"plain.mp4\" AlphaLayer=\"0\" BlendType=\"0\"/><Layer File = \"matte.webm\" AlphaLayer=\"1\" BlendType=\"6\"/></Layers>";
        assert_eq!(alpha_layer(both).as_deref(), Some("matte.webm"));
        assert_eq!(alpha_layer("<Layers><Layer File=\"plain.mp4\" AlphaLayer=\"0\"/></Layers>"), None);
        assert_eq!(alpha_layer("<Layers></Layers>"), None);
    }
}
