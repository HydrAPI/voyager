//! What can be shown about a file besides its name, as Caja has it: the
//! list view's columns (chosen and ordered in Preferences or with a right
//! click on the column headers), the sort orders, and the captions under
//! the names in the icon view. Each comes from the pane's model (pane.rs),
//! which holds the values; the texts are made only for what's shown.

use std::cmp::Ordering;

use gtk::prelude::*;

use crate::pane::{value_str, ModelGet, COL_ALLOC, COL_ATIME, COL_BTIME, COL_CTYPE, COL_GROUP, COL_IS_DIR, COL_MODE, COL_MTIME, COL_MTIME_TEXT, COL_NAME, COL_ORIG, COL_OWNER, COL_SIZE, COL_SIZE_TEXT, COL_TRASHED, COL_TYPE, COL_URI};
use crate::util;

pub struct Detail {
    pub id: &'static str,
    /// The list view column's title (and the caption's name).
    pub title: &'static str,
    /// In View → Arrange Items, if it's there.
    pub menu: Option<&'static str>,
    pub xalign: f32,
    pub min_width: i32,
    /// Sorted biggest or newest first when its column header is clicked.
    pub desc_first: bool,
    /// Only in the Trash (its column shows there by itself).
    pub trash: bool,
}

const fn d(id: &'static str, title: &'static str, menu: Option<&'static str>, xalign: f32, min_width: i32, desc_first: bool) -> Detail {
    Detail { id, title, menu, xalign, min_width, desc_first, trash: false }
}

/// Everything but the name, in the order the column chooser lists them. A
/// detail's sort id (see sort_id) is its place here plus one, the name's
/// is 0: the first three were Arkha's only ones, and stay as they were.
pub const DETAILS: &[Detail] = &[
    d("size", "Size", Some("By _Size"), 1.0, 90, true),
    d("type", "Type", Some("By _Type"), 0.0, 130, false),
    d("mtime", "Modified", Some("By Modification _Date"), 0.0, 150, true),
    d("atime", "Accessed", Some("By _Access Date"), 0.0, 150, true),
    d("btime", "Created", Some("By _Creation Date"), 0.0, 150, true),
    d("owner", "Owner", Some("By _Owner"), 0.0, 90, false),
    d("group", "Group", Some("By _Group"), 0.0, 90, false),
    d("permissions", "Permissions", Some("By _Permissions"), 0.0, 110, false),
    d("octal", "Octal Permissions", None, 1.0, 60, false),
    d("mime", "MIME Type", Some("By _MIME Type"), 0.0, 140, false),
    d("extension", "Extension", Some("By _Extension"), 0.0, 70, false),
    d("location", "Location", Some("By _Location"), 0.0, 160, false),
    d("disk", "Size on Disk", Some("By Size on Dis_k"), 1.0, 90, true),
    Detail { id: "trashed", title: "Trashed On", menu: None, xalign: 0.0, min_width: 150, desc_first: true, trash: true },
    Detail { id: "orig", title: "Original Location", menu: None, xalign: 0.0, min_width: 160, desc_first: false, trash: true },
];

/// The list view's columns until chosen otherwise (Arkha's from the start).
pub const DEFAULT_COLUMNS: &[&str] = &["size", "type", "mtime"];
/// The icon captions until chosen otherwise, as in Caja: more show as the
/// icons get bigger (see shown_captions), "none" keeps a place empty.
pub const DEFAULT_CAPTIONS: &[&str] = &["none", "size", "mtime"];

pub fn get(id: &str) -> Option<&'static Detail> {
    DETAILS.iter().find(|d| d.id == id)
}

/// The details that can be columns or captions, outside the Trash.
pub fn choosable() -> impl Iterator<Item = &'static Detail> {
    DETAILS.iter().filter(|d| !d.trash)
}

/// How many sort orders there are (the name's and each detail's).
pub fn sort_count() -> u32 {
    DETAILS.len() as u32 + 1
}

pub fn sort_id(by: &str) -> u32 {
    DETAILS.iter().position(|d| d.id == by).map_or(0, |i| i as u32 + 1)
}

pub fn sort_name(id: u32) -> &'static str {
    id.checked_sub(1).and_then(|i| DETAILS.get(i as usize)).map_or("name", |d| d.id)
}

/// A setting's list of details, those Arkha knows each once (`choosable`
/// ones only), in the order given.
pub fn known(ids: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    for id in ids {
        if let Some(d) = choosable().find(|d| d.id == id) {
            if !out.contains(&d.id) {
                out.push(d.id);
            }
        }
    }
    out
}

/// The captions shown under names with icons of `size` pixels: as in
/// Caja, none below its standard size (48), then one, two (72 on) and all
/// three (192 on), "none" ones as gaps left out.
pub fn shown_captions(captions: &[String], size: i32) -> Vec<&'static str> {
    let n = match size {
        ..=47 => 0,
        48..=71 => 1,
        72..=191 => 2,
        _ => 3,
    };
    captions.iter().take(n).filter_map(|c| choosable().find(|d| d.id == c)).map(|d| d.id).collect()
}

/// The file attribute `id` needs beyond those always read (see pane.rs).
fn attribute(id: &str) -> Option<&'static str> {
    match id {
        "atime" => Some("time::access"),
        "btime" => Some("time::created"),
        "owner" => Some("owner::user"),
        "group" => Some("owner::group"),
        "permissions" | "octal" => Some("unix::mode"),
        "disk" => Some("standard::allocated-size"),
        _ => None,
    }
}

/// What to add to the attributes read for each item (",owner::user,…") for
/// details `ids`: nothing for the usual ones, so a folder shown as before
/// batch 3 costs what it did.
pub fn attributes(ids: &[&str]) -> String {
    let mut out = String::new();
    for a in ids.iter().filter_map(|id| attribute(id)) {
        if !out.split(',').any(|x| x == a) {
            out.push(',');
            out.push_str(a);
        }
    }
    out
}

/// Whether attributes `wanted` (as from `attributes`) are all in `read`.
pub fn attributes_read(wanted: &str, read: &str) -> bool {
    wanted.split(',').filter(|a| !a.is_empty()).all(|a| read.split(',').any(|r| r == a))
}

/// "drwxr-xr-x", as ls and Caja show a mode; empty if it isn't known.
pub fn permissions_text(mode: u32) -> String {
    if mode == 0 {
        return String::new();
    }
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o020000 => 'c',
        0o060000 => 'b',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    // Set-ID and sticky bits take the place of x: lowercase with x set.
    let special = |x: u32, s: u32, set: char| match (mode & x != 0, mode & s != 0) {
        (true, true) => set,
        (false, true) => set.to_ascii_uppercase(),
        (true, false) => 'x',
        (false, false) => '-',
    };
    [
        kind,
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        special(0o100, 0o4000, 's'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        special(0o010, 0o2000, 's'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        special(0o001, 0o1000, 't'),
    ]
    .iter()
    .collect()
}

/// "755" (or "4755" with special bits), as Caja shows it; empty if unknown.
pub fn octal_text(mode: u32) -> String {
    if mode == 0 {
        String::new()
    } else {
        format!("{:o}", mode & 0o7777)
    }
}

/// A file name's extension, as Caja tells it: "gz", or "tar.gz" for a
/// compressed tarball; none for folders and names starting with the dot.
pub fn extension(name: &str, is_dir: bool) -> &str {
    if is_dir {
        return "";
    }
    let body = name.strip_prefix('.').unwrap_or(name);
    let offset = name.len() - body.len();
    let Some(dot) = body.rfind('.') else { return "" };
    if dot == 0 || dot + 1 == body.len() {
        return "";
    }
    let start = match body[..dot].rfind('.') {
        Some(p) if body[p + 1..dot].eq_ignore_ascii_case("tar") && p > 0 => p,
        _ => dot,
    };
    &name[offset + start + 1..]
}

/// The folder an item is in, as a path (or URI where there's none).
pub fn location(uri: &str) -> String {
    let parent = gio::File::for_uri(uri).parent();
    parent.map(|p| p.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| p.uri().to_string())).unwrap_or_default()
}

/// The folder part of a URI ("file:///a" of "file:///a/b").
fn folder_part(uri: &str) -> &str {
    uri.trim_end_matches('/').rsplit_once('/').map_or("", |(p, _)| p)
}

fn time_text(t: i64) -> String {
    if t > 0 {
        util::format_time(t)
    } else {
        String::new()
    }
}

fn int<T: for<'b> glib::value::FromValue<'b> + 'static>(m: &gtk::TreeModel, it: &gtk::TreeIter, col: u32) -> T {
    m.gv(it, col as i32)
}

/// What `id` shows for a row.
pub fn text(id: &str, m: &gtk::TreeModel, it: &gtk::TreeIter) -> String {
    let s = |col: u32| value_str(&m.value(it, col as i32)).to_string();
    match id {
        "size" => s(COL_SIZE_TEXT),
        "type" => s(COL_TYPE),
        "mtime" => s(COL_MTIME_TEXT),
        "atime" => time_text(int(m, it, COL_ATIME)),
        "btime" => time_text(int(m, it, COL_BTIME)),
        "trashed" => time_text(int(m, it, COL_TRASHED)),
        "owner" => s(COL_OWNER),
        "group" => s(COL_GROUP),
        "permissions" => permissions_text(int(m, it, COL_MODE)),
        "octal" => octal_text(int(m, it, COL_MODE)),
        "mime" => s(COL_CTYPE),
        "extension" => extension(value_str(&m.value(it, COL_NAME as i32)), int(m, it, COL_IS_DIR)).to_string(),
        "location" => location(value_str(&m.value(it, COL_URI as i32))),
        "disk" if !int::<bool>(m, it, COL_IS_DIR) => util::format_size(int(m, it, COL_ALLOC)),
        "orig" => s(COL_ORIG),
        _ => String::new(),
    }
}

/// How two rows compare by `id` (the caller breaks ties by name).
pub fn cmp(id: &str, m: &gtk::TreeModel, a: &gtk::TreeIter, b: &gtk::TreeIter) -> Ordering {
    let num = |col: u32| int::<i64>(m, a, col).cmp(&int::<i64>(m, b, col));
    let unum = |col: u32| int::<u64>(m, a, col).cmp(&int::<u64>(m, b, col));
    let text_of = |col: u32| {
        let (va, vb) = (m.value(a, col as i32), m.value(b, col as i32));
        value_str(&va).to_lowercase().cmp(&value_str(&vb).to_lowercase())
    };
    match id {
        "size" => unum(COL_SIZE),
        "type" => text_of(COL_TYPE),
        "mtime" => num(COL_MTIME),
        "atime" => num(COL_ATIME),
        "btime" => num(COL_BTIME),
        "trashed" => num(COL_TRASHED),
        "owner" => text_of(COL_OWNER),
        "group" => text_of(COL_GROUP),
        "permissions" | "octal" => int::<u32>(m, a, COL_MODE).cmp(&int::<u32>(m, b, COL_MODE)),
        "mime" => text_of(COL_CTYPE),
        "extension" => text("extension", m, a).to_lowercase().cmp(&text("extension", m, b).to_lowercase()),
        // By the folder's URI: no need to make paths for a comparison.
        "location" => folder_part(value_str(&m.value(a, COL_URI as i32))).cmp(folder_part(value_str(&m.value(b, COL_URI as i32)))),
        "disk" => unum(COL_ALLOC),
        "orig" => text_of(COL_ORIG),
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_ids_keep_the_old_ones() {
        assert_eq!([sort_id("name"), sort_id("size"), sort_id("type"), sort_id("mtime")], [0, 1, 2, 3]);
        for id in 0..sort_count() {
            assert_eq!(sort_id(sort_name(id)), id);
        }
        assert_eq!(sort_id("bogus"), 0);
        assert_eq!(sort_name(999), "name");
    }

    #[test]
    fn modes_as_text() {
        assert_eq!(permissions_text(0o040755), "drwxr-xr-x");
        assert_eq!(permissions_text(0o100644), "-rw-r--r--");
        assert_eq!(permissions_text(0o104755), "-rwsr-xr-x");
        assert_eq!(permissions_text(0o102644), "-rw-r-Sr--");
        assert_eq!(permissions_text(0o041777), "drwxrwxrwt");
        assert_eq!(permissions_text(0o120777), "lrwxrwxrwx");
        assert_eq!(permissions_text(0), "");
        assert_eq!(octal_text(0o040755), "755");
        assert_eq!(octal_text(0o104755), "4755");
        assert_eq!(octal_text(0), "");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("photo.JPG", false), "JPG");
        assert_eq!(extension("backup.tar.gz", false), "tar.gz");
        assert_eq!(extension("a.b.gz", false), "gz");
        assert_eq!(extension(".bashrc", false), "");
        assert_eq!(extension(".config.toml", false), "toml");
        assert_eq!(extension("README", false), "");
        assert_eq!(extension("odd.", false), "");
        assert_eq!(extension("tar.gz", false), "gz");
        assert_eq!(extension("photos.d", true), "");
    }

    #[test]
    fn locations() {
        assert_eq!(location("file:///home/user/a%20b.txt"), "/home/user");
        assert_eq!(location("file:///x"), "/");
    }

    #[test]
    fn captions_by_icon_size() {
        let c: Vec<String> = DEFAULT_CAPTIONS.iter().map(|s| s.to_string()).collect();
        // Caja's defaults: nothing until 72 px, the size, then the date too.
        assert!(shown_captions(&c, 48).is_empty());
        assert_eq!(shown_captions(&c, 72), ["size"]);
        assert_eq!(shown_captions(&c, 192), ["size", "mtime"]);
        let mine: Vec<String> = ["owner", "none", "bogus"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shown_captions(&mine, 32), Vec::<&str>::new());
        assert_eq!(shown_captions(&mine, 48), ["owner"]);
        assert_eq!(shown_captions(&mine, 256), ["owner"]);
    }

    #[test]
    fn attributes_only_for_details_that_need_them() {
        assert_eq!(attributes(&["size", "type", "mtime", "name", "mime", "extension", "location", "trashed", "orig"]), "");
        assert_eq!(attributes(&["owner", "permissions", "octal", "owner", "disk"]), ",owner::user,unix::mode,standard::allocated-size");
        assert!(attributes_read("", ""));
        assert!(attributes_read(",unix::mode", ",owner::user,unix::mode"));
        assert!(!attributes_read(",unix::mode,time::access", ",unix::mode"));
    }

    #[test]
    fn known_ids_once_each() {
        let ids: Vec<String> = ["type", "bogus", "size", "type", "orig", "owner"].iter().map(|s| s.to_string()).collect();
        assert_eq!(known(&ids), ["type", "size", "owner"]);
    }
}
