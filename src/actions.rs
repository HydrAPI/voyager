//! Context menu customization: built-in items the user can hide and
//! reorder, and custom actions (shell commands) stored in
//! ~/.config/arkha/actions.conf.

use std::path::{Path, PathBuf};

/// Built-in context menu items that can be hidden: (key, label).
/// Keys starting with "bg:" are in the menu for a folder's empty space.
pub const ITEM_ENTRIES: &[(&str, &str)] = &[
    ("open", "Open"),
    ("open-new-tab", "Open in New Tab"),
    ("open-new-window", "Open in New Window"),
    ("open-with", "Open With"),
    ("open-parent", "Open parent location (in search results)"),
    ("cut", "Cut"),
    ("copy", "Copy"),
    ("paste-into", "Paste Into Folder"),
    ("duplicate", "Duplicate"),
    ("make-link", "Make Link"),
    ("rename", "Rename…"),
    ("rename-all", "Rename All… / Advanced Rename…"),
    ("copy-to", "Copy To…"),
    ("move-to", "Move To…"),
    ("trash", "Move to Trash"),
    ("delete", "Delete"),
    ("extract", "Extract Here"),
    ("compress", "Compress…"),
    ("terminal", "Open in Terminal"),
    ("restore", "Restore (in the Trash)"),
    ("properties", "Properties"),
];

pub const BACKGROUND_ENTRIES: &[(&str, &str)] = &[
    ("bg:new-folder", "Create Folder"),
    ("bg:new-document", "Create Document"),
    ("bg:terminal", "Open in Terminal"),
    ("bg:paste", "Paste"),
    ("bg:select-all", "Select All"),
    ("bg:arrange", "Arrange Items"),
    ("bg:view", "View (the whole submenu)"),
    ("bg:view-icons", "View › Icons"),
    ("bg:view-list", "View › List"),
    ("bg:view-compact", "View › Compact"),
    ("bg:view-columns", "View › Columns"),
    ("bg:show-hidden", "View › Show Hidden Files"),
    ("bg:view-sidepane", "View › Side Pane"),
    ("bg:view-extrapane", "View › Extra Pane"),
    ("bg:view-statusbar", "View › Statusbar"),
    ("bg:add-bookmark", "Add Bookmark"),
    ("bg:empty-trash", "Empty Trash (in the Trash)"),
    ("bg:reload", "Reload"),
    ("bg:properties", "Properties"),
];

/// Separators in a menu order.
pub const SEPARATOR: &str = "-";

/// The default order of the menu for selected items (Caja's layout). It has
/// everything that can appear in any situation; what doesn't apply (Restore
/// outside the Trash, say) is left out when the menu is built.
pub const ITEM_ORDER: &[&str] = &[
    "restore", "open", "open-new-tab", "open-new-window", "open-with", "open-parent", SEPARATOR,
    "cut", "copy", "paste-into", SEPARATOR,
    "duplicate", "make-link", "rename", "rename-all", "copy-to", "move-to", SEPARATOR,
    "trash", "delete", SEPARATOR,
    "extract", "compress", "terminal", SEPARATOR,
    "properties",
];

/// The same for a folder's empty space. The View submenu's own entries
/// (VIEW_SUBMENU) keep their order; they can only be hidden.
pub const BACKGROUND_ORDER: &[&str] = &[
    "bg:empty-trash", SEPARATOR,
    "bg:new-folder", "bg:new-document", SEPARATOR,
    "bg:terminal", SEPARATOR,
    "bg:paste", "bg:select-all", SEPARATOR,
    "bg:reload", "bg:view", "bg:arrange", "bg:add-bookmark", SEPARATOR,
    "bg:properties",
];

pub const VIEW_SUBMENU: &[&str] = &[
    "bg:view-icons", "bg:view-list", "bg:view-compact", "bg:view-columns", "bg:show-hidden",
    "bg:view-sidepane", "bg:view-extrapane", "bg:view-statusbar",
];

/// A custom action's key in a menu order (and in `hidden_menu_items`).
pub fn custom_key(a: &CustomAction) -> String {
    format!("custom:{}", a.id)
}

/// The menu order to use: the saved one (`saved`, empty for the default),
/// cleaned up: unknown entries dropped, and built-in entries it doesn't
/// have yet added after the one they follow by default (else in front of
/// Properties), custom actions it doesn't have yet (`customs`, their keys)
/// in front of Properties, in a group of their own as before.
pub fn menu_order(saved: &[String], defaults: &[&str], customs: &[String]) -> Vec<String> {
    let known = |k: &str| k == SEPARATOR || defaults.contains(&k) || customs.iter().any(|c| c == k);
    let mut order: Vec<String> = vec![];
    let start: Vec<String> = if saved.is_empty() { defaults.iter().map(|s| s.to_string()).collect() } else { saved.to_vec() };
    for k in start {
        if known(&k) && (k == SEPARATOR || !order.contains(&k)) {
            order.push(k);
        }
    }
    let props = defaults.last().copied().unwrap_or_default();
    let before_props = |order: &Vec<String>| order.iter().position(|k| k == props).unwrap_or(order.len());
    for (i, k) in defaults.iter().enumerate().filter(|(_, k)| **k != SEPARATOR) {
        if !order.iter().any(|o| o == k) {
            let after = defaults[..i].iter().rev().filter(|d| **d != SEPARATOR).find_map(|d| order.iter().position(|o| o == d));
            let at = after.map_or_else(|| before_props(&order), |p| p + 1);
            order.insert(at, k.to_string());
        }
    }
    let missing: Vec<String> = customs.iter().filter(|c| !order.contains(c)).cloned().collect();
    if !missing.is_empty() {
        let at = before_props(&order);
        let mut group = vec![];
        if at > 0 && order[at - 1] != SEPARATOR {
            group.push(SEPARATOR.to_string());
        }
        group.extend(missing);
        if at < order.len() {
            group.push(SEPARATOR.to_string());
        }
        order.splice(at..at, group);
    }
    order
}

/// A new custom action's id (stable across renaming and reordering, so
/// its place in the menus and its hidden state stay with it).
pub fn new_id() -> String {
    glib::uuid_string_random().chars().filter(|c| *c != '-').take(12).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomAction {
    pub id: String,
    pub name: String,
    /// Run with `sh -c`; see `expand` for the placeholders.
    pub command: String,
    pub files: bool,
    pub folders: bool,
    /// Shown when right-clicking a folder's empty space.
    pub background: bool,
    /// Glob patterns separated by ';', matched against every selected name
    /// (case-insensitive). Empty matches everything.
    pub patterns: String,
    /// File type groups (FILE_TYPES ids) separated by ';'. With patterns
    /// too, a file needs to match either.
    pub types: String,
}

fn path() -> PathBuf {
    crate::config::arkha_file("actions.conf")
}

pub fn load() -> Vec<CustomAction> {
    let kf = glib::KeyFile::new();
    if kf.load_from_file(path(), glib::KeyFileFlags::NONE).is_err() {
        return vec![];
    }
    let mut groups: Vec<(u32, String)> = kf
        .groups()
        .iter()
        .filter_map(|g| Some((g.strip_prefix("Action ")?.parse().ok()?, g.to_string())))
        .collect();
    groups.sort();
    groups
        .into_iter()
        .filter_map(|(_, g)| {
            let s = |k: &str| kf.string(&g, k).map(|v| v.to_string()).unwrap_or_default();
            let b = |k: &str| kf.boolean(&g, k).unwrap_or(false);
            // One written by hand may have none.
            let id = Some(s("id")).filter(|i| !i.is_empty()).unwrap_or_else(new_id);
            let a = CustomAction {
                id,
                name: s("name"),
                command: s("command"),
                files: b("files"),
                folders: b("folders"),
                background: b("background"),
                patterns: s("patterns"),
                types: s("types"),
            };
            (!a.name.is_empty() && !a.command.is_empty()).then_some(a)
        })
        .collect()
}

pub fn save(actions: &[CustomAction]) {
    let kf = glib::KeyFile::new();
    for (i, a) in actions.iter().enumerate() {
        let g = format!("Action {i}");
        kf.set_string(&g, "id", &a.id);
        kf.set_string(&g, "name", &a.name);
        kf.set_string(&g, "command", &a.command);
        kf.set_boolean(&g, "files", a.files);
        kf.set_boolean(&g, "folders", a.folders);
        kf.set_boolean(&g, "background", a.background);
        kf.set_string(&g, "patterns", &a.patterns);
        kf.set_string(&g, "types", &a.types);
    }
    crate::util::save_settings(&kf, &path());
}

/// The shell custom actions run with (`sh` unless changed in Preferences).
pub fn shell() -> String {
    let s = crate::config::get().action_shell.trim().to_string();
    if s.is_empty() { "sh".into() } else { s }
}

/// Whether `shell` (a name on PATH or a full path) can be run.
pub fn shell_found(shell: &str) -> bool {
    glib::find_program_in_path(shell).is_some()
}

/// File type groups a custom action can be limited to: (id, label).
pub const FILE_TYPES: &[(&str, &str)] = &[
    ("video", "Videos"),
    ("audio", "Audio"),
    ("image", "Images"),
    ("text", "Text files"),
    ("document", "Documents"),
    ("archive", "Archives"),
];

/// Whether content type `ctype` belongs to file type group `kind`.
pub fn type_matches(kind: &str, ctype: &str) -> bool {
    match kind {
        "video" => ctype.starts_with("video/"),
        "audio" => ctype.starts_with("audio/"),
        "image" => ctype.starts_with("image/"),
        // Source code, scripts, config files… are text too.
        "text" => gio::content_type_is_a(ctype, "text/plain"),
        "document" => {
            ctype == "application/pdf"
                || ctype == "application/msword"
                || ctype == "application/rtf"
                || ctype == "application/epub+zip"
                || ctype.starts_with("application/vnd.oasis.opendocument.")
                || ctype.starts_with("application/vnd.openxmlformats-officedocument.")
                || ctype.starts_with("application/vnd.ms-")
        }
        "archive" => crate::util::is_archive(ctype),
        _ => false,
    }
}

/// Whether `a` belongs in the menu for this selection: (name, is_dir,
/// content type) per selected item, or an empty slice for the folder's
/// empty space. A file needs one of the action's file types or a name
/// matching one of its patterns (nothing set: any file); folders only go
/// by the patterns.
pub fn applies(a: &CustomAction, sel: &[(String, bool, String)]) -> bool {
    if sel.is_empty() {
        return a.background;
    }
    let types: Vec<&str> = a.types.split(';').map(str::trim).filter(|t| !t.is_empty()).collect();
    let pats: Vec<Vec<char>> = a
        .patterns
        .split(';')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .map(|p| p.chars().collect())
        .collect();
    sel.iter().all(|(name, is_dir, ctype)| {
        let name: Vec<char> = name.to_lowercase().chars().collect();
        let named = pats.iter().any(|p| crate::util::glob_match(p, &name));
        if *is_dir {
            a.folders && (pats.is_empty() || named)
        } else {
            let typed = types.iter().any(|t| type_matches(t, ctype));
            a.files && ((pats.is_empty() && types.is_empty()) || named || typed)
        }
    })
}

/// Fills in the placeholders, each shell-quoted:
/// %f first selected path, %F all selected paths, %n first selected name,
/// %N all selected names, %d the current folder, %% a literal %.
/// With nothing selected, %f and %F stand for the current folder.
pub fn expand(command: &str, sel: &[PathBuf], dir: &Path) -> String {
    let q = |p: &Path| glib::shell_quote(p).to_string_lossy().to_string();
    let name = |p: &PathBuf| q(Path::new(p.file_name().unwrap_or(p.as_os_str())));
    let dir_only = [dir.to_path_buf()];
    let paths = if sel.is_empty() { &dir_only[..] } else { sel };
    let mut out = String::new();
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('f') => out.push_str(&q(&paths[0])),
            Some('F') => out.push_str(&paths.iter().map(|p| q(p)).collect::<Vec<_>>().join(" ")),
            Some('n') => out.push_str(&name(&paths[0])),
            Some('N') => out.push_str(&paths.iter().map(name).collect::<Vec<_>>().join(" ")),
            Some('d') => out.push_str(&q(dir)),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_placeholders() {
        let dir = Path::new("/home/u/My Dir");
        let sel = [dir.join("a b.txt"), dir.join("c.txt")];
        assert_eq!(expand("echo %f", &sel, dir), "echo '/home/u/My Dir/a b.txt'");
        assert_eq!(expand("x %F", &sel, dir), "x '/home/u/My Dir/a b.txt' '/home/u/My Dir/c.txt'");
        assert_eq!(expand("%n|%N", &sel, dir), "'a b.txt'|'a b.txt' 'c.txt'");
        assert_eq!(expand("cd %d && 100%% %x", &sel, dir), "cd '/home/u/My Dir' && 100% %x");
        assert_eq!(expand("open %f", &[], dir), "open '/home/u/My Dir'");
    }

    #[test]
    fn menu_order_rules() {
        let d: &[&str] = &["a", "b", SEPARATOR, "c", SEPARATOR, "properties"];
        let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // The default, with custom actions as their own group above Properties.
        assert_eq!(menu_order(&[], d, &[]), v(d));
        assert_eq!(menu_order(&[], d, &v(&["custom:1", "custom:2"])), v(&["a", "b", "-", "c", "-", "custom:1", "custom:2", "-", "properties"]));
        // A saved order is kept; gone entries drop out, new ones are added.
        let saved = v(&["custom:1", "b", "custom:gone", "-", "a", "properties", "c"]);
        assert_eq!(menu_order(&saved, d, &v(&["custom:1", "custom:3"])), v(&["custom:1", "b", "-", "a", "-", "custom:3", "-", "properties", "c"]));
        // A new built-in entry goes after the one it follows by default.
        assert_eq!(menu_order(&v(&["c", "-", "a", "properties"]), d, &[]), v(&["c", "-", "a", "b", "properties"]));
        assert_eq!(menu_order(&v(&["c", "properties"]), d, &[]), v(&["c", "a", "b", "properties"]));
    }

    #[test]
    fn saved_and_loaded() {
        let _ = std::fs::remove_file(path());
        assert!(load().is_empty());
        let a = CustomAction {
            id: new_id(),
            name: "Odd; \"name\"\n[x]".into(),
            command: "echo %f \\ 'q' # c".into(),
            files: true,
            folders: false,
            background: true,
            patterns: "*.jpg; *.PNG".into(),
            types: "video;audio".into(),
        };
        let b = CustomAction { id: new_id(), name: "B".into(), command: "b".into(), ..Default::default() };
        save(&[a.clone(), b.clone()]);
        assert_eq!(load(), [a, b]);
        // Ones without a name or command are left out; one without an id gets one.
        std::fs::write(path(), "[Action 0]\nname=x\ncommand=\n[Action 1]\nname=y\ncommand=y\n").unwrap();
        let l = load();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].name.as_str(), l[0].id.len()), ("y", 12));
        assert_ne!(new_id(), new_id());
        let _ = std::fs::remove_file(path());
    }

    #[test]
    fn applies_rules() {
        let f = |n: &str, ct: &str| (n.to_string(), false, ct.to_string());
        let dir = |n: &str| (n.to_string(), true, "inode/directory".to_string());
        let a = CustomAction { name: "x".into(), command: "x".into(), files: true, patterns: "*.jpg; *.PNG".into(), ..Default::default() };
        assert!(applies(&a, &[f("Pic.JPG", "image/jpeg"), f("b.png", "image/png")]));
        assert!(!applies(&a, &[f("Pic.jpg", "image/jpeg"), f("notes.txt", "text/plain")]));
        assert!(!applies(&a, &[dir("dir.jpg")]));
        assert!(!applies(&a, &[]));
        let bg = CustomAction { background: true, folders: true, ..a.clone() };
        assert!(applies(&bg, &[]));
        let any = CustomAction { patterns: String::new(), files: true, folders: true, ..a.clone() };
        assert!(applies(&any, &[dir("dir"), f("f", "text/plain")]));
        // File types, alone or together with patterns (either matches).
        let video = CustomAction { patterns: String::new(), types: "video".into(), ..a.clone() };
        assert!(applies(&video, &[f("a.mkv", "video/x-matroska"), f("b.mp4", "video/mp4")]));
        assert!(!applies(&video, &[f("a.mkv", "video/x-matroska"), f("a.srt", "application/x-subrip")]));
        let both = CustomAction { patterns: "*.srt".into(), types: "video;audio".into(), ..a };
        assert!(applies(&both, &[f("a.mkv", "video/x-matroska"), f("a.srt", "application/x-subrip"), f("s.flac", "audio/flac")]));
        assert!(!applies(&both, &[f("c.jpg", "image/jpeg")]));
    }

    #[test]
    fn random_commands_and_orders() {
        crate::testutil::cases(2000, |r| {
            let cmd = r.text(20);
            let sel: Vec<PathBuf> = (0..r.below(3)).map(|_| Path::new("/d").join(r.text(6))).collect();
            let out = expand(&cmd, &sel, Path::new("/d"));
            if !cmd.contains('%') {
                assert_eq!(out, cmd);
            }
            let _ = crate::thumbnailers::expand(&cmd, Path::new("/in"), Path::new("/out"), 128);
            // Every default entry once, whatever was saved.
            let d: &[&str] = &["a", "b", SEPARATOR, "c", "properties"];
            let saved: Vec<String> = (0..r.below(8)).map(|_| r.pick(&["a", "b", "c", "-", "x", "custom:1", "properties"]).to_string()).collect();
            let order = menu_order(&saved, d, &["custom:1".into()]);
            for k in ["a", "b", "c", "properties", "custom:1"] {
                assert_eq!(order.iter().filter(|o| *o == k).count(), 1, "{k} in {order:?} from {saved:?}");
            }
            assert!(!order.contains(&"x".to_string()));
        });
    }
}
