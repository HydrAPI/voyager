//! View, sorting and the info panel remembered for individual folders, in
//! ~/.config/arkha/folders.conf (one group per folder URI). Used when the
//! "remember view and sorting for each folder" preference is on.

use std::cell::RefCell;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderView {
    pub view: Option<String>,
    pub sort: Option<(String, bool)>,
    /// The info panel (see info.rs) shown in this folder.
    pub info_panel: Option<bool>,
}

thread_local! {
    static STORE: RefCell<Option<glib::KeyFile>> = const { RefCell::new(None) };
}

fn path() -> std::path::PathBuf {
    glib::user_config_dir().join("arkha").join("folders.conf")
}

fn with_store<R>(f: impl FnOnce(&glib::KeyFile) -> R) -> R {
    STORE.with(|s| {
        let mut s = s.borrow_mut();
        let kf = s.get_or_insert_with(|| {
            let kf = glib::KeyFile::new();
            let _ = kf.load_from_file(path(), glib::KeyFileFlags::NONE);
            kf
        });
        f(kf)
    })
}

fn save(kf: &glib::KeyFile) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = kf.save_to_file(p);
}

pub fn get(uri: &str) -> FolderView {
    with_store(|kf| {
        let view = kf.string(uri, "view").ok().map(|v| v.to_string());
        let sort = kf
            .string(uri, "sort_by")
            .ok()
            .map(|by| (by.to_string(), kf.boolean(uri, "sort_desc").unwrap_or(false)));
        let info_panel = kf.boolean(uri, "info_panel").ok();
        FolderView { view, sort, info_panel }
    })
}

pub fn set_view(uri: &str, view: &str) {
    with_store(|kf| {
        kf.set_string(uri, "view", view);
        save(kf);
    })
}

pub fn set_sort(uri: &str, by: &str, desc: bool) {
    with_store(|kf| {
        kf.set_string(uri, "sort_by", by);
        kf.set_boolean(uri, "sort_desc", desc);
        save(kf);
    })
}

pub fn set_info_panel(uri: &str, on: bool) {
    with_store(|kf| {
        kf.set_boolean(uri, "info_panel", on);
        save(kf);
    })
}

/// Number of folders with remembered settings.
pub fn count() -> usize {
    with_store(|kf| kf.groups().len())
}

pub fn forget_all() {
    with_store(|kf| {
        for g in kf.groups().iter() {
            let _ = kf.remove_group(g);
        }
        save(kf);
    })
}
