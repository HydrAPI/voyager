//! "This PC", as Windows Explorer shows it: the user's folders, the drives
//! as tiles with a bar for how full they are, and the network locations.
//! Double-click (or Enter) opens a tile; a drive that isn't mounted is
//! mounted first.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gio::prelude::*;
use gtk::prelude::*;

use crate::config;
use crate::i18n::{fill, n_, tr};
use crate::util;

type Opener = Rc<dyn Fn(gio::File)>;
/// Takes a drive or place out (unmount, eject, …): started with the mount
/// operation, reports its result to the callback.
type Removal = Rc<dyn Fn(gtk::MountOperation, Box<dyn FnOnce(Result<(), glib::Error>)>)>;

#[derive(Clone)]
enum Action {
    Open(gio::File),
    Mount(gio::Volume),
}

struct TileInfo {
    icon: Option<gio::Icon>,
    name: String,
    /// The file system to read the space of.
    space_of: Option<gio::File>,
    /// What to say when there's no space to show ("Not mounted").
    note: Option<String>,
    action: Action,
    mount: Option<gio::Mount>,
    volume: Option<gio::Volume>,
}

pub struct ThisPc {
    pub widget: gtk::ScrolledWindow,
    content: gtk::Box,
    opener: RefCell<Option<Opener>>,
    count: Cell<usize>,
    /// The tile lists of the sections: only one has a selection.
    lists: RefCell<Vec<gtk::FlowBox>>,
    gen: Cell<u64>,
    monitor: gio::VolumeMonitor,
}

const NETWORK_SCHEMES: &[&str] = &["smb", "sftp", "ssh", "ftp", "ftps", "dav", "davs", "nfs", "afp", "http", "https", "dav+sd", "davs+sd"];

impl ThisPc {
    pub fn new() -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_border_width(14);
        let widget = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        widget.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        widget.style_context().add_class("thispc");
        // A click beside the tiles takes the selection away, as in Explorer.
        let backdrop = gtk::EventBox::new();
        backdrop.set_visible_window(false);
        backdrop.add(&content);
        widget.add(&backdrop);
        let this = Rc::new(ThisPc { widget, content, opener: RefCell::new(None), count: Cell::new(0), lists: RefCell::new(vec![]), gen: Cell::new(0), monitor: gio::VolumeMonitor::get() });
        // Drives coming and going show at once.
        let m = &this.monitor;
        macro_rules! on {
            ($connect:ident) => {{
                let w = Rc::downgrade(&this);
                m.$connect(move |_, _| {
                    if let Some(s) = w.upgrade() {
                        if s.widget.is_mapped() {
                            s.refresh();
                        }
                    }
                });
            }};
        }
        on!(connect_volume_added);
        on!(connect_volume_removed);
        on!(connect_mount_added);
        on!(connect_mount_removed);
        on!(connect_mount_changed);
        {
            let w = Rc::downgrade(&this);
            let ev = backdrop.clone();
            backdrop.connect_button_press_event(move |_, e| {
                let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
                if e.button() != 1 {
                    return glib::Propagation::Proceed;
                }
                let Some(win) = ev.window() else { return glib::Propagation::Proceed };
                let (rx, ry) = e.root();
                let (_, ox, oy) = win.origin();
                let (px, py) = ((rx - ox as f64) as i32, (ry - oy as f64) as i32);
                for fb in s.lists.borrow().iter() {
                    if let Some((x, y)) = ev.translate_coordinates(fb, px, py) {
                        if fb.child_at_pos(x, y).is_some() {
                            return glib::Propagation::Proceed;
                        }
                    }
                }
                for fb in s.lists.borrow().iter() {
                    fb.unselect_all();
                }
                glib::Propagation::Proceed
            });
        }
        this.widget.show_all();
        this
    }

    pub fn set_opener(&self, f: impl Fn(gio::File) + 'static) {
        *self.opener.borrow_mut() = Some(Rc::new(f));
    }

    /// How many tiles it shows (the status bar's number).
    pub fn count(&self) -> usize {
        self.count.get()
    }

    fn open(&self, f: gio::File) {
        let o = self.opener.borrow().clone();
        if let Some(o) = o {
            o(f);
        }
    }

    fn folders() -> Vec<TileInfo> {
        use glib::UserDirectory as U;
        let mut out = vec![];
        for d in [U::Desktop, U::Documents, U::Downloads, U::Music, U::Pictures, U::Videos] {
            let Some(p) = glib::user_special_dir(d).filter(|p| p.is_dir()) else { continue };
            let f = gio::File::for_path(&p);
            let info = f.query_info("standard::display-name,standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
            out.push(TileInfo {
                icon: info.as_ref().and_then(|i| i.icon()),
                name: info.map(|i| i.display_name().to_string()).unwrap_or_else(|| util::display_name(&f)),
                space_of: None,
                note: None,
                action: Action::Open(f),
                mount: None,
                volume: None,
            });
        }
        out
    }

    fn drives(&self) -> (Vec<TileInfo>, Vec<TileInfo>) {
        let (mut local, mut network) = (vec![], vec![]);
        let mut has_root = false;
        for m in self.monitor.mounts() {
            if m.is_shadowed() {
                continue;
            }
            let root = m.root();
            let scheme = root.uri_scheme().map(|s| s.to_string()).unwrap_or_default();
            let tile = TileInfo { icon: Some(m.icon()), name: config::mount_name(&root.uri()).unwrap_or_else(|| m.name().to_string()), space_of: Some(root.clone()), note: None, action: Action::Open(root.clone()), mount: Some(m.clone()), volume: m.volume() };
            if NETWORK_SCHEMES.contains(&scheme.as_str()) {
                network.push(tile);
            } else if root.is_native() || scheme == "file" {
                has_root |= root.path().is_some_and(|p| p == std::path::Path::new("/"));
                local.push(tile);
            }
        }
        for v in self.monitor.volumes() {
            if v.get_mount().is_none() && v.can_mount() {
                local.push(TileInfo { icon: Some(v.icon()), name: v.name().to_string(), space_of: None, note: Some(tr("Not mounted")), action: Action::Mount(v.clone()), mount: None, volume: Some(v) });
            }
        }
        if !has_root {
            let root = gio::File::for_path("/");
            local.insert(
                0,
                TileInfo { icon: Some(gio::ThemedIcon::new("drive-harddisk").upcast()), name: format!("{} (/)", tr("File System")), space_of: Some(root.clone()), note: None, action: Action::Open(root), mount: None, volume: None },
            );
        }
        (local, network)
    }

    /// Rebuilds the page from what's there now.
    pub fn refresh(self: &Rc<Self>) {
        let gen = self.gen.get() + 1;
        self.gen.set(gen);
        self.lists.borrow_mut().clear();
        for c in self.content.children() {
            self.content.remove(&c);
        }
        let folders = Self::folders();
        let (local, network) = self.drives();
        self.count.set(folders.len() + local.len() + network.len());
        self.section(gen, &tr("Folders"), folders, false);
        self.section(gen, &tr("Devices and drives"), local, true);
        self.section(gen, &tr("Network locations"), network, true);
        self.content.show_all();
    }

    fn section(self: &Rc<Self>, gen: u64, title: &str, tiles: Vec<TileInfo>, drive: bool) {
        if tiles.is_empty() {
            return;
        }
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.set_hexpand(true);
        let label = gtk::Label::new(Some(&format!("{title} ({})", tiles.len())));
        label.style_context().add_class("thispc-heading");
        head.pack_start(&label, false, false, 0);
        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_valign(gtk::Align::Center);
        head.pack_start(&sep, true, true, 0);
        let exp = gtk::Expander::new(None);
        exp.set_label_widget(Some(&head));
        exp.set_expanded(true);
        exp.set_hexpand(true);
        exp.set_label_fill(true);
        exp.style_context().add_class("thispc-section");

        let fb = gtk::FlowBox::new();
        fb.style_context().add_class("thispc");
        fb.set_selection_mode(gtk::SelectionMode::Single);
        fb.set_activate_on_single_click(false);
        fb.set_homogeneous(true);
        fb.set_min_children_per_line(1);
        fb.set_max_children_per_line(40);
        fb.set_column_spacing(4);
        fb.set_row_spacing(4);
        fb.set_halign(gtk::Align::Start);
        fb.set_valign(gtk::Align::Start);
        fb.set_margin_top(6);
        fb.set_margin_start(6);
        self.lists.borrow_mut().push(fb.clone());
        {
            let w = Rc::downgrade(self);
            let me = fb.clone();
            fb.connect_selected_children_changed(move |f| {
                if f.selected_children().is_empty() {
                    return;
                }
                if let Some(s) = w.upgrade() {
                    for o in s.lists.borrow().iter().filter(|o| **o != me) {
                        if !o.selected_children().is_empty() {
                            o.unselect_all();
                        }
                    }
                }
            });
        }
        let mut actions = vec![];
        let mut mounts = vec![];
        let mut volumes = vec![];
        for t in tiles {
            let tile = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            tile.set_size_request(if drive { 270 } else { 220 }, -1);
            let img = match &t.icon {
                Some(i) => gtk::Image::from_gicon(i, gtk::IconSize::Dialog),
                None => gtk::Image::from_icon_name(Some("folder"), gtk::IconSize::Dialog),
            };
            img.set_pixel_size(48);
            img.set_valign(gtk::Align::Start);
            tile.pack_start(&img, false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 3);
            col.set_valign(gtk::Align::Center);
            let name = gtk::Label::new(Some(&t.name));
            name.set_xalign(0.0);
            name.set_ellipsize(pango::EllipsizeMode::End);
            col.pack_start(&name, false, false, 0);
            if drive {
                let bar = gtk::ProgressBar::new();
                bar.style_context().add_class("thispc-bar");
                let sub = gtk::Label::new(None);
                sub.set_xalign(0.0);
                sub.style_context().add_class("dim-label");
                col.pack_start(&bar, false, false, 0);
                col.pack_start(&sub, false, false, 0);
                match (&t.space_of, &t.note) {
                    (Some(root), _) => {
                        let root = root.clone();
                        let w = Rc::downgrade(self);
                        sub.set_text("…");
                        glib::MainContext::default().spawn_local(async move {
                            let res = root.query_filesystem_info_future("filesystem::size,filesystem::free", glib::Priority::DEFAULT).await;
                            let Some(s) = w.upgrade() else { return };
                            if s.gen.get() != gen {
                                return;
                            }
                            match res {
                                Ok(info) if info.has_attribute("filesystem::size") && info.attribute_uint64("filesystem::size") > 0 => {
                                    let (size, free) = (info.attribute_uint64("filesystem::size"), info.attribute_uint64("filesystem::free"));
                                    let used = size.saturating_sub(free) as f64 / size as f64;
                                    bar.set_fraction(used);
                                    if used > 0.9 {
                                        bar.style_context().add_class("critical");
                                    }
                                    sub.set_text(&fill(&tr("{free} free of {total}"), &[("free", &util::format_size(free)), ("total", &util::format_size(size))]));
                                }
                                _ => {
                                    bar.set_visible(false);
                                    sub.set_text("");
                                }
                            }
                        });
                    }
                    (None, note) => {
                        bar.set_no_show_all(true);
                        bar.set_visible(false);
                        sub.set_text(note.as_deref().unwrap_or(""));
                    }
                }
            }
            tile.pack_start(&col, true, true, 0);
            tile.set_tooltip_text(Some(&t.name));
            fb.add(&tile);
            actions.push(t.action);
            mounts.push(t.mount);
            volumes.push(t.volume);
        }
        let actions = Rc::new(actions);
        let mounts = Rc::new(mounts);
        let volumes = Rc::new(volumes);
        {
            let w = Rc::downgrade(self);
            let actions = actions.clone();
            fb.connect_child_activated(move |_, child| {
                let (Some(s), Some(a)) = (w.upgrade(), actions.get(child.index() as usize).cloned()) else { return };
                s.activate(a);
            });
        }
        {
            let w = Rc::downgrade(self);
            fb.connect_button_press_event(move |fb, e| {
                if e.button() != 3 {
                    return glib::Propagation::Proceed;
                }
                let (x, y) = e.position();
                let Some(child) = fb.child_at_pos(x as i32, y as i32) else { return glib::Propagation::Proceed };
                fb.select_child(&child);
                let (Some(s), Some(a)) = (w.upgrade(), actions.get(child.index() as usize).cloned()) else { return glib::Propagation::Proceed };
                let menu = gtk::Menu::new();
                let idx = child.index() as usize;
                let mount = mounts.get(idx).cloned().flatten();
                let volume = volumes.get(idx).cloned().flatten();
                let drive = volume.as_ref().and_then(|v| v.drive()).or_else(|| mount.as_ref().and_then(|m| m.drive()));
                let open = gtk::MenuItem::with_mnemonic(&tr("_Open"));
                {
                    let (s, a) = (s.clone(), a.clone());
                    open.connect_activate(move |_| s.activate(a.clone()));
                }
                menu.append(&open);
                if let Some(m) = mount.clone() {
                    let item = gtk::MenuItem::with_mnemonic(&tr("Re_name…"));
                    let w = Rc::downgrade(&s);
                    let current = config::mount_name(&m.root().uri()).unwrap_or_else(|| m.name().to_string());
                    item.connect_activate(move |it| {
                        let parent = it.toplevel().and_downcast::<gtk::Window>();
                        if let Some(name) = crate::sidebar::ask_name(parent.as_ref(), &tr("Rename"), &current) {
                            config::set_mount_name(&m.root().uri(), &name);
                            crate::sidebar::refresh_all();
                            if let Some(s) = w.upgrade() {
                                s.refresh();
                            }
                        }
                    });
                    menu.append(&item);
                }
                // Taking out: always after a warning, and a failure is said.
                let removal = |label: &str, kind: &'static str, what: Removal, name: String| {
                    let item = gtk::MenuItem::with_mnemonic(&tr(label));
                    let w = Rc::downgrade(&s);
                    item.connect_activate(move |it| {
                        let parent = it.toplevel().and_downcast::<gtk::Window>();
                        if !crate::sidebar::removal_warning(parent.as_ref(), &name, kind) {
                            return;
                        }
                        let op = gtk::MountOperation::new(parent.as_ref());
                        let (w, name, parent) = (w.clone(), name.clone(), parent.clone());
                        what(
                            op,
                            Box::new(move |res| {
                                if let Err(e) = res {
                                    if !e.matches(gio::IOErrorEnum::FailedHandled) {
                                        util::error_dialog(parent.as_ref(), &fill(&tr("Unable to remove “{name}”"), &[("name", &name)]), e.message());
                                    }
                                }
                                if let Some(s) = w.upgrade() {
                                    s.refresh();
                                }
                            }),
                        );
                    });
                    item
                };
                let shown_name = mount.as_ref().map(|m| config::mount_name(&m.root().uri()).unwrap_or_else(|| m.name().to_string())).or_else(|| volume.as_ref().map(|v| v.name().to_string())).unwrap_or_default();
                let mut sep_done = false;
                let mut sep = |menu: &gtk::Menu| {
                    if !sep_done {
                        menu.append(&gtk::SeparatorMenuItem::new());
                        sep_done = true;
                    }
                };
                if let Some(m) = mount.clone().filter(|m| m.can_unmount()) {
                    sep(&menu);
                    let what: Removal =
                        Rc::new(move |op, cb| m.unmount_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb));
                    menu.append(&removal(n_("_Unmount"), "unmount", what, shown_name.clone()));
                }
                if let Some(d) = drive.clone().filter(|d| d.is_removable() && d.can_stop()) {
                    sep(&menu);
                    let what: Removal =
                        Rc::new(move |op, cb| d.stop(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb));
                    menu.append(&removal(n_("_Safely Remove Drive"), "stop", what, shown_name.clone()));
                } else if let Some(d) = drive.clone().filter(|d| d.can_eject()) {
                    sep(&menu);
                    let what: Removal =
                        Rc::new(move |op, cb| d.eject_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb));
                    menu.append(&removal(n_("_Eject"), "eject", what, shown_name.clone()));
                }
                if let Some(v) = volume.clone().filter(crate::format::can_format) {
                    sep(&menu);
                    let item = gtk::MenuItem::with_mnemonic(&tr("_Format…"));
                    let w = Rc::downgrade(&s);
                    let name = shown_name.clone();
                    item.connect_activate(move |it| {
                        let parent = it.toplevel().and_downcast::<gtk::Window>();
                        let w = w.clone();
                        let done: Rc<dyn Fn()> = Rc::new(move || {
                            if let Some(s) = w.upgrade() {
                                s.refresh();
                            }
                        });
                        crate::format::show(parent.as_ref(), v.clone(), name.clone(), done);
                    });
                    menu.append(&item);
                }
                if let Some(m) = mount.clone() {
                    menu.append(&gtk::SeparatorMenuItem::new());
                    let item = gtk::MenuItem::with_mnemonic(&tr("_Properties"));
                    item.connect_activate(move |it| {
                        if let Some(p) = it.toplevel().and_downcast::<gtk::Window>() {
                            crate::props::show(&p, vec![m.root()]);
                        }
                    });
                    menu.append(&item);
                }
                menu.show_all();
                menu.popup_at_pointer(Some(e));
                glib::Propagation::Stop
            });
        }
        exp.add(&fb);
        self.content.pack_start(&exp, false, false, 4);
    }

    fn activate(self: &Rc<Self>, a: Action) {
        match a {
            Action::Open(f) => self.open(f),
            Action::Mount(v) => {
                let op = gtk::MountOperation::new(self.widget.toplevel().and_downcast::<gtk::Window>().as_ref());
                let w = Rc::downgrade(self);
                let vol = v.clone();
                v.mount(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, move |res| {
                    let Some(s) = w.upgrade() else { return };
                    match res {
                        Ok(()) => {
                            if let Some(m) = vol.get_mount() {
                                s.open(m.root());
                            } else {
                                s.refresh();
                            }
                        }
                        Err(_) => s.refresh(),
                    }
                });
            }
        }
    }
}
