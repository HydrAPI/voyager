//! The info panel beside a pane's file view. For a folder it shows its
//! cover picture (a file like folder.jpg in it; the names are a setting)
//! or its icon, for a file the picture itself (images), its thumbnail
//! (videos and the like) or its icon, with a few details under it; for
//! several items a summary. Icons set in Caja's Properties (GVfs metadata,
//! so any program can read them) are used instead of the theme's. Which
//! folders show the panel is remembered like their view, and what it shows
//! for which selection is decided in pane.rs.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gio::prelude::*;
use gtk::prelude::*;

use crate::{config, icons, util};

/// What to show for a folder without a cover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Missing {
    /// Its icon with the details.
    Icon,
    /// Nothing at all.
    Nothing,
}

enum Msg {
    /// `remote`: on a network share, where adding up its size would mean a
    /// request for every file in it, so it's left out.
    Found { cover: Option<PathBuf>, custom: Option<Custom>, items: usize, mtime: i64, remote: bool },
    Size(u64),
}

/// A custom icon set for an item in Caja (and GVfs-based file managers).
enum Custom {
    /// metadata::custom-icon: a picture file.
    Picture(PathBuf),
    /// metadata::custom-icon-name: an icon from the theme.
    Name(String),
}

const CUSTOM_ATTRS: &str = "metadata::custom-icon,metadata::custom-icon-name";

fn custom_icon(info: &gio::FileInfo) -> Option<Custom> {
    if let Some(uri) = info.attribute_string("metadata::custom-icon").filter(|u| !u.is_empty()) {
        // Usually a file: URI; older Nautilus versions stored a path.
        let f = if uri.contains("://") { gio::File::for_uri(&uri) } else { gio::File::for_path(uri.as_str()) };
        if let Some(p) = f.path().filter(|p| p.is_file()) {
            return Some(Custom::Picture(p));
        }
    }
    info.attribute_string("metadata::custom-icon-name").filter(|n| !n.is_empty()).map(|n| Custom::Name(n.to_string()))
}

pub struct InfoPanel {
    pub widget: gtk::ScrolledWindow,
    image: gtk::Image,
    name: gtk::Label,
    details: gtk::Grid,
    /// The value labels of the details shown, by their key ("Size", …).
    values: RefCell<Vec<(&'static str, gtk::Label)>>,
    /// What's shown (URI and how a missing cover is handled), so repeated
    /// requests for the same item do nothing.
    shown: RefCell<Option<(String, Missing)>>,
    /// A file whose thumbnail is still being made: shown once it's ready.
    awaiting_thumb: RefCell<Option<String>>,
    /// Bumped for every new item: background work for an older one stops.
    gen: Arc<AtomicU64>,
}

impl InfoPanel {
    pub fn new() -> Rc<Self> {
        let image = gtk::Image::new();
        image.set_halign(gtk::Align::Center);
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_line_wrap(true);
        name.set_line_wrap_mode(pango::WrapMode::WordChar);
        name.set_selectable(true);
        name.set_can_focus(false);
        let details = gtk::Grid::new();
        details.set_row_spacing(3);
        details.set_column_spacing(10);
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        b.set_border_width(12);
        b.add(&image);
        b.add(&name);
        b.add(&details);
        let widget = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        widget.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        widget.set_size_request(120, -1);
        widget.add(&b);
        widget.show_all();
        // The pane decides when it's visible (its show_all stays out).
        widget.hide();
        widget.set_no_show_all(true);
        let panel = Rc::new(InfoPanel {
            widget,
            image,
            name,
            details,
            values: RefCell::new(vec![]),
            shown: RefCell::new(None),
            awaiting_thumb: RefCell::new(None),
            gen: Arc::new(AtomicU64::new(0)),
        });
        panel.blank();
        // A thumbnail made while its file is shown replaces the icon.
        let w = Rc::downgrade(&panel);
        let id = icons::on_thumbnail_ready(move |uri, thumb| {
            let Some(p) = w.upgrade() else { return };
            if p.awaiting_thumb.borrow().as_deref() == Some(uri) {
                p.awaiting_thumb.borrow_mut().take();
                let gen = p.gen.load(Ordering::SeqCst);
                p.load_image(Path::new(thumb), gen, false);
            }
        });
        panel.widget.connect_destroy(move |_| icons::remove_thumbnail_listener(id));
        panel
    }

    /// Forgets what's shown, so the next request loads again (after the
    /// settings or the panel's width changed).
    pub fn reset(&self) {
        *self.shown.borrow_mut() = None;
    }

    fn blank(&self) {
        self.image.clear();
        self.image.hide();
        self.name.hide();
        self.details.hide();
        self.awaiting_thumb.borrow_mut().take();
    }

    /// Starts showing a new item: returns its generation, or None if it's
    /// already shown.
    fn start(&self, uri: &str, missing: Missing) -> Option<u64> {
        let key = (uri.to_string(), missing);
        if self.shown.borrow().as_ref() == Some(&key) {
            return None;
        }
        *self.shown.borrow_mut() = Some(key);
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.blank();
        Some(gen)
    }

    fn set_name(&self, name: &str) {
        self.name.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(name)));
        self.name.show();
    }

    /// Replaces the details with `rows` (key, value).
    fn set_details(&self, rows: &[(&'static str, String)]) {
        for c in self.details.children() {
            self.details.remove(&c);
        }
        let mut values = vec![];
        for (i, (key, value)) in rows.iter().enumerate() {
            let k = gtk::Label::new(Some(key));
            k.set_xalign(0.0);
            k.set_valign(gtk::Align::Start);
            k.style_context().add_class("dim-label");
            let v = gtk::Label::new(Some(value));
            v.set_xalign(0.0);
            v.set_line_wrap(true);
            v.set_line_wrap_mode(pango::WrapMode::WordChar);
            self.details.attach(&k, 0, i as i32, 1, 1);
            self.details.attach(&v, 1, i as i32, 1, 1);
            values.push((*key, v));
        }
        *self.values.borrow_mut() = values;
        self.details.show_all();
        self.details.set_visible(config::with(|s| s.info_details));
    }

    fn set_value(&self, key: &str, value: &str) {
        if let Some((_, l)) = self.values.borrow().iter().find(|(k, _)| *k == key) {
            l.set_text(value);
        }
    }

    /// A custom icon (Caja's), else `icon`, else a folder icon.
    fn show_custom_or_icon(self: &Rc<Self>, custom: Option<Custom>, icon: Option<&gio::Icon>, gen: u64) {
        match custom {
            Some(Custom::Picture(p)) => self.load_image(&p, gen, false),
            Some(Custom::Name(n)) => self.show_icon(Some(&gio::ThemedIcon::new(&n).upcast())),
            None => self.show_icon(icon),
        }
    }

    fn show_icon(&self, icon: Option<&gio::Icon>) {
        match icon {
            Some(i) => self.image.set_from_gicon(i, gtk::IconSize::Dialog),
            None => self.image.set_from_icon_name(Some("folder"), gtk::IconSize::Dialog),
        }
        self.image.set_pixel_size(128);
        self.image.show();
    }

    /// Shows `dir`'s cover (or icon) and details. Covers are only looked
    /// for in local folders: elsewhere (phones, network shares) that would
    /// make browsing slow.
    pub fn show_folder(self: &Rc<Self>, dir: &gio::File, name: &str, missing: Missing) {
        let Some(gen) = self.start(&dir.uri(), missing) else { return };
        let Some(path) = dir.path() else {
            if missing == Missing::Icon {
                self.show_icon(None);
                self.set_name(name);
            }
            return;
        };
        let names: Vec<String> = config::get()
            .cover_names
            .split(';')
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let current = self.gen.clone();
        std::thread::spawn(move || {
            let (cover, items, mtime) = look(&path, &names);
            let custom = gio::File::for_path(&path)
                .query_info(CUSTOM_ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
                .ok()
                .and_then(|i| custom_icon(&i));
            let remote = is_remote(&path);
            if tx.send_blocking(Msg::Found { cover, custom, items, mtime, remote }).is_err() || remote {
                return;
            }
            if let Some(bytes) = du(&path, gen, &current) {
                let _ = tx.send_blocking(Msg::Size(bytes));
            }
        });
        let w = Rc::downgrade(self);
        let name = name.to_string();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(p) = w.upgrade() else { return };
                if p.gen.load(Ordering::SeqCst) != gen {
                    return;
                }
                match msg {
                    Msg::Found { cover, custom, items, mtime, remote } => {
                        if cover.is_none() && custom.is_none() && missing == Missing::Nothing {
                            continue;
                        }
                        p.set_name(&name);
                        let contents = if items == 1 { "1 item".to_string() } else { format!("{items} items") };
                        let modified = ("Modified", util::format_time(mtime));
                        if remote {
                            p.set_details(&[("Contents", contents), modified]);
                        } else {
                            p.set_details(&[("Contents", contents), ("Size", "…".into()), modified]);
                        }
                        match cover {
                            Some(file) => p.load_image(&file, gen, false),
                            None => p.show_custom_or_icon(custom, None, gen),
                        }
                    }
                    Msg::Size(bytes) => p.set_value("Size", &util::format_size(bytes)),
                }
            }
        });
    }

    /// Shows a file: the picture itself for images (local ones), else its
    /// thumbnail if there is one (or once it's made), else its icon; with
    /// its type, size, date and, for pictures, dimensions.
    pub fn show_file(self: &Rc<Self>, file: &gio::File, name: &str) {
        let Some(gen) = self.start(&file.uri(), Missing::Icon) else { return };
        self.set_name(name);
        let w = Rc::downgrade(self);
        let file = file.clone();
        glib::MainContext::default().spawn_local(async move {
            let attrs = format!("standard::content-type,standard::fast-content-type,standard::size,standard::icon,time::modified,thumbnail::path,thumbnail::is-valid,{CUSTOM_ATTRS}");
            let info = file.query_info_future(&attrs, gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await;
            let Some(p) = w.upgrade() else { return };
            if p.gen.load(Ordering::SeqCst) != gen {
                return;
            }
            let Ok(info) = info else { return };
            let ctype = util::info_content_type(&info).unwrap_or_default();
            let mtime = info.attribute_uint64("time::modified") as i64;
            let mut rows: Vec<(&'static str, String)> = vec![
                ("Type", crate::pane::type_description(&ctype, false)),
                ("Size", util::format_size(info.size().max(0) as u64)),
                ("Modified", util::format_time(mtime)),
            ];
            let path = file.path();
            let is_image = ctype.starts_with("image/") && path.is_some();
            if let Some(path) = path.as_ref().filter(|_| is_image) {
                if let Some((_, w, h)) = gdk_pixbuf::Pixbuf::file_info(path) {
                    rows.push(("Dimensions", format!("{w} × {h}")));
                }
            }
            p.set_details(&rows);
            let uri = file.uri().to_string();
            if is_image && gdk_pixbuf::Pixbuf::file_info(path.as_ref().unwrap()).is_some() {
                p.load_image(path.as_ref().unwrap(), gen, true);
            } else if let Some(thumb) = icons::existing_thumbnail(&info, &uri, mtime) {
                p.load_image(Path::new(&thumb), gen, false);
            } else {
                p.show_custom_or_icon(custom_icon(&info), info.icon().as_ref(), gen);
                // Once it's made, the thumbnail replaces the icon.
                if config::with(|s| s.thumbnails) && icons::can_thumbnail(&ctype) {
                    if let Some(path) = &path {
                        icons::request_thumbnail(&uri, path, mtime, info.size().max(0) as u64, &ctype);
                    }
                    *p.awaiting_thumb.borrow_mut() = Some(uri);
                }
            }
        });
    }

    /// Shows a summary of several items (uri, is a folder, size): how many
    /// folders and files, and their total size, folders added up in the
    /// background (local ones only, like covers).
    pub fn show_summary(self: &Rc<Self>, items: &[(String, bool, u64)]) {
        let key = format!("summary\n{}", items.iter().map(|i| i.0.as_str()).collect::<Vec<_>>().join("\n"));
        let Some(gen) = self.start(&key, Missing::Icon) else { return };
        let n_dirs = items.iter().filter(|i| i.1).count();
        let n_files = items.len() - n_dirs;
        let file_bytes: u64 = items.iter().filter(|i| !i.1).map(|i| i.2).sum();
        let dirs: Vec<PathBuf> = items.iter().filter(|i| i.1).filter_map(|i| gio::File::for_uri(&i.0).path()).collect();
        let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let contents = match (n_dirs, n_files) {
            (0, f) => count(f, "file", "files"),
            (d, 0) => count(d, "folder", "folders"),
            (d, f) => format!("{}, {}", count(d, "folder", "folders"), count(f, "file", "files")),
        };
        self.set_name(&format!("{} items selected", items.len()));
        let size = if dirs.is_empty() { util::format_size(file_bytes) } else { "…".into() };
        self.set_details(&[("Contents", contents), ("Size", size)]);
        self.show_icon(Some(&gio::ThemedIcon::from_names(&["emblem-documents", "text-x-generic"]).upcast()));
        if dirs.is_empty() {
            return;
        }
        // Whether any is on a network share (then the size is left out, see
        // Msg::Found) is asked off the main thread too.
        let (tx, rx) = async_channel::bounded::<bool>(1);
        {
            let dirs = dirs.clone();
            std::thread::spawn(move || {
                let _ = tx.send_blocking(dirs.iter().any(|d| is_remote(d)));
            });
        }
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Ok(remote) = rx.recv().await else { return };
            let Some(p) = w.upgrade() else { return };
            if p.gen.load(Ordering::SeqCst) != gen {
                return;
            }
            if remote {
                p.remove_detail("Size");
            } else {
                p.total_size(gen, dirs, file_bytes);
            }
        });
    }

    /// Adds up the size of `dirs` (and `bytes`) in the background and
    /// shows it, unless another item is shown meanwhile.
    fn total_size(self: &Rc<Self>, gen: u64, dirs: Vec<PathBuf>, bytes: u64) {
        let (tx, rx) = async_channel::bounded::<u64>(1);
        let current = self.gen.clone();
        std::thread::spawn(move || {
            let mut total = bytes;
            for d in &dirs {
                match du(d, gen, &current) {
                    Some(b) => total += b,
                    None => return,
                }
            }
            let _ = tx.send_blocking(total);
        });
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Ok(total) = rx.recv().await else { return };
            let Some(p) = w.upgrade() else { return };
            if p.gen.load(Ordering::SeqCst) == gen {
                p.set_value("Size", &util::format_size(total));
            }
        });
    }

    /// Takes the row `key` out of the details.
    fn remove_detail(&self, key: &str) {
        let rows: Vec<(&'static str, String)> = self
            .values
            .borrow()
            .iter()
            .filter(|(k, _)| *k != key)
            .map(|(k, l)| (*k, l.text().to_string()))
            .collect();
        self.set_details(&rows);
    }

    /// Loads a picture to fit the panel's width (no larger than it is),
    /// off the main thread. `orient`: turn photos the way their camera
    /// recorded (thumbnails are already upright).
    fn load_image(self: &Rc<Self>, file: &Path, gen: u64, orient: bool) {
        let alloc = self.widget.allocated_width();
        let width = (if alloc > 1 { alloc } else { config::with(|s| s.info_panel_width) } - 24).max(64);
        let natural = gdk_pixbuf::Pixbuf::file_info(file).map(|(_, w, _)| w).unwrap_or(width);
        let width = width.min(natural.max(1));
        let f = gio::File::for_path(file);
        let w = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Ok(stream) = f.read_future(glib::Priority::DEFAULT).await else { return };
            // Very tall pictures are kept to one and a half times the width.
            let res = gdk_pixbuf::Pixbuf::from_stream_at_scale_future(&stream, width, width * 3 / 2, true).await;
            let Some(p) = w.upgrade() else { return };
            if p.gen.load(Ordering::SeqCst) != gen {
                return;
            }
            if let Ok(pix) = res {
                let pix = if orient { pix.apply_embedded_orientation().unwrap_or(pix) } else { pix };
                p.image.set_pixel_size(-1);
                p.image.set_from_pixbuf(Some(&pix));
                p.image.show();
            }
        });
    }
}

/// The cover in `dir` (the first of `names` there, ignoring case), the
/// number of items (hidden ones left out) and the folder's modification
/// time.
fn look(dir: &Path, names: &[String]) -> (Option<PathBuf>, usize, i64) {
    let mut found: Vec<(usize, PathBuf)> = vec![];
    let mut items = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                items += 1;
            }
            if let Some(rank) = names.iter().position(|n| *n == name.to_lowercase()) {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    found.push((rank, e.path()));
                }
            }
        }
    }
    found.sort();
    let cover = found.into_iter().next().map(|(_, p)| p);
    let mtime = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    (cover, items, mtime)
}

/// Whether `path` is on a network share (SMB, NFS, SSHFS, …).
fn is_remote(path: &Path) -> bool {
    gio::File::for_path(path)
        .query_filesystem_info("filesystem::remote", gio::Cancellable::NONE)
        .is_ok_and(|i| i.boolean("filesystem::remote"))
}

/// The total size of everything in `dir` (links not followed), or None if
/// another item was asked for meanwhile.
fn du(dir: &Path, gen: u64, current: &AtomicU64) -> Option<u64> {
    let mut total = 0u64;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        if current.load(Ordering::SeqCst) != gen {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                todo.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    #[test]
    fn cover_and_items() {
        let t = TestDir::new("cover");
        for f in ["Cover.JPG", "folder.png", ".hidden", "song.mp3"] {
            t.file(f, "x");
        }
        // A folder with a cover's name isn't one.
        std::fs::create_dir(t.0.join("folder.jpg")).unwrap();
        let names: Vec<String> = ["folder.jpg", "folder.png", "cover.jpg"].map(String::from).to_vec();
        let (cover, items, mtime) = look(&t.0, &names);
        assert_eq!(cover, Some(t.0.join("folder.png")));
        assert_eq!(items, 4);
        assert!(mtime > 0);
        // Upper and lower case don't matter.
        std::fs::remove_file(t.0.join("folder.png")).unwrap();
        assert_eq!(look(&t.0, &names).0, Some(t.0.join("Cover.JPG")));
        assert_eq!(look(&t.0, &[]).0, None);
        assert_eq!(look(&t.0.join("missing"), &names), (None, 0, 0));
    }

    #[test]
    fn folder_size() {
        let t = TestDir::new("du");
        t.file("a", "abc");
        t.file("sub/b", "12345");
        t.file("sub/deeper/c", "1234567");
        // Links aren't followed (this one would loop).
        std::os::unix::fs::symlink("..", t.0.join("sub/up")).unwrap();
        let link = std::fs::symlink_metadata(t.0.join("sub/up")).unwrap().len();
        let gen = AtomicU64::new(1);
        assert_eq!(du(&t.0, 1, &gen), Some(15 + link));
        // Another item was selected meanwhile.
        gen.store(2, Ordering::SeqCst);
        assert_eq!(du(&t.0, 1, &gen), None);
    }

    #[test]
    fn caja_custom_icons() {
        let t = TestDir::new("custom");
        let pic = t.file("icon.png", "x");
        let i = gio::FileInfo::new();
        assert!(custom_icon(&i).is_none());
        i.set_attribute_string("metadata::custom-icon-name", "folder-music");
        assert!(matches!(custom_icon(&i), Some(Custom::Name(n)) if n == "folder-music"));
        // A picture wins, as a URI or (older Nautilus) a path …
        i.set_attribute_string("metadata::custom-icon", gio::File::for_path(&pic).uri().as_str());
        assert!(matches!(custom_icon(&i), Some(Custom::Picture(p)) if p == pic));
        i.set_attribute_string("metadata::custom-icon", pic.to_str().unwrap());
        assert!(matches!(custom_icon(&i), Some(Custom::Picture(p)) if p == pic));
        // … while it's there.
        i.set_attribute_string("metadata::custom-icon", "file:///no/such/icon.png");
        assert!(matches!(custom_icon(&i), Some(Custom::Name(_))));
        i.set_attribute_string("metadata::custom-icon-name", "");
        assert!(custom_icon(&i).is_none());
    }
}
