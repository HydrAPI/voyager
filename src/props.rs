//! File / folder properties dialog (Basic, Permissions, Open With, Media Info).

use gio::prelude::*;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::{config, icons, util};

fn row(grid: &gtk::Grid, r: i32, key: &str, val: &gtk::Widget) {
    let k = gtk::Label::new(Some(key));
    k.set_xalign(1.0);
    k.set_yalign(0.0);
    k.style_context().add_class("dim-label");
    grid.attach(&k, 0, r, 1, 1);
    val.set_hexpand(true);
    grid.attach(val, 1, r, 1, 1);
}

fn value_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_selectable(true);
    l.set_line_wrap(true);
    l.set_line_wrap_mode(pango::WrapMode::WordChar);
    l.set_max_width_chars(48);
    l
}

/// Total size and item count of a set of files, computed in a thread.
/// `count_top` counts the given items themselves (a multi-selection), otherwise
/// only what's inside them (a single folder's contents).
fn deep_count(uris: Vec<String>, count_top: bool, label: gtk::Label) {
    let (tx, rx) = async_channel::unbounded::<(u64, u64, bool)>();
    std::thread::spawn(move || {
        let mut bytes = 0u64;
        let mut items = 0u64;
        let mut stack: Vec<std::path::PathBuf> = vec![];
        for u in &uris {
            if let Some(p) = gio::File::for_uri(u).path() {
                stack.push(p);
            }
        }
        let top: std::collections::HashSet<_> = stack.iter().cloned().collect();
        let mut last = std::time::Instant::now();
        while let Some(p) = stack.pop() {
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            if count_top || !top.contains(&p) {
                items += 1;
            }
            if md.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for e in rd.flatten() {
                        stack.push(e.path());
                    }
                }
            } else {
                bytes += md.len();
            }
            if last.elapsed().as_millis() > 150 {
                last = std::time::Instant::now();
                if tx.send_blocking((bytes, items, false)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send_blocking((bytes, items, true));
    });
    glib::MainContext::default().spawn_local(async move {
        while let Ok((b, n, done)) = rx.recv().await {
            label.set_text(&format!(
                "{n} item{}, totalling {}{}",
                if n == 1 { "" } else { "s" },
                util::format_size(b),
                if done { "" } else { " …" }
            ));
        }
    });
}

pub fn show(parent: &gtk::Window, files: Vec<gio::File>) {
    if files.is_empty() {
        return;
    }
    let dialog = gtk::Dialog::with_buttons(
        Some(&if files.len() == 1 {
            format!("{} Properties", util::display_name(&files[0]))
        } else {
            format!("Properties of {} items", files.len())
        }),
        Some(parent),
        gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Close", gtk::ResponseType::Close)],
    );
    dialog.set_default_size(460, -1);
    dialog.connect_response(|d, _| d.close());
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    dialog.content_area().pack_start(&nb, true, true, 0);

    let attrs = "standard::*,time::*,unix::*,owner::*,access::*,id::filesystem,filesystem::*,selinux::context";
    // Keep files and infos paired up, dropping files that vanished.
    let (files, infos): (Vec<gio::File>, Vec<gio::FileInfo>) = files
        .into_iter()
        .filter_map(|f| {
            let info = f.query_info(attrs, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok()?;
            Some((f, info))
        })
        .unzip();
    if infos.is_empty() {
        util::error_dialog(Some(parent), "Could not read properties", "The file may have been removed.");
        return;
    }

    // ---------------------------------------------------------------- Basic
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_border_width(12);
    let mut r = 0;

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let img = gtk::Image::new();
    if let Some(pb) = icons::icon_pixbuf(infos[0].icon().as_ref(), 64) {
        img.set_from_pixbuf(Some(&pb));
    }
    head.pack_start(&img, false, false, 0);

    if files.len() == 1 {
        let f = files[0].clone();
        let info = &infos[0];
        let name_entry = gtk::Entry::new();
        name_entry.set_text(&info.display_name());
        name_entry.set_hexpand(true);
        name_entry.set_valign(gtk::Align::Center);
        name_entry.set_editable(info.boolean("access::can-rename"));
        {
            let cur = std::rc::Rc::new(std::cell::RefCell::new(f.clone()));
            let d = dialog.clone();
            name_entry.connect_activate(move |e| {
                let new = e.text().to_string();
                let file = cur.borrow().clone();
                if new.is_empty() || Some(new.as_str()) == file.basename().map(|b| b.to_string_lossy().to_string()).as_deref() {
                    return;
                }
                match file.set_display_name(&new, gio::Cancellable::NONE) {
                    Ok(nf) => {
                        *cur.borrow_mut() = nf;
                        d.set_title(&format!("{new} Properties"));
                    }
                    Err(err) => util::error_dialog(d.transient_for().as_ref(), "Could not rename", err.message()),
                }
            });
        }
        head.pack_start(&name_entry, true, true, 0);
        grid.attach(&head, 0, r, 2, 1);
        r += 1;

        let is_dir = info.file_type() == gio::FileType::Directory;
        let ctype = util::info_content_type(info).unwrap_or_default();
        let tdesc = if is_dir {
            "Folder (inode/directory)".to_string()
        } else {
            format!("{} ({ctype})", gio::content_type_get_description(&ctype))
        };
        row(&grid, r, "Type:", value_label(&tdesc).upcast_ref());
        r += 1;
        if util::info_is_symlink(info) {
            if let Some(t) = info.symlink_target() {
                row(&grid, r, "Link target:", value_label(&t.to_string_lossy()).upcast_ref());
                r += 1;
            }
        }
        let size_label = value_label("");
        if is_dir {
            size_label.set_text("Counting…");
            if f.path().is_some() {
                deep_count(vec![f.uri().to_string()], false, size_label.clone());
            } else {
                size_label.set_text("Unknown");
            }
            row(&grid, r, "Contents:", size_label.upcast_ref());
        } else {
            let s = info.size().max(0) as u64;
            size_label.set_text(&format!("{} ({} bytes)", util::format_size(s), group_digits(s)));
            row(&grid, r, "Size:", size_label.upcast_ref());
        }
        r += 1;
        let parent_loc = f.parent().map(|p| util::pretty_location(&p)).unwrap_or_default();
        row(&grid, r, "Location:", value_label(&parent_loc).upcast_ref());
        r += 1;
        if let Ok(fsinfo) = f.query_filesystem_info("filesystem::free,filesystem::size", gio::Cancellable::NONE) {
            if fsinfo.has_attribute("filesystem::free") {
                row(
                    &grid,
                    r,
                    "Free space:",
                    value_label(&format!(
                        "{} of {}",
                        util::format_size(fsinfo.attribute_uint64("filesystem::free")),
                        util::format_size(fsinfo.attribute_uint64("filesystem::size"))
                    ))
                    .upcast_ref(),
                );
                r += 1;
            }
        }
        // A drive's top folder: how full it is, as in Caja.
        if is_dir && is_mount_root(&f) {
            if let Some(usage) = disk_usage(&f) {
                grid.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1);
                r += 1;
                row(&grid, r, "Disk usage:", usage.upcast_ref());
                r += 1;
            }
        }
        grid.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1);
        r += 1;
        for (key, attr) in [("Accessed:", "time::access"), ("Modified:", "time::modified"), ("Created:", "time::created")] {
            if info.has_attribute(attr) {
                row(&grid, r, key, value_label(&util::format_time_long(info.attribute_uint64(attr) as i64)).upcast_ref());
                r += 1;
            }
        }
    } else {
        let l = value_label(&format!("{} items", files.len()));
        head.pack_start(&l, true, true, 0);
        grid.attach(&head, 0, r, 2, 1);
        r += 1;
        let size_label = value_label("Counting…");
        let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
        deep_count(uris, true, size_label.clone());
        row(&grid, r, "Contents:", size_label.upcast_ref());
        r += 1;
        let parent_loc = files[0].parent().map(|p| util::pretty_location(&p)).unwrap_or_default();
        row(&grid, r, "Location:", value_label(&parent_loc).upcast_ref());
    }
    nb.append_page(&grid, Some(&gtk::Label::new(Some("Basic"))));

    // ---------------------------------------------------------- Permissions
    if infos.iter().all(|i| i.has_attribute("unix::mode")) {
        let pg = gtk::Grid::new();
        pg.set_row_spacing(8);
        pg.set_column_spacing(12);
        pg.set_border_width(12);
        let info = &infos[0];
        let mut r = 0;
        let owner = info.attribute_string("owner::user").map(|s| s.to_string()).unwrap_or_default();
        let group = info.attribute_string("owner::group").map(|s| s.to_string()).unwrap_or_default();
        if files.len() == 1 {
            row(&pg, r, "Owner:", value_label(&owner).upcast_ref());
            r += 1;
            row(&pg, r, "Group:", value_label(&group).upcast_ref());
            r += 1;
        }
        // Every file keeps its own mode; a checkbox only flips its one bit in each.
        let modes: Rc<Vec<Cell<u32>>> = Rc::new(infos.iter().map(|i| Cell::new(i.attribute_uint32("unix::mode"))).collect());
        let my_uid = unsafe { libc_getuid() };
        let can_change = infos.iter().all(|i| i.attribute_uint32("unix::uid") == my_uid || my_uid == 0);
        let headers = ["", "Read", "Write", "Execute"];
        for (c, h) in headers.iter().enumerate() {
            let l = gtk::Label::new(Some(h));
            l.style_context().add_class("dim-label");
            pg.attach(&l, c as i32, r, 1, 1);
        }
        r += 1;
        let files_rc = Rc::new(files.clone());
        let any_dir = infos.iter().any(|i| i.file_type() == gio::FileType::Directory);
        let octal = value_label("");
        octal.style_context().add_class("dim-label");
        let update_octal = {
            let modes = modes.clone();
            let octal = octal.clone();
            Rc::new(move || {
                let first = modes[0].get() & 0o7777;
                let text = if modes.iter().all(|m| m.get() & 0o7777 == first) {
                    format!("Mode: {first:04o}")
                } else {
                    "Mode: mixed".to_string()
                };
                octal.set_text(&format!("{text}{}", if any_dir { "  (execute = enter folder)" } else { "" }));
            })
        };
        update_octal();
        // A check box for one permission bit, flipping it in every file.
        let bit_box = {
            let (modes, files, update_octal, d) = (modes.clone(), files_rc.clone(), update_octal.clone(), dialog.clone());
            move |bit: u32, label: Option<&str>| -> gtk::CheckButton {
                let cb = match label {
                    Some(l) => gtk::CheckButton::with_label(l),
                    None => gtk::CheckButton::new(),
                };
                let on = modes.iter().filter(|m| m.get() & bit != 0).count();
                cb.set_active(on == modes.len());
                cb.set_inconsistent(on != 0 && on != modes.len());
                cb.set_sensitive(can_change);
                let (modes, files, update_octal, d) = (modes.clone(), files.clone(), update_octal.clone(), d.clone());
                cb.connect_toggled(move |cb| {
                    cb.set_inconsistent(false);
                    let set = cb.is_active();
                    for (f, m) in files.iter().zip(modes.iter()) {
                        let new = if set { m.get() | bit } else { m.get() & !bit };
                        if let Err(e) = f.set_attribute_uint32("unix::mode", new & 0o7777, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                            util::error_dialog(d.transient_for().as_ref(), "Could not change permissions", e.message());
                            break;
                        }
                        m.set(new);
                    }
                    update_octal();
                });
                cb
            }
        };
        for (i, who) in ["Owner", "Group", "Others"].iter().enumerate() {
            let l = gtk::Label::new(Some(who));
            l.set_xalign(1.0);
            pg.attach(&l, 0, r, 1, 1);
            for j in 0..3 {
                let cb = bit_box(1u32 << (8 - (i * 3 + j)), None);
                cb.set_halign(gtk::Align::Center);
                pg.attach(&cb, j as i32 + 1, r, 1, 1);
            }
            r += 1;
        }
        // setuid, setgid, sticky: the latter for folders like /tmp, where
        // only an item's owner may delete or rename it.
        {
            let l = gtk::Label::new(Some("Special"));
            l.set_xalign(1.0);
            l.set_yalign(0.0);
            pg.attach(&l, 0, r, 1, 1);
            let specials = gtk::Box::new(gtk::Orientation::Vertical, 2);
            for (bit, label, tip) in [
                (0o4000, "Set user ID", "Run as the file's owner, whoever starts it"),
                (0o2000, "Set group ID", "Run with the file's group; in a folder, new items get the folder's group"),
                (0o1000, "Sticky", "In a folder: only an item's owner can delete or rename it"),
            ] {
                let cb = bit_box(bit, Some(label));
                cb.set_tooltip_text(Some(tip));
                specials.add(&cb);
            }
            pg.attach(&specials, 1, r, 3, 1);
            r += 1;
        }
        pg.attach(&octal, 0, r, 4, 1);
        // Apply to what's inside, as Caja does: folders get this mode,
        // files its read and write bits (their own execute bits stay).
        if can_change && any_dir {
            r += 1;
            let apply = gtk::Button::with_label("Apply to Enclosed Items…");
            apply.set_halign(gtk::Align::Start);
            apply.set_tooltip_text(Some("Give the folders inside these permissions, and the files inside the same read and write permissions"));
            let done = value_label("");
            done.style_context().add_class("dim-label");
            let grid = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            grid.pack_start(&apply, false, false, 0);
            grid.pack_start(&done, true, true, 0);
            pg.attach(&grid, 0, r, 4, 1);
            let (modes, files, d) = (modes.clone(), files_rc.clone(), dialog.clone());
            apply.connect_clicked(move |b| {
                let dirs: Vec<(std::path::PathBuf, u32)> = files
                    .iter()
                    .zip(modes.iter())
                    .filter_map(|(f, m)| Some((f.path()?, m.get() & 0o7777)))
                    .filter(|(p, _)| p.is_dir())
                    .collect();
                if dirs.is_empty() {
                    return;
                }
                let ask = gtk::MessageDialog::new(
                    Some(&d),
                    gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                    gtk::MessageType::Question,
                    gtk::ButtonsType::None,
                    "Apply these permissions to everything inside?",
                );
                ask.set_secondary_text(Some("Every folder inside gets the same permissions, every file the same read and write permissions. This can't be undone."));
                ask.add_button("_Cancel", gtk::ResponseType::Cancel);
                ask.add_button("_Apply", gtk::ResponseType::Accept);
                let ok = ask.run() == gtk::ResponseType::Accept;
                ask.close();
                if !ok {
                    return;
                }
                b.set_sensitive(false);
                done.set_text("Applying…");
                let (b, done) = (b.clone(), done.clone());
                glib::MainContext::default().spawn_local(async move {
                    let (changed, failed) = gio::spawn_blocking(move || apply_enclosed(&dirs)).await.unwrap_or((0, 0));
                    b.set_sensitive(true);
                    let items = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
                    done.set_text(&match failed {
                        0 => format!("Changed {}.", items(changed)),
                        f => format!("Changed {}; {} couldn't be changed (not yours?).", items(changed), items(f)),
                    });
                });
            });
        }
        // The SELinux security context, where the system has one.
        if let Some(ctx) = infos[0].attribute_string("selinux::context").filter(|c| !c.is_empty() && files.len() == 1) {
            r += 1;
            row(&pg, r, "Security context:", value_label(&ctx).upcast_ref());
        }
        if !can_change {
            r += 1;
            let l = value_label("You are not the owner, so you can't change these permissions.");
            pg.attach(&l, 0, r, 4, 1);
        }
        nb.append_page(&pg, Some(&gtk::Label::new(Some("Permissions"))));
    }

    // ------------------------------------------------------------ Open With
    if files.len() == 1 && infos[0].file_type() != gio::FileType::Directory {
        if let Some(ctype) = util::info_content_type(&infos[0]) {
            let ob = gtk::Box::new(gtk::Orientation::Vertical, 8);
            ob.set_border_width(12);
            let l = value_label(&format!(
                "Select an application to open “{}” and other files of type “{}”:",
                infos[0].display_name(),
                gio::content_type_get_description(&ctype)
            ));
            ob.pack_start(&l, false, false, 0);
            let w = gtk::AppChooserWidget::new(&ctype);
            w.set_show_default(true);
            w.set_show_recommended(true);
            w.set_show_fallback(true);
            w.set_vexpand(true);
            w.set_size_request(-1, 260);
            ob.pack_start(&w, true, true, 0);
            let set_btn = gtk::Button::with_mnemonic("_Set as Default");
            set_btn.set_halign(gtk::Align::End);
            {
                let w = w.clone();
                let ctype = ctype.to_string();
                let d = dialog.clone();
                set_btn.connect_clicked(move |_| {
                    if let Some(app) = w.app_info() {
                        if let Err(e) = app.set_as_default_for_type(&ctype) {
                            util::error_dialog(d.transient_for().as_ref(), "Could not set default application", e.message());
                        }
                    }
                });
            }
            ob.pack_start(&set_btn, false, false, 0);
            nb.append_page(&ob, Some(&gtk::Label::new(Some("Open With"))));
        }
    }

    // ----------------------------------------------------------- Media Info
    if files.len() == 1 && infos[0].file_type() == gio::FileType::Regular {
        let ctype = util::info_content_type(&infos[0]).unwrap_or_default();
        if let (Some(path), true) = (files[0].path(), media_info_wanted(&ctype)) {
            nb.append_page(&media_info_page(path), Some(&gtk::Label::new(Some("Media Info"))));
        }
    }

    dialog.show_all();
}

/// Whether mediainfo is installed.
pub fn mediainfo_available() -> bool {
    glib::find_program_in_path("mediainfo").is_some()
}

/// Whether the Media Info tab is wanted for a file of this content type.
fn media_info_wanted(ctype: &str) -> bool {
    let s = config::get();
    let kind_on = match ctype.split('/').next() {
        Some("video") => s.media_info_video,
        Some("audio") => s.media_info_audio,
        Some("image") => s.media_info_image,
        _ => s.media_info_other,
    };
    s.media_info && kind_on && mediainfo_available()
}

/// mediainfo's report for `path`, filled in once it has run in the background.
fn media_info_page(path: std::path::PathBuf) -> gtk::ScrolledWindow {
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bx.set_border_width(12);
    let wait = value_label("Reading media information…");
    wait.style_context().add_class("dim-label");
    bx.add(&wait);
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_min_content_height(320);
    sw.add(&bx);
    glib::MainContext::default().spawn_local(async move {
        let argv = [std::ffi::OsStr::new("mediainfo"), path.as_os_str()];
        let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE;
        let result = match gio::Subprocess::newv(&argv, flags) {
            Ok(p) => match p.communicate_utf8_future(None).await {
                Ok((out, err)) => {
                    let out = out.map(|s| s.to_string()).unwrap_or_default();
                    let err = err.map(|s| s.trim().to_string()).unwrap_or_default();
                    let sections = parse_media_info(&out);
                    if !sections.is_empty() {
                        Ok(sections)
                    } else if !err.is_empty() {
                        Err(format!("mediainfo reported an error:\n{err}"))
                    } else {
                        Err("mediainfo found no media information in this file.".to_string())
                    }
                }
                Err(e) => Err(format!("Could not read mediainfo's output: {}", e.message())),
            },
            Err(e) => Err(format!("Could not run mediainfo: {}", e.message())),
        };
        bx.remove(&wait);
        match result {
            Ok(sections) => {
                let grid = gtk::Grid::new();
                grid.set_row_spacing(6);
                grid.set_column_spacing(12);
                let mut r = 0;
                for (title, fields) in sections {
                    let h = gtk::Label::new(None);
                    h.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&title)));
                    h.set_xalign(0.0);
                    if r > 0 {
                        h.set_margin_top(10);
                    }
                    grid.attach(&h, 0, r, 2, 1);
                    r += 1;
                    for (k, v) in fields {
                        row(&grid, r, &k, value_label(&v).upcast_ref());
                        r += 1;
                    }
                }
                bx.add(&grid);
            }
            Err(msg) => bx.add(&value_label(&msg)),
        }
        bx.show_all();
    });
    sw
}

/// Splits mediainfo's text report into sections ("General", "Video", …) of
/// (field, value) pairs. Fields are "Name   : value" lines; a line without a
/// " : " starts a new section.
fn parse_media_info(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut sections: Vec<(String, Vec<(String, String)>)> = vec![];
    for line in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        match line.split_once(" : ") {
            Some((k, v)) if !sections.is_empty() => {
                sections.last_mut().unwrap().1.push((k.trim().to_string(), v.trim().to_string()));
            }
            Some(_) => {}
            None => sections.push((line.trim().to_string(), vec![])),
        }
    }
    sections.retain(|(_, fields)| !fields.is_empty());
    sections
}

/// Whether `f` is the top folder of a file system: `/`, a folder on another
/// device than its parent, or the root of a mount (a phone, a share).
fn is_mount_root(f: &gio::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    if let Some(p) = f.path() {
        let Some(parent) = p.parent() else { return true };
        if let (Ok(a), Ok(b)) = (std::fs::metadata(&p), std::fs::metadata(parent)) {
            if a.dev() != b.dev() {
                return true;
            }
        }
    }
    f.find_enclosing_mount(gio::Cancellable::NONE).is_ok_and(|m| m.root().equal(f))
}

/// A pie of the used and free space on `f`'s file system, with the
/// numbers beside it.
fn disk_usage(f: &gio::File) -> Option<gtk::Widget> {
    let fs = f.query_filesystem_info("filesystem::size,filesystem::free,filesystem::used,filesystem::type", gio::Cancellable::NONE).ok()?;
    let total = fs.attribute_uint64("filesystem::size");
    if total == 0 {
        return None;
    }
    let free = fs.attribute_uint64("filesystem::free").min(total);
    let used = if fs.has_attribute("filesystem::used") { fs.attribute_uint64("filesystem::used") } else { total - free };
    let fraction = used as f64 / (used + free).max(1) as f64;
    let hb = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let pie = gtk::DrawingArea::new();
    pie.set_size_request(72, 72);
    pie.set_valign(gtk::Align::Start);
    // The theme's selection color for what's used, a pale one for the rest.
    let colors = |w: &gtk::DrawingArea| {
        let sc = w.style_context();
        let used = sc.lookup_color("theme_selected_bg_color").unwrap_or(gdk::RGBA::new(0.21, 0.52, 0.89, 1.0));
        let fg = sc.lookup_color("theme_fg_color").unwrap_or(gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
        (used, gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.15))
    };
    pie.connect_draw(move |w, cr| {
        let (used, free) = colors(w);
        let (wd, ht) = (w.allocated_width() as f64, w.allocated_height() as f64);
        let (cx, cy, rad) = (wd / 2.0, ht / 2.0, wd.min(ht) / 2.0 - 1.0);
        let start = -std::f64::consts::FRAC_PI_2;
        let end = start + fraction * 2.0 * std::f64::consts::PI;
        cr.set_source_rgba(free.red(), free.green(), free.blue(), free.alpha());
        cr.arc(cx, cy, rad, 0.0, 2.0 * std::f64::consts::PI);
        let _ = cr.fill();
        if fraction > 0.0 {
            cr.set_source_rgba(used.red(), used.green(), used.blue(), used.alpha());
            cr.move_to(cx, cy);
            cr.arc(cx, cy, rad, start, end);
            cr.close_path();
            let _ = cr.fill();
        }
        glib::Propagation::Stop
    });
    hb.pack_start(&pie, false, false, 0);
    let legend = gtk::Grid::new();
    legend.set_row_spacing(4);
    legend.set_column_spacing(8);
    let swatch = |used_color: bool| {
        let sw = gtk::DrawingArea::new();
        sw.set_size_request(12, 12);
        sw.set_valign(gtk::Align::Center);
        sw.connect_draw(move |w, cr| {
            let (used, free) = colors(w);
            let c = if used_color { used } else { free };
            cr.set_source_rgba(c.red(), c.green(), c.blue(), c.alpha());
            cr.rectangle(0.0, 0.0, w.allocated_width() as f64, w.allocated_height() as f64);
            let _ = cr.fill();
            glib::Propagation::Stop
        });
        sw
    };
    let line = |r: i32, sw: Option<gtk::DrawingArea>, key: &str, val: String| {
        if let Some(sw) = sw {
            legend.attach(&sw, 0, r, 1, 1);
        }
        let k = gtk::Label::new(Some(key));
        k.set_xalign(0.0);
        k.style_context().add_class("dim-label");
        legend.attach(&k, 1, r, 1, 1);
        let v = gtk::Label::new(Some(&val));
        v.set_xalign(0.0);
        v.set_selectable(true);
        legend.attach(&v, 2, r, 1, 1);
    };
    line(0, Some(swatch(true)), "Used:", format!("{} ({:.0} %)", util::format_size(used), fraction * 100.0));
    line(1, Some(swatch(false)), "Free:", util::format_size(free));
    line(2, None, "Total:", util::format_size(total));
    if let Some(t) = fs.attribute_string("filesystem::type").filter(|t| !t.is_empty()) {
        line(3, None, "File system:", t.to_string());
    }
    hb.pack_start(&legend, false, false, 0);
    Some(hb.upcast())
}

/// Applies each folder's mode to everything inside it: subfolders get it
/// whole, files its read and write bits beside their own execute bits.
/// Links are left alone. (changed, failed).
fn apply_enclosed(dirs: &[(std::path::PathBuf, u32)]) -> (usize, usize) {
    use std::os::unix::fs::PermissionsExt;
    let (mut changed, mut failed) = (0, 0);
    for (dir, mode) in dirs {
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                failed += 1;
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
                if md.file_type().is_symlink() {
                    continue;
                }
                let old = md.permissions().mode() & 0o7777;
                let new = if md.is_dir() {
                    stack.push(p.clone());
                    *mode
                } else {
                    (old & 0o7111) | (*mode & 0o666)
                };
                if new == old {
                    continue;
                }
                match std::fs::set_permissions(&p, std::fs::Permissions::from_mode(new)) {
                    Ok(()) => changed += 1,
                    Err(_) => failed += 1,
                }
            }
        }
    }
    (changed, failed)
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_media_info() {
        let text = "General\nComplete name                            : /a/b c.mp4\nFormat                                   : MPEG-4\n\nVideo\nID                                       : 1\nFormat/Info                              : Advanced Video Codec : x\n\nEmpty\n";
        let s = super::parse_media_info(text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0, "General");
        assert_eq!(s[0].1, vec![("Complete name".into(), "/a/b c.mp4".into()), ("Format".into(), "MPEG-4".into())]);
        assert_eq!(s[1].1[1], ("Format/Info".to_string(), "Advanced Video Codec : x".to_string()));
    }

    #[test]
    fn random_media_info_text() {
        crate::testutil::cases(3000, |r| {
            let text = (0..r.below(8)).map(|_| r.text(30)).collect::<Vec<_>>().join(if r.chance(2) { " : " } else { "\n" });
            for (section, rows) in super::parse_media_info(&text) {
                assert!(!section.is_empty() && !rows.is_empty());
            }
        });
    }

    #[test]
    fn apply_enclosed_modes() {
        use std::os::unix::fs::PermissionsExt;
        let t = crate::testutil::TestDir::new("props-enclosed");
        let f = t.file("top/a.txt", "a");
        let x = t.file("top/run.sh", "x");
        let sub = t.0.join("top/sub");
        std::fs::create_dir_all(&sub).unwrap();
        let g = t.file("top/sub/b.txt", "b");
        let set = |p: &std::path::Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        set(&f, 0o600);
        set(&x, 0o700);
        set(&g, 0o644);
        set(&sub, 0o700);
        std::os::unix::fs::symlink(&f, t.0.join("top/link")).unwrap();
        let (changed, failed) = super::apply_enclosed(&[(t.0.join("top"), 0o750)]);
        assert_eq!(failed, 0);
        assert_eq!(mode(&sub), 0o750);
        assert_eq!(mode(&f), 0o640);
        // Execute bits stay the file's own.
        assert_eq!(mode(&x), 0o740);
        assert_eq!(mode(&g), 0o640);
        assert_eq!(changed, 4);
    }
}
