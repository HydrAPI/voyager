//! Small helpers: formatting, naming, dialogs, launching.

use gio::prelude::*;
use gtk::prelude::*;

/// Sizes in IEC units (KiB, MiB: powers of 1024) instead of SI (kB, MB).
/// A process-wide flag, as file operations format sizes on their own thread.
static IEC_UNITS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// GVfs tells its metadata store about every local file removed or moved
/// (GDaemonVfs's local_file_removed / local_file_moved, called by
/// g_file_delete and the like) and waits for the answer. Even decoding an
/// image does it: gdk-pixbuf's glycin loader removes a temporary file. So
/// when the store was busy, GTK loading an icon froze the window until it
/// answered (up to 25 s, D-Bus's limit). From the main thread, the store is
/// now told from a thread of its own, in the same order; other threads (file
/// operations) wait for it as before.
pub fn tell_metadata_store_in_background() {
    use std::ffi::{c_char, CStr, CString};
    use std::sync::{mpsc, OnceLock};
    type Removed = unsafe extern "C" fn(*mut gio::ffi::GVfs, *const c_char);
    type Moved = unsafe extern "C" fn(*mut gio::ffi::GVfs, *const c_char, *const c_char);
    enum Job {
        Removed(usize, CString),
        Moved(usize, CString, CString),
    }
    static ORIGINAL: OnceLock<(Option<Removed>, Option<Moved>)> = OnceLock::new();
    static QUEUE: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

    fn on_main_thread() -> bool {
        glib::MainContext::default().is_owner()
    }
    unsafe extern "C" fn removed(vfs: *mut gio::ffi::GVfs, name: *const c_char) {
        let Some(original) = ORIGINAL.get().and_then(|o| o.0) else { return };
        if on_main_thread() && !name.is_null() {
            if let Some(q) = QUEUE.get() {
                if q.send(Job::Removed(vfs as usize, CStr::from_ptr(name).to_owned())).is_ok() {
                    return;
                }
            }
        }
        original(vfs, name);
    }
    unsafe extern "C" fn moved(vfs: *mut gio::ffi::GVfs, from: *const c_char, to: *const c_char) {
        let Some(original) = ORIGINAL.get().and_then(|o| o.1) else { return };
        if on_main_thread() && !from.is_null() && !to.is_null() {
            if let Some(q) = QUEUE.get() {
                if q.send(Job::Moved(vfs as usize, CStr::from_ptr(from).to_owned(), CStr::from_ptr(to).to_owned())).is_ok() {
                    return;
                }
            }
        }
        original(vfs, from, to);
    }

    if ORIGINAL.get().is_some() {
        return;
    }
    let vfs = gio::Vfs::default();
    // SAFETY: the default GVfs lives as long as the program; its class
    // structure is GVfsClass (or a subclass of it), and only the two
    // functions are swapped, before anything else runs on other threads.
    unsafe {
        use glib::translate::ToGlibPtr;
        let instance: *mut gio::ffi::GVfs = vfs.to_glib_none().0;
        let class = (*(instance as *mut glib::gobject_ffi::GTypeInstance)).g_class as *mut gio::ffi::GVfsClass;
        let original = ((*class).local_file_removed, (*class).local_file_moved);
        if original == (None, None) || ORIGINAL.set(original).is_err() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let started = std::thread::Builder::new().name("metadata".into()).spawn(move || {
            let (removed, moved) = original;
            for job in rx {
                match job {
                    Job::Removed(vfs, name) => {
                        if let Some(f) = removed {
                            f(vfs as *mut gio::ffi::GVfs, name.as_ptr());
                        }
                    }
                    Job::Moved(vfs, from, to) => {
                        if let Some(f) = moved {
                            f(vfs as *mut gio::ffi::GVfs, from.as_ptr(), to.as_ptr());
                        }
                    }
                }
            }
        });
        if started.is_err() || QUEUE.set(tx).is_err() {
            return;
        }
        if original.0.is_some() {
            (*class).local_file_removed = Some(removed);
        }
        if original.1.is_some() {
            (*class).local_file_moved = Some(moved);
        }
    }
}

/// GtkTreeView::scroll_to_cell, once the view is realized. Asked before
/// that, GTK keeps the scroll for its next size allocation, which can come
/// while the view still isn't (a column of a tab not shown yet), and then
/// warns (gtk_tree_view_get_cell_area): it's asked again when the view
/// shows.
pub fn scroll_tree_to(view: &gtk::TreeView, path: &gtk::TreePath, use_align: bool, row_align: f32, col_align: f32) {
    if view.is_realized() {
        view.scroll_to_cell(Some(path), None::<&gtk::TreeViewColumn>, use_align, row_align, col_align);
        return;
    }
    let path = path.clone();
    let handler: std::rc::Rc<std::cell::RefCell<Option<glib::SignalHandlerId>>> = Default::default();
    let h = handler.clone();
    let id = view.connect_map(move |v| {
        if let Some(id) = h.borrow_mut().take() {
            v.disconnect(id);
        }
        v.scroll_to_cell(Some(&path), None::<&gtk::TreeViewColumn>, use_align, row_align, col_align);
    });
    *handler.borrow_mut() = Some(id);
}

/// Hands memory freed in bulk (a closed tab, a big folder left) back to
/// the system, a moment later and once for several. Folders are many small
/// pieces (a row, its texts), and the allocator keeps the pages they leave
/// for later: Arkha would go on showing the most it ever used.
pub fn release_memory_soon() {
    thread_local! { static PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(std::time::Duration::from_secs(1), || {
        PENDING.with(|p| p.set(false));
        #[cfg(target_env = "gnu")]
        // SAFETY: no arguments to get wrong; glibc's own locking.
        unsafe {
            libc::malloc_trim(0);
        }
    });
}

pub fn set_iec_units(on: bool) {
    IEC_UNITS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn format_size(n: u64) -> String {
    let flags = if IEC_UNITS.load(std::sync::atomic::Ordering::Relaxed) { glib::FormatSizeFlags::IEC_UNITS } else { glib::FormatSizeFlags::DEFAULT };
    glib::format_size_full(n, flags).to_string()
}

/// Writes a settings file: whole or not at all (a new file renamed over
/// the old), but without waiting for the disk to have it, as
/// KeyFile::save_to_file does. That takes a moment, longer while the disk
/// is busy (with a copy, say), and settings are saved as they change.
pub fn save_settings(kf: &glib::KeyFile, path: &std::path::Path) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // GLib's own way (file_set_contents_full, CONSISTENT) flushes too.
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&tmp, kf.to_data().as_bytes()).is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Shows a desktop notification from Arkha (as libnotify does, which Caja
/// uses: clicking it does nothing).
pub fn notify(summary: &str, body: &str) {
    send_notification(&Notification { summary, body, icon: "system-file-manager", sticky: false }, |_| {});
}

pub struct Notification<'a> {
    pub summary: &'a str,
    pub body: &'a str,
    pub icon: &'a str,
    /// Stays until it's closed (`close_notification`) instead of timing
    /// out, and isn't kept in the notification history (critical urgency,
    /// transient), like Caja's "Writing data to …".
    pub sticky: bool,
}

/// Sends `n`; `shown` gets its id (0 if it couldn't be shown).
pub fn send_notification(n: &Notification, shown: impl FnOnce(u32) + 'static) {
    let mut hints: std::collections::HashMap<String, glib::Variant> = [("desktop-entry".to_string(), "arkha".to_variant())].into();
    if n.sticky {
        hints.insert("urgency".to_string(), 2u8.to_variant());
        hints.insert("transient".to_string(), true.to_variant());
    }
    let args = ("Arkha", 0u32, n.icon, n.summary, n.body, Vec::<String>::new(), hints, -1i32).to_variant();
    notifications_call("Notify", args, move |reply| shown(reply.and_then(|r| r.child_value(0).get::<u32>()).unwrap_or(0)));
}

pub fn close_notification(id: u32) {
    if id != 0 {
        notifications_call("CloseNotification", (id,).to_variant(), |_| {});
    }
}

fn notifications_call(method: &'static str, args: glib::Variant, done: impl FnOnce(Option<glib::Variant>) + 'static) {
    gio::bus_get(gio::BusType::Session, gio::Cancellable::NONE, move |bus| {
        let Ok(bus) = bus else { return done(None) };
        bus.call(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            method,
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            gio::Cancellable::NONE,
            move |r| done(r.ok()),
        );
    });
}

pub fn format_time(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    let Ok(dt) = glib::DateTime::from_unix_local(t) else {
        return String::new();
    };
    let Some((today, yesterday)) = recent_days() else {
        return String::new();
    };
    let fmt = if dt.ymd() == today {
        "Today %H:%M"
    } else if dt.ymd() == yesterday {
        "Yesterday %H:%M"
    } else {
        "%-d %b %Y %H:%M"
    };
    dt.format(fmt).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// (year, month, day)
type Date = (i32, i32, i32);

/// Today's and yesterday's dates, looked up once a minute: a folder's
/// listing formats thousands of times.
fn recent_days() -> Option<(Date, Date)> {
    type Days = Option<(i64, Date, Date)>;
    thread_local! {
        static DAYS: std::cell::Cell<Days> = const { std::cell::Cell::new(None) };
    }
    let now = glib::DateTime::now_local().ok()?;
    if let Some((at, today, yesterday)) = DAYS.with(|d| d.get()) {
        if now.to_unix() - at < 60 && now.ymd() == today {
            return Some((today, yesterday));
        }
    }
    let days = (now.ymd(), now.add_days(-1).ok()?.ymd());
    DAYS.with(|d| d.set(Some((now.to_unix(), days.0, days.1))));
    Some(days)
}

pub fn format_time_long(t: i64) -> String {
    glib::DateTime::from_unix_local(t)
        .ok()
        .and_then(|d| d.format("%A, %-d %B %Y, %H:%M:%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

/// Split "archive.tar.gz" into ("archive", ".tar.gz"), "photo.jpg" into ("photo", ".jpg").
/// Hidden files without another dot have no extension.
pub fn split_ext(name: &str) -> (&str, &str) {
    let lower = name.to_lowercase();
    for multi in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz"] {
        if lower.ends_with(multi) && name.len() > multi.len() {
            let i = name.len() - multi.len();
            return (&name[..i], &name[i..]);
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A name in `dir` that doesn't exist yet: "name", "name (2)", ...
pub fn unique_name(dir: &gio::File, name: &str, is_dir: bool) -> String {
    if !dir.child(name).query_exists(gio::Cancellable::NONE) {
        return name.to_string();
    }
    let (stem, ext) = if is_dir { (name, "") } else { split_ext(name) };
    for i in 2.. {
        let cand = format!("{stem} ({i}){ext}");
        if !dir.child(&cand).query_exists(gio::Cancellable::NONE) {
            return cand;
        }
    }
    unreachable!()
}

/// Name for a copy pasted into the same folder: "a (copy).txt", "a (copy 2).txt", ...
pub fn copy_name(dir: &gio::File, name: &str, is_dir: bool) -> String {
    let (stem, ext) = if is_dir { (name, "") } else { split_ext(name) };
    let first = format!("{stem} (copy){ext}");
    if !dir.child(&first).query_exists(gio::Cancellable::NONE) {
        return first;
    }
    for i in 2.. {
        let cand = format!("{stem} (copy {i}){ext}");
        if !dir.child(&cand).query_exists(gio::Cancellable::NONE) {
            return cand;
        }
    }
    unreachable!()
}

pub fn display_name(file: &gio::File) -> String {
    if let Ok(info) = file.query_info(
        "standard::display-name",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) {
        return info.display_name().to_string();
    }
    file.basename()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| file.uri().to_string())
}

/// Human readable location for the title / properties.
pub fn pretty_location(file: &gio::File) -> String {
    match file.path() {
        Some(p) => {
            let s = p.to_string_lossy().to_string();
            if let Some(home) = dirs_home() {
                if s == home {
                    return "~".into();
                }
            }
            s
        }
        None => glib::Uri::unescape_string(file.uri().as_str(), None::<&str>)
            .map(|s| s.to_string())
            .unwrap_or_else(|| file.uri().to_string()),
    }
}

pub fn dirs_home() -> Option<String> {
    Some(glib::home_dir().to_string_lossy().to_string())
}

pub fn home_file() -> gio::File {
    gio::File::for_path(glib::home_dir())
}

pub fn error_dialog(parent: Option<&gtk::Window>, title: &str, detail: &str) {
    let d = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Error,
        gtk::ButtonsType::Close,
        title,
    );
    d.set_secondary_text(Some(detail));
    d.connect_response(|d, _| d.close());
    d.show();
}

/// Find the first program on $PATH out of the candidates.
pub fn find_program(cands: &[&str]) -> Option<String> {
    cands
        .iter()
        .find(|c| glib::find_program_in_path(c).is_some())
        .map(|s| s.to_string())
}

pub fn spawn_in_dir(argv: &[String], dir: Option<&std::path::Path>) -> Result<(), String> {
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Reap it when it exits so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// argv to run `program` in a new window of `term`, keeping the path as one argument.
pub fn terminal_run_argv(term: &str, program: &str) -> Vec<String> {
    let name = std::path::Path::new(term)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut argv = vec![term.to_string()];
    match name.as_str() {
        "gnome-terminal" => argv.push("--".into()),
        "mate-terminal" | "xfce4-terminal" => argv.push("-x".into()),
        "kitty" => {}
        _ => argv.push("-e".into()),
    }
    argv.push(program.to_string());
    argv
}

pub fn terminal_command() -> Option<String> {
    if let Ok(t) = std::env::var("TERMINAL") {
        if glib::find_program_in_path(&t).is_some() {
            return Some(t);
        }
    }
    find_program(&[
        "mate-terminal",
        "x-terminal-emulator",
        "xfce4-terminal",
        "gnome-terminal",
        "tilix",
        "konsole",
        "alacritty",
        "kitty",
        "xterm",
    ])
}

pub fn archiver_command() -> Option<String> {
    find_program(&["engrampa", "file-roller", "xarchiver"])
}

pub fn is_archive(ctype: &str) -> bool {
    const T: &[&str] = &[
        "application/zip",
        "application/x-tar",
        "application/x-compressed-tar",
        "application/x-bzip-compressed-tar",
        "application/x-xz-compressed-tar",
        "application/x-zstd-compressed-tar",
        "application/x-7z-compressed",
        "application/vnd.rar",
        "application/x-rar",
        "application/gzip",
        "application/x-bzip",
        "application/x-xz",
        "application/x-lzma",
        "application/x-cpio",
        "application/x-rpm",
        "application/vnd.debian.binary-package",
        "application/x-deb",
        "application/x-iso9660-image",
        "application/java-archive",
    ];
    T.contains(&ctype)
}

// Some backends (phones over MTP) leave attributes out of a GFileInfo, and
// its getters warn about that ("GFileInfo created without …"). These check
// first and fall back on the name.

/// The item's name, if the info has it.
fn info_name(info: &gio::FileInfo) -> Option<String> {
    info.has_attribute("standard::name").then(|| info.name().to_string_lossy().to_string())
}

/// Hidden: as reported, or else a name starting with a dot.
pub fn info_is_hidden(info: &gio::FileInfo) -> bool {
    if info.has_attribute("standard::is-hidden") {
        return info.is_hidden();
    }
    info_name(info).is_some_and(|n| n.starts_with('.'))
}

/// Whether an item named `name` is left out while hidden items are: a dot
/// file or a backup ("~"), going by the name alone.
pub fn name_hidden(name: &str) -> bool {
    name.starts_with('.') || name.ends_with('~')
}

/// Backup file: as reported, or else a name ending in "~".
pub fn info_is_backup(info: &gio::FileInfo) -> bool {
    if info.has_attribute("standard::is-backup") {
        return info.is_backup();
    }
    info_name(info).is_some_and(|n| n.ends_with('~'))
}

pub fn info_is_symlink(info: &gio::FileInfo) -> bool {
    info.has_attribute("standard::is-symlink") && info.is_symlink()
}

/// The content type: as reported, or else guessed from the name.
pub fn info_content_type(info: &gio::FileInfo) -> Option<String> {
    if info.has_attribute("standard::content-type") {
        return info.content_type().map(|s| s.to_string());
    }
    info_name(info).map(|n| guess_content_type(&n))
}

/// The content type for a file name. (gio-rs passes an empty slice as
/// zero bytes of data, which makes every guess "application/x-zerosize";
/// no data at all has to be NULL.)
fn guess_content_type(name: &str) -> String {
    use glib::translate::*;
    unsafe {
        let p = gio::ffi::g_content_type_guess(name.to_glib_none().0, std::ptr::null(), 0, std::ptr::null_mut());
        glib::GString::from_glib_full(p).to_string()
    }
}

/// Bookmarks are shared with Caja / GTK file chooser.
pub fn bookmarks_file() -> std::path::PathBuf {
    crate::config::config_home().join("gtk-3.0").join("bookmarks")
}

pub fn read_bookmarks() -> Vec<(String, String)> {
    std::fs::read_to_string(bookmarks_file())
        .map(|text| parse_bookmarks(&text))
        .unwrap_or_default()
}

/// (URI, label) of each line of a bookmarks file; without a label, the
/// URI's file name.
fn parse_bookmarks(text: &str) -> Vec<(String, String)> {
    bookmark_lines(text)
        .into_iter()
        .map(|(uri, label)| {
            let label = if label.is_empty() { default_bookmark_label(&uri) } else { label };
            (uri, label)
        })
        .collect()
}

/// (URI, label) of each line as written: the label empty without one.
fn bookmark_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once(' ') {
            Some((u, n)) => (u.to_string(), n.to_string()),
            None => (l.to_string(), String::new()),
        })
        .collect()
}

/// What a bookmark without a label is called: its folder's name.
fn default_bookmark_label(uri: &str) -> String {
    gio::File::for_uri(uri).basename().map(|b| b.to_string_lossy().to_string()).unwrap_or_else(|| uri.to_string())
}

/// Changes the bookmarks file: `f` gets its (URI, label) lines, the label
/// empty for the folder's name. A label that's just the folder's name isn't
/// written. The windows watch the file and update their bookmarks.
pub fn edit_bookmarks(f: impl FnOnce(&mut Vec<(String, String)>)) -> std::io::Result<()> {
    edit_bookmarks_at(&bookmarks_file(), f)
}

fn edit_bookmarks_at(path: &std::path::Path, f: impl FnOnce(&mut Vec<(String, String)>)) -> std::io::Result<()> {
    let old = bookmark_lines(&std::fs::read_to_string(path).unwrap_or_default());
    let mut b = old.clone();
    f(&mut b);
    if b == old {
        return Ok(());
    }
    let mut out = String::new();
    for (uri, label) in &b {
        out.push_str(uri);
        if !label.is_empty() && *label != default_bookmark_label(uri) {
            out.push(' ');
            out.push_str(label);
        }
        out.push('\n');
    }
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, out)
}

pub fn add_bookmark(uri: &str) -> std::io::Result<bool> {
    add_bookmark_to(&bookmarks_file(), uri)
}

/// Adds `uri` at the end of the bookmarks file `path`; false if it's there
/// already.
fn add_bookmark_to(path: &std::path::Path, uri: &str) -> std::io::Result<bool> {
    let mut added = false;
    edit_bookmarks_at(path, |b| {
        if !b.iter().any(|(u, _)| u == uri) {
            b.push((uri.to_string(), String::new()));
            added = true;
        }
    })?;
    Ok(added)
}

/// Minimal shell-style glob: * ? and [abc].
pub fn glob_match(pat: &[char], s: &[char]) -> bool {
    // On a mismatch only the last "*" takes one more character: trying every
    // split for every "*" takes exponential time with a few of them.
    let (mut p, mut i) = (0, 0);
    // The pattern position after the last "*", and the text position it's up to.
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        if pat.get(p) == Some(&'*') {
            p += 1;
            star = Some((p, i));
            continue;
        }
        if let Some(len) = pat.get(p..).and_then(|rest| glob_one(rest, s[i])) {
            p += len;
            i += 1;
            continue;
        }
        let Some((sp, si)) = star else { return false };
        p = sp;
        i = si + 1;
        star = Some((sp, i));
    }
    pat[p..].iter().all(|c| *c == '*')
}

/// Whether the first element of `pat` (a character, "?" or a "[set]")
/// matches `c`: how many pattern characters it took, or None.
fn glob_one(pat: &[char], c: char) -> Option<usize> {
    match *pat.first()? {
        '?' => Some(1),
        '[' => match pat.iter().position(|x| *x == ']') {
            Some(end) if end > 1 => pat[1..end].contains(&c).then_some(end + 1),
            _ => (c == '[').then_some(1),
        },
        p => (p == c).then_some(1),
    }
}

/// Whether a text field has the focus (an entry, or the multi-line rename
/// editor of the icon views): its keys (Ctrl+A, Delete, Backspace, …) are
/// then for the text, not the window's shortcuts.
pub fn is_editable_focused(win: &gtk::Window) -> bool {
    win.focused_widget()
        .map(|w| w.is::<gtk::Editable>() || w.is::<gtk::TextView>())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    fn glob(p: &str, s: &str) -> bool {
        glob_match(&p.chars().collect::<Vec<_>>(), &s.chars().collect::<Vec<_>>())
    }

    #[test]
    fn globs() {
        assert!(glob("*.jpg", "photo.jpg"));
        assert!(!glob("*.jpg", "photo.jpeg"));
        assert!(glob("*", "") && glob("*", "anything"));
        assert!(glob("", "") && !glob("", "x"));
        assert!(glob("IMG_????.*", "IMG_0042.png"));
        assert!(!glob("IMG_????.*", "IMG_42.png"));
        assert!(!glob("?", ""));
        assert!(glob("[ab]*", "beta") && !glob("[ab]*", "gamma"));
        assert!(glob("a*b*c", "aXXbYYc") && !glob("a*b*c", "aXXbYY"));
        assert!(glob("*.tar.*", "x.tar.gz"));
        assert!(glob("ünï*", "ünïcode"));
        // A "[" without a set is an ordinary character.
        assert!(glob("[", "[") && glob("x[", "x[") && glob("[]", "[]"));
    }

    #[test]
    fn glob_with_many_stars_is_quick() {
        let name = "a".repeat(200);
        let t = std::time::Instant::now();
        assert!(!glob("*a*a*a*a*a*a*a*a*b", &name));
        assert!(glob("*a*a*a*a*a*a*a*a*", &name));
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "took {:?}", t.elapsed());
    }

    #[test]
    fn extensions() {
        assert_eq!(split_ext("photo.jpg"), ("photo", ".jpg"));
        assert_eq!(split_ext("a.b.c"), ("a.b", ".c"));
        assert_eq!(split_ext("archive.tar.gz"), ("archive", ".tar.gz"));
        assert_eq!(split_ext("Backup.TAR.XZ"), ("Backup", ".TAR.XZ"));
        assert_eq!(split_ext("Fotos-Ünïcödé.tar.zst"), ("Fotos-Ünïcödé", ".tar.zst"));
        assert_eq!(split_ext("notes"), ("notes", ""));
        // A leading dot alone (hidden files) isn't an extension.
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext(".config.bak"), (".config", ".bak"));
        assert_eq!(split_ext(".tar.gz"), (".tar", ".gz"));
        assert_eq!(split_ext(""), ("", ""));
    }

    #[test]
    fn free_names() {
        let t = TestDir::new("names");
        let dir = gio::File::for_path(&t.0);
        assert_eq!(unique_name(&dir, "a.txt", false), "a.txt");
        t.file("a.txt", "");
        assert_eq!(unique_name(&dir, "a.txt", false), "a (2).txt");
        t.file("a (2).txt", "");
        assert_eq!(unique_name(&dir, "a.txt", false), "a (3).txt");
        t.file("b.tar.gz", "");
        assert_eq!(unique_name(&dir, "b.tar.gz", false), "b (2).tar.gz");
        std::fs::create_dir(t.0.join("my.folder")).unwrap();
        // A folder's name has no extension.
        assert_eq!(unique_name(&dir, "my.folder", true), "my.folder (2)");

        assert_eq!(copy_name(&dir, "a.txt", false), "a (copy).txt");
        t.file("a (copy).txt", "");
        assert_eq!(copy_name(&dir, "a.txt", false), "a (copy 2).txt");
        t.file("a (copy 2).txt", "");
        assert_eq!(copy_name(&dir, "a.txt", false), "a (copy 3).txt");
        assert_eq!(copy_name(&dir, "my.folder", true), "my.folder (copy)");
        assert_eq!(copy_name(&dir, ".hidden", false), ".hidden (copy)");
    }

    #[test]
    fn sizes() {
        // One test for both units, as the setting is shared by the whole process.
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1), "1 byte");
        // GLib keeps number and unit together with a no-break space.
        assert_eq!(format_size(1000), "1.0\u{a0}kB");
        assert_eq!(format_size(1_500_000), "1.5\u{a0}MB");
        set_iec_units(true);
        let iec = [format_size(1000), format_size(1024), format_size(1_572_864)];
        set_iec_units(false);
        assert_eq!(iec, ["1000 bytes", "1.0\u{a0}KiB", "1.5\u{a0}MiB"]);
    }

    #[test]
    fn times() {
        assert_eq!(format_time(0), "");
        assert_eq!(format_time(-5), "");
        let now = glib::DateTime::now_local().unwrap();
        assert!(format_time(now.to_unix()).starts_with("Today "));
        assert!(format_time(now.add_days(-1).unwrap().to_unix()).starts_with("Yesterday "));
        // 9 September 2001, 01:46:40 UTC: the 8th or 9th wherever we are.
        assert!(format_time(1_000_000_000).contains("Sep 2001"));
        assert!(format_time_long(1_000_000_000).contains("September 2001"));
    }

    #[test]
    fn terminals() {
        assert_eq!(terminal_run_argv("gnome-terminal", "/a b/run"), ["gnome-terminal", "--", "/a b/run"]);
        assert_eq!(terminal_run_argv("/usr/bin/xfce4-terminal", "x"), ["/usr/bin/xfce4-terminal", "-x", "x"]);
        assert_eq!(terminal_run_argv("mate-terminal", "x"), ["mate-terminal", "-x", "x"]);
        assert_eq!(terminal_run_argv("kitty", "x"), ["kitty", "x"]);
        assert_eq!(terminal_run_argv("xterm", "x"), ["xterm", "-e", "x"]);
    }

    #[test]
    fn locations_and_hidden_names() {
        assert_eq!(pretty_location(&home_file()), "~");
        assert_eq!(pretty_location(&gio::File::for_path("/a b/c")), "/a b/c");
        assert_eq!(pretty_location(&gio::File::for_uri("sftp://host/My%20Files")), "sftp://host/My Files");
        assert!(name_hidden(".bashrc") && name_hidden("notes.txt~"));
        assert!(!name_hidden("a.txt") && !name_hidden("a~b"));
    }

    #[test]
    fn info_fallbacks() {
        let i = gio::FileInfo::new();
        assert!(!info_is_hidden(&i) && !info_is_backup(&i) && !info_is_symlink(&i));
        assert_eq!(info_content_type(&i), None);
        // Without the attributes (MTP), the name decides.
        i.set_name(".notes.txt~");
        assert!(info_is_hidden(&i) && info_is_backup(&i));
        i.set_name("photo.png");
        assert!(!info_is_hidden(&i) && !info_is_backup(&i));
        assert_eq!(info_content_type(&i).as_deref(), Some("image/png"));
        // What the backend reports wins.
        i.set_name(".x~");
        i.set_is_hidden(false);
        i.set_attribute_boolean("standard::is-backup", false);
        assert!(!info_is_hidden(&i) && !info_is_backup(&i));
        i.set_content_type("text/plain");
        assert_eq!(info_content_type(&i).as_deref(), Some("text/plain"));
        i.set_is_symlink(true);
        assert!(info_is_symlink(&i));
    }

    #[test]
    fn bookmarks() {
        let b = parse_bookmarks("file:///home/u/Music\nfile:///home/u/My%20Docs Work stuff\n\nsftp://host/srv Server\n");
        let want: Vec<(String, String)> = [
            ("file:///home/u/Music", "Music"),
            ("file:///home/u/My%20Docs", "Work stuff"),
            ("sftp://host/srv", "Server"),
        ]
        .map(|(u, l)| (u.to_string(), l.to_string()))
        .to_vec();
        assert_eq!(b, want);
        assert_eq!(parse_bookmarks("file:///a%20b")[0].1, "a b");

        let t = TestDir::new("bookmarks");
        let p = t.0.join("gtk-3.0").join("bookmarks");
        assert!(add_bookmark_to(&p, "file:///a").unwrap());
        assert!(!add_bookmark_to(&p, "file:///a").unwrap());
        // A labelled line without a final newline.
        std::fs::write(&p, "file:///a A").unwrap();
        assert!(!add_bookmark_to(&p, "file:///a").unwrap());
        assert!(add_bookmark_to(&p, "file:///ab").unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "file:///a A\nfile:///ab\n");
        // A label that's just the folder's name isn't written; an empty one
        // means the folder's name.
        edit_bookmarks_at(&p, |b| {
            b[0].1 = "a".into();
            b[1].1 = "Other".into();
            b.push(("file:///c".into(), String::new()));
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "file:///a\nfile:///ab Other\nfile:///c\n");
        // Nothing changed: the file isn't written.
        std::fs::write(&p, "file:///a\n\nfile:///b B").unwrap();
        edit_bookmarks_at(&p, |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "file:///a\n\nfile:///b B");
    }

    #[test]
    fn random_names_patterns_and_bookmarks() {
        use crate::testutil::cases;
        cases(3000, |r| {
            let name = r.text(20);
            let (a, b) = split_ext(&name);
            assert_eq!(format!("{a}{b}"), name, "split_ext({name:?})");
            let n: Vec<char> = name.chars().collect();
            // Without its special characters a pattern matches itself; "*" anything.
            let plain: Vec<char> = n.iter().copied().filter(|c| !"*?[".contains(*c)).collect();
            assert!(glob_match(&plain, &plain), "{plain:?}");
            assert!(glob_match(&['*'], &n));
            let pat: Vec<char> = r.text(12).chars().collect();
            let _ = glob_match(&pat, &n);
            let _ = name_hidden(&name);
        });
        // Bookmarks read back as written.
        let t = TestDir::new("random-bookmarks");
        let p = t.0.join("bookmarks");
        let uri_chars: Vec<char> = "abcäß日/%:.-_~".chars().collect();
        cases(300, |r| {
            let list: Vec<(String, String)> = (0..r.below(6))
                .map(|_| (format!("file:///{}", r.text_of(&uri_chars, 12)), r.text(10).replace(['\n', '\r'], "")))
                .collect();
            let _ = std::fs::remove_file(&p);
            edit_bookmarks_at(&p, |b| *b = list.clone()).unwrap();
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let want: Vec<(String, String)> = list
                .iter()
                .map(|(u, l)| (u.clone(), if *l == default_bookmark_label(u) { String::new() } else { l.clone() }))
                .collect();
            assert_eq!(bookmark_lines(&text), want, "{text:?}");
        });
    }
}
