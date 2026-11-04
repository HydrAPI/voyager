//! The side pane: Computer, Devices and Bookmarks sections with headings,
//! like Caja's. A section can be put somewhere else by dragging its heading
//! onto another one; bookmarks are reordered the same way.
//!
//! Devices (drives, partitions, phones over MTP, network mounts) are mounted
//! by clicking them, and unmounted / ejected / safely removed with the button
//! next to them or the context menu. Arkha mounts newly inserted media
//! itself (see `init_automount`), independent of the desktop (whose own
//! automounting it switches off), unless that's turned off in the
//! preferences.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gio::prelude::*;
use gtk::prelude::*;

use crate::{config, util};

/// Section ids (what `sidebar_sections` stores) and headings.
pub const SECTIONS: &[(&str, &str)] = &[("computer", "Computer"), ("devices", "Devices"), ("bookmarks", "Bookmarks")];

const SECTION_TARGET: &str = "ARKHA_SIDEBAR_SECTION";
const BOOKMARK_TARGET: &str = "ARKHA_SIDEBAR_BOOKMARK";

#[derive(Clone, Copy, PartialEq)]
pub enum OpenIn {
    Current,
    Tab,
    Window,
}

/// Where the side pane's requests go (the window).
type Handler = Rc<dyn Fn(Request)>;

/// What the side pane asks of its window.
pub enum Request {
    Open(gio::File, OpenIn),
    Drop { uris: Vec<String>, dest: gio::File, action: gdk::DragAction },
    /// A mount went away: leave any folder shown from it.
    Unmounted(gio::File),
    Status(String),
}

#[derive(Clone)]
enum Target {
    Location(gio::File),
    /// Not mounted: clicking mounts it.
    Volume(gio::Volume),
    /// A drive without media (an empty card reader, say): clicking checks.
    Drive(gio::Drive),
}

#[derive(Clone)]
struct Item {
    section: &'static str,
    /// What `hidden_places` stores (devices and bookmarks can't be hidden: None).
    key: Option<String>,
    label: String,
    icon: Option<gio::Icon>,
    tooltip: Option<String>,
    target: Target,
    mount: Option<gio::Mount>,
    volume: Option<gio::Volume>,
    drive: Option<gio::Drive>,
    bookmark: Option<String>,
    trash: bool,
}

enum Row {
    Header(&'static str),
    Item(Item),
}

/// A place that can be hidden in the preferences.
pub struct Place {
    pub key: String,
    pub label: String,
    pub icon: Option<gio::Icon>,
}

pub struct Sidebar {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
    location: RefCell<Option<gio::File>>,
    handler: RefCell<Option<Handler>>,
    monitor: gio::VolumeMonitor,
    signals: RefCell<Vec<glib::SignalHandlerId>>,
    _trash_monitor: RefCell<Option<gio::FileMonitor>>,
    refresh_queued: Cell<bool>,
    /// Where a drop landed, kept from drag-drop until the data arrives.
    drop_y: Cell<i32>,
    /// The row a left button press started on (rows open on release).
    pressed: Cell<Option<usize>>,
    /// Likewise for headings, which collapse / expand their section.
    pressed_header: Cell<Option<&'static str>>,
}

thread_local! {
    static ALL: RefCell<Vec<Weak<Sidebar>>> = const { RefCell::new(vec![]) };
}

/// Refreshes every window's side pane (after a settings change).
pub fn refresh_all() {
    ALL.with(|a| {
        for s in a.borrow().iter().filter_map(Weak::upgrade) {
            s.refresh();
        }
    });
}

impl Sidebar {
    pub fn new() -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.style_context().add_class("sidebar");
        list.style_context().add_class("arkha-sidebar");
        let widget = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        widget.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        widget.style_context().add_class("sidebar");
        widget.add(&list);
        widget.set_size_request(140, -1);
        let s = Rc::new(Sidebar {
            widget,
            list,
            rows: RefCell::new(vec![]),
            location: RefCell::new(None),
            handler: RefCell::new(None),
            monitor: gio::VolumeMonitor::get(),
            signals: RefCell::new(vec![]),
            _trash_monitor: RefCell::new(None),
            refresh_queued: Cell::new(false),
            drop_y: Cell::new(0),
            pressed: Cell::new(None),
            pressed_header: Cell::new(None),
        });
        s.connect();
        s.refresh();
        ALL.with(|a| a.borrow_mut().push(Rc::downgrade(&s)));
        s
    }

    pub fn set_handler(&self, f: impl Fn(Request) + 'static) {
        *self.handler.borrow_mut() = Some(Rc::new(f));
    }

    fn request(&self, r: Request) {
        let h = self.handler.borrow().clone();
        if let Some(h) = h {
            h(r);
        }
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.widget.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok())
    }

    fn connect(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        // Device changes rebuild the list (once, however many arrive).
        let queue = {
            let w = w.clone();
            move || {
                if let Some(s) = w.upgrade() {
                    s.queue_refresh();
                }
            }
        };
        let m = &self.monitor;
        let mut ids = vec![];
        macro_rules! on {
            ($connect:ident) => {{
                let q = queue.clone();
                ids.push(m.$connect(move |_, _| q()));
            }};
        }
        on!(connect_volume_added);
        on!(connect_volume_removed);
        on!(connect_volume_changed);
        on!(connect_mount_added);
        on!(connect_mount_changed);
        on!(connect_drive_connected);
        on!(connect_drive_disconnected);
        on!(connect_drive_changed);
        {
            let w = w.clone();
            ids.push(m.connect_mount_removed(move |_, mount| {
                if let Some(s) = w.upgrade() {
                    s.queue_refresh();
                    s.request(Request::Unmounted(mount.root()));
                }
            }));
        }
        *self.signals.borrow_mut() = ids;
        {
            let w = w.clone();
            self.widget.connect_destroy(move |_| {
                if let Some(s) = w.upgrade() {
                    for id in s.signals.borrow_mut().drain(..) {
                        s.monitor.disconnect(id);
                    }
                }
            });
        }
        // The trash icon shows whether it's empty.
        if let Ok(tm) = gio::File::for_uri("trash:///").monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            let q = queue.clone();
            tm.connect_changed(move |_, _, _, _| q());
            *self._trash_monitor.borrow_mut() = Some(tm);
        }

        // Rows open on clicks themselves (see item_row): the list loses
        // track of clicks after a drag. This is for the keyboard (Enter).
        self.list.set_activate_on_single_click(false);
        {
            let w = w.clone();
            self.list.connect_row_activated(move |_, row| {
                let mouse = gtk::current_event().is_some_and(|e| {
                    matches!(e.event_type(), gdk::EventType::ButtonPress | gdk::EventType::DoubleButtonPress | gdk::EventType::ButtonRelease)
                });
                if let (Some(s), false) = (w.upgrade(), mouse) {
                    s.activate(row.index(), OpenIn::Current);
                }
            });
        }
        self.setup_drop();
    }

    fn queue_refresh(self: &Rc<Self>) {
        if self.refresh_queued.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(s) = w.upgrade() {
                s.refresh_queued.set(false);
                s.refresh();
            }
        });
    }

    /// Rebuilds the list from the places, devices and bookmarks.
    pub fn refresh(self: &Rc<Self>) {
        let cfg = config::get();
        let mut rows = vec![];
        for id in &cfg.sidebar_sections {
            let Some(&(id, _)) = SECTIONS.iter().find(|(s, _)| s == id) else { continue };
            let items: Vec<Item> = self
                .items(id)
                .into_iter()
                .filter(|i| i.key.as_ref().is_none_or(|k| !cfg.hidden_places.contains(k)))
                .collect();
            // A section without anything to show leaves out its heading too.
            if items.is_empty() {
                continue;
            }
            rows.push(Row::Header(id));
            // A collapsed section shows only its heading.
            if !cfg.sidebar_collapsed.iter().any(|c| c == id) {
                rows.extend(items.into_iter().map(Row::Item));
            }
        }
        for c in self.list.children() {
            self.list.remove(&c);
        }
        for (i, r) in rows.iter().enumerate() {
            let row = match r {
                Row::Header(id) => self.header_row(id),
                Row::Item(item) => self.item_row(item, i),
            };
            self.list.add(&row);
        }
        *self.rows.borrow_mut() = rows;
        self.list.show_all();
        self.select_location();
    }

    /// Highlights the row of the folder shown in the window.
    pub fn set_location(&self, f: &gio::File) {
        *self.location.borrow_mut() = Some(f.clone());
        self.select_location();
    }

    fn select_location(&self) {
        let loc = self.location.borrow().clone();
        let rows = self.rows.borrow();
        let hit = loc.and_then(|f| {
            rows.iter().position(|r| matches!(r, Row::Item(Item { target: Target::Location(l), .. }) if l.equal(&f)))
        });
        match hit.and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
    }

    /// The Computer entries, for the preferences (devices and bookmarks
    /// can't be hidden).
    pub fn places(self: &Rc<Self>) -> Vec<Place> {
        self.items("computer")
            .into_iter()
            .filter_map(|i| Some(Place { key: i.key?, label: i.label, icon: i.icon }))
            .collect()
    }

    fn items(&self, section: &'static str) -> Vec<Item> {
        match section {
            "computer" => computer_items(),
            "devices" => self.device_items(),
            _ => bookmark_items(),
        }
    }

    fn device_items(&self) -> Vec<Item> {
        let m = &self.monitor;
        let mut out = vec![];
        for d in m.connected_drives() {
            let vols = d.volumes();
            if vols.is_empty() {
                if d.is_media_removable() && !d.is_media_check_automatic() {
                    let name = d.name().to_string();
                    out.push(Item {
                        icon: Some(d.symbolic_icon()),
                        tooltip: Some(name.clone()),
                        label: name,
                        target: Target::Drive(d.clone()),
                        drive: Some(d.clone()),
                        ..Item::blank("devices")
                    });
                }
                continue;
            }
            for v in vols {
                out.push(volume_item(&v, Some(&d)));
            }
        }
        // Volumes of their own: phones (MTP), cameras, …
        for v in m.volumes() {
            if v.drive().is_none() {
                out.push(volume_item(&v, None));
            }
        }
        // Mounts without a volume: network shares and the like.
        for mount in m.mounts() {
            if mount.is_shadowed() || mount.volume().is_some() {
                continue;
            }
            out.push(mount_item(&mount, None, None));
        }
        out
    }

    fn header_row(self: &Rc<Self>, id: &'static str) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.set_selectable(false);
        row.set_can_focus(false);
        let title = SECTIONS.iter().find(|(s, _)| *s == id).map_or(id, |(_, t)| t);
        let label = gtk::Label::new(None);
        label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.style_context().add_class("dim-label");
        // An arrow in front of the title shows whether the section is open.
        let collapsed = config::get().sidebar_collapsed.iter().any(|c| c == id);
        let arrow = gtk::Image::from_icon_name(Some(if collapsed { "pan-end-symbolic" } else { "pan-down-symbolic" }), gtk::IconSize::Menu);
        arrow.style_context().add_class("dim-label");
        arrow.set_margin_start(4);
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        hb.set_margin_top(8);
        hb.set_margin_bottom(2);
        hb.add(&arrow);
        hb.add(&label);
        let ev = gtk::EventBox::new();
        ev.set_visible_window(false);
        ev.add(&hb);
        ev.set_tooltip_text(Some(if collapsed {
            "Click to show this section, drag onto another heading to move it there"
        } else {
            "Click to hide this section's entries, drag onto another heading to move it there"
        }));
        // A click collapses or expands the section (in every window, and
        // it's remembered); a drag moves it.
        let w = Rc::downgrade(self);
        ev.connect_button_press_event(move |_, e| {
            if let (Some(s), 1, gdk::EventType::ButtonPress) = (w.upgrade(), e.button(), e.event_type()) {
                s.pressed_header.set(Some(id));
            }
            glib::Propagation::Proceed
        });
        let w = Rc::downgrade(self);
        ev.connect_button_release_event(move |_, e| {
            if let (Some(s), 1) = (w.upgrade(), e.button()) {
                if s.pressed_header.take() == Some(id) {
                    config::update(|c| {
                        if c.sidebar_collapsed.iter().any(|x| x == id) {
                            c.sidebar_collapsed.retain(|x| x != id);
                        } else {
                            c.sidebar_collapsed.push(id.to_string());
                        }
                    });
                    refresh_all();
                }
            }
            glib::Propagation::Proceed
        });
        let targets = [gtk::TargetEntry::new(SECTION_TARGET, gtk::TargetFlags::SAME_APP, 0)];
        ev.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::MOVE);
        let w = Rc::downgrade(self);
        ev.connect_drag_begin(move |ev, ctx| {
            if let Some(s) = w.upgrade() {
                s.pressed_header.set(None);
            }
            drag_picture(ev.upcast_ref(), ctx);
        });
        // A custom target, so the id goes as raw bytes.
        ev.connect_drag_data_get(move |_, _, data, _, _| {
            data.set(&data.target(), 8, id.as_bytes());
        });
        row.add(&ev);
        row
    }

    fn item_row(self: &Rc<Self>, item: &Item, index: usize) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.style_context().add_class("sidebar-row");
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hb.set_margin_start(14);
        hb.set_margin_end(4);
        if let Some(icon) = &item.icon {
            let img = gtk::Image::from_gicon(icon, gtk::IconSize::Menu);
            img.style_context().add_class("sidebar-icon");
            hb.add(&img);
        }
        let label = gtk::Label::new(Some(&item.label));
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.set_hexpand(true);
        label.style_context().add_class("sidebar-label");
        hb.add(&label);
        if eject_kind(item).is_some() {
            let b = gtk::Button::from_icon_name(Some("media-eject-symbolic"), gtk::IconSize::Menu);
            b.set_relief(gtk::ReliefStyle::None);
            b.set_can_focus(false);
            // Kept small (see apply_sidebar_css), so device rows aren't
            // taller than the others.
            b.style_context().add_class("sidebar-button");
            b.set_valign(gtk::Align::Center);
            b.set_tooltip_text(Some(match eject_kind(item) {
                Some(Removal::Unmount) => "Unmount",
                Some(Removal::Stop) => "Safely remove",
                _ => "Eject",
            }));
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.eject(index, eject_kind_at(&s, index));
                }
            });
            hb.add(&b);
        }
        // An event box for middle-/right-clicks and dragging bookmarks.
        let ev = gtk::EventBox::new();
        ev.set_visible_window(false);
        ev.add(&hb);
        if let Some(t) = &item.tooltip {
            ev.set_tooltip_text(Some(t));
        }
        let w = Rc::downgrade(self);
        ev.connect_button_press_event(move |_, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            if e.event_type() != gdk::EventType::ButtonPress {
                return glib::Propagation::Proceed;
            }
            match e.button() {
                1 => {
                    s.pressed.set(Some(index));
                    glib::Propagation::Proceed
                }
                2 => {
                    s.activate(index as i32, OpenIn::Tab);
                    glib::Propagation::Stop
                }
                3 => {
                    s.context_menu(index, e);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        let w = Rc::downgrade(self);
        ev.connect_button_release_event(move |_, e| {
            if let (Some(s), 1) = (w.upgrade(), e.button()) {
                if s.pressed.take() == Some(index) {
                    s.activate(index as i32, OpenIn::Current);
                }
            }
            glib::Propagation::Proceed
        });
        if let Some(uri) = item.bookmark.clone() {
            let targets = [gtk::TargetEntry::new(BOOKMARK_TARGET, gtk::TargetFlags::SAME_APP, 0)];
            ev.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::MOVE);
            let w = Rc::downgrade(self);
            ev.connect_drag_begin(move |ev, ctx| {
                // A drag, not a click.
                if let Some(s) = w.upgrade() {
                    s.pressed.set(None);
                }
                drag_picture(ev.upcast_ref(), ctx);
            });
            ev.connect_drag_data_get(move |_, _, data, _, _| {
                data.set(&data.target(), 8, uri.as_bytes());
            });
        }
        row.add(&ev);
        row
    }

    fn item_at(&self, index: i32) -> Option<Item> {
        match self.rows.borrow().get(usize::try_from(index).ok()?)? {
            Row::Item(i) => Some(i.clone()),
            Row::Header(_) => None,
        }
    }

    fn section_at(&self, index: i32) -> Option<&'static str> {
        match self.rows.borrow().get(usize::try_from(index).ok()?)? {
            Row::Item(i) => Some(i.section),
            Row::Header(id) => Some(id),
        }
    }

    fn activate(self: &Rc<Self>, index: i32, how: OpenIn) {
        let Some(item) = self.item_at(index) else { return };
        match item.target {
            Target::Location(f) => self.request(Request::Open(f, how)),
            Target::Volume(v) => self.mount(&v, Some(how)),
            Target::Drive(d) => {
                d.poll_for_media(gio::Cancellable::NONE, |_| {});
            }
        }
    }

    fn mount(self: &Rc<Self>, v: &gio::Volume, then: Option<OpenIn>) {
        let op = gtk::MountOperation::new(self.parent_window().as_ref());
        let w = Rc::downgrade(self);
        let vol = v.clone();
        v.mount(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, move |res| {
            let Some(s) = w.upgrade() else { return };
            match res {
                Ok(()) => {
                    if let (Some(how), Some(m)) = (then, vol.get_mount()) {
                        s.request(Request::Open(m.root(), how));
                    }
                }
                Err(e) => s.error(&format!("Unable to mount “{}”", vol.name()), &e),
            }
        });
    }

    fn error(&self, title: &str, e: &glib::Error) {
        // The user cancelled (in a password dialog, say).
        if e.matches(gio::IOErrorEnum::FailedHandled) || e.matches(gio::IOErrorEnum::Cancelled) {
            return;
        }
        util::error_dialog(self.parent_window().as_ref(), title, e.message());
    }

    fn eject(self: &Rc<Self>, index: usize, how: Option<Removal>) {
        let (Some(item), Some(how)) = (self.item_at(index as i32), how) else { return };
        let op = gtk::MountOperation::new(self.parent_window().as_ref());
        let w = Rc::downgrade(self);
        let name = item.label.clone();
        let done = move |res: Result<(), glib::Error>, verb: &str, ok: Option<String>| {
            let Some(s) = w.upgrade() else { return };
            match res {
                Ok(()) => {
                    if let Some(msg) = ok {
                        s.request(Request::Status(msg));
                    }
                }
                Err(e) => s.error(&format!("Unable to {verb} “{name}”"), &e),
            }
        };
        let flags = gio::MountUnmountFlags::NONE;
        let none = gio::Cancellable::NONE;
        let label = item.label.clone();
        match how {
            Removal::Unmount => {
                if let Some(m) = &item.mount {
                    m.unmount_with_operation(flags, Some(&op), none, move |r| done(r, "unmount", None));
                }
            }
            Removal::Stop => {
                if let Some(d) = &item.drive {
                    let msg = format!("“{}” can be removed safely", d.name());
                    d.stop(flags, Some(&op), none, move |r| done(r, "safely remove", Some(msg)));
                }
            }
            Removal::Eject => {
                let msg = Some(format!("“{label}” was ejected"));
                if let Some(d) = item.drive.as_ref().filter(|d| d.can_eject()) {
                    d.eject_with_operation(flags, Some(&op), none, move |r| done(r, "eject", msg));
                } else if let Some(v) = item.volume.as_ref().filter(|v| v.can_eject()) {
                    v.eject_with_operation(flags, Some(&op), none, move |r| done(r, "eject", msg));
                } else if let Some(m) = &item.mount {
                    m.eject_with_operation(flags, Some(&op), none, move |r| done(r, "eject", msg));
                }
            }
        }
    }

    fn context_menu(self: &Rc<Self>, index: usize, ev: &gdk::EventButton) {
        let Some(item) = self.item_at(index as i32) else { return };
        let menu = gtk::Menu::new();
        let add = |label: &str, f: Box<dyn Fn()>| {
            let mi = gtk::MenuItem::with_mnemonic(label);
            mi.connect_activate(move |_| f());
            menu.append(&mi);
        };
        let sep = || menu.append(&gtk::SeparatorMenuItem::new());
        let w = Rc::downgrade(self);
        let act = |how: OpenIn| -> Box<dyn Fn()> {
            let w = w.clone();
            Box::new(move || {
                if let Some(s) = w.upgrade() {
                    s.activate(index as i32, how);
                }
            })
        };
        let openable = !matches!(item.target, Target::Drive(_));
        if openable {
            add("_Open", act(OpenIn::Current));
            add("Open in New _Tab", act(OpenIn::Tab));
            add("Open in New _Window", act(OpenIn::Window));
        }
        // Devices.
        let mut device = vec![];
        if let Target::Volume(v) = &item.target {
            if v.can_mount() {
                let (w, v) = (w.clone(), v.clone());
                device.push(("_Mount", Box::new(move || {
                    if let Some(s) = w.upgrade() {
                        s.mount(&v, None);
                    }
                }) as Box<dyn Fn()>));
            }
        }
        let ej = |how: Removal| -> Box<dyn Fn()> {
            let w = w.clone();
            Box::new(move || {
                if let Some(s) = w.upgrade() {
                    s.eject(index, Some(how));
                }
            })
        };
        if item.mount.as_ref().is_some_and(|m| m.can_unmount()) {
            device.push(("_Unmount", ej(Removal::Unmount)));
        }
        if removable(&item).is_some_and(|d| d.can_stop()) {
            device.push(("_Safely Remove Drive", ej(Removal::Stop)));
        }
        if can_eject(&item) {
            device.push(("_Eject", ej(Removal::Eject)));
        }
        if let Target::Drive(_) = &item.target {
            device.push(("_Check for Media", act(OpenIn::Current)));
        }
        if !device.is_empty() {
            if openable {
                sep();
            }
            for (l, f) in device {
                add(l, f);
            }
        }
        if let Some(uri) = &item.bookmark {
            sep();
            let (w1, u1, l1) = (w.clone(), uri.clone(), item.label.clone());
            add("Re_name…", Box::new(move || {
                if let Some(s) = w1.upgrade() {
                    s.rename_bookmark(&u1, &l1);
                }
            }));
            let u2 = uri.clone();
            add("_Remove", Box::new(move || {
                edit_bookmarks(|b| b.retain(|(u, _)| *u != u2));
            }));
        }
        if item.trash {
            sep();
            let w = w.clone();
            add("_Empty Trash", Box::new(move || {
                let Some(s) = w.upgrade() else { return };
                if let Some(a) = s
                    .parent_window()
                    .and_then(|t| t.downcast::<gtk::ApplicationWindow>().ok())
                    .and_then(|t| t.lookup_action("empty-trash"))
                {
                    a.activate(None);
                }
            }));
        }
        if let Target::Location(f) = &item.target {
            sep();
            let (w, f) = (w.clone(), f.clone());
            add("_Properties", Box::new(move || {
                if let Some(s) = w.upgrade() {
                    if let Some(p) = s.parent_window() {
                        crate::props::show(&p, vec![f.clone()]);
                    }
                }
            }));
        }
        menu.show_all();
        if let Some(t) = self.parent_window() {
            menu.set_attach_widget(Some(&t));
        }
        menu.popup_at_pointer(Some(ev));
    }

    fn rename_bookmark(&self, uri: &str, current: &str) {
        let d = gtk::Dialog::with_buttons(
            Some("Rename Bookmark"),
            self.parent_window().as_ref(),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("_Cancel", gtk::ResponseType::Cancel), ("_Rename", gtk::ResponseType::Accept)],
        );
        d.set_default_response(gtk::ResponseType::Accept);
        let e = gtk::Entry::new();
        e.set_text(current);
        e.set_activates_default(true);
        e.set_width_chars(30);
        let area = d.content_area();
        area.set_border_width(12);
        area.add(&e);
        d.show_all();
        if d.run() == gtk::ResponseType::Accept {
            let name = e.text().trim().to_string();
            let uri = uri.to_string();
            edit_bookmarks(|b| {
                for (u, l) in b.iter_mut() {
                    if *u == uri {
                        *l = name.clone();
                    }
                }
            });
        }
        d.close();
    }

    // ------------------------------------------------------------ dropping

    fn setup_drop(self: &Rc<Self>) {
        let targets = [
            gtk::TargetEntry::new(SECTION_TARGET, gtk::TargetFlags::SAME_APP, 0),
            gtk::TargetEntry::new(BOOKMARK_TARGET, gtk::TargetFlags::SAME_APP, 1),
            gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 2),
        ];
        let l = &self.list;
        l.drag_dest_set(gtk::DestDefaults::empty(), &targets, gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK | gdk::DragAction::ASK);
        let w = Rc::downgrade(self);
        l.connect_drag_motion(move |l, ctx, _, y, time| {
            let Some(s) = w.upgrade() else { return false };
            let (row, action) = s.drop_spot(ctx, y);
            match &row {
                Some(r) => l.drag_highlight_row(r),
                None => l.drag_unhighlight_row(),
            }
            ctx.drag_status(action, time);
            true
        });
        l.connect_drag_leave(|l, _, _| l.drag_unhighlight_row());
        let w = Rc::downgrade(self);
        l.connect_drag_drop(move |l, ctx, _, y, time| {
            if let Some(s) = w.upgrade() {
                s.drop_y.set(y);
            }
            match l.drag_dest_find_target(ctx, None) {
                Some(t) => l.drag_get_data(ctx, &t, time),
                None => ctx.drag_finish(false, false, time),
            }
            true
        });
        let w = Rc::downgrade(self);
        l.connect_drag_data_received(move |l, ctx, _, _, data, _, time| {
            l.drag_unhighlight_row();
            let Some(s) = w.upgrade() else { return };
            let ok = s.dropped(ctx, s.drop_y.get(), data);
            ctx.drag_finish(ok, false, time);
        });
    }

    fn kind(&self, ctx: &gdk::DragContext) -> Option<String> {
        self.list.drag_dest_find_target(ctx, None).map(|a| a.name().to_string())
    }

    /// The row a drop at `y` would go to (for highlighting) and the action.
    fn drop_spot(&self, ctx: &gdk::DragContext, y: i32) -> (Option<gtk::ListBoxRow>, gdk::DragAction) {
        let none = (None, gdk::DragAction::empty());
        let Some(row) = self.list.row_at_y(y) else { return none };
        let i = row.index();
        let header = |sec: &str| {
            let rows = self.rows.borrow();
            rows.iter().position(|r| matches!(r, Row::Header(h) if *h == sec)).and_then(|p| self.list.row_at_index(p as i32))
        };
        match self.kind(ctx).as_deref() {
            Some(SECTION_TARGET) => match self.section_at(i) {
                Some(sec) => (header(sec), gdk::DragAction::MOVE),
                None => none,
            },
            Some(BOOKMARK_TARGET) => match self.section_at(i) {
                Some("bookmarks") => (Some(row), gdk::DragAction::MOVE),
                _ => none,
            },
            Some(_) => match (self.section_at(i), self.item_at(i)) {
                // Folders dropped on the Bookmarks heading become bookmarks.
                (Some("bookmarks"), None) => (Some(row), gdk::DragAction::LINK),
                // So do folders dropped on a bookmark; with Shift they go
                // into its folder.
                (_, Some(Item { bookmark: Some(_), .. })) if bookmarking(self.list.upcast_ref()) => (Some(row), gdk::DragAction::LINK),
                (_, Some(Item { target: Target::Location(f), .. })) => {
                    let a = crate::pane::choose_action(self.list.upcast_ref(), ctx, &f);
                    (Some(row), a)
                }
                _ => none,
            },
            None => none,
        }
    }

    fn dropped(self: &Rc<Self>, ctx: &gdk::DragContext, y: i32, data: &gtk::SelectionData) -> bool {
        let Some(row) = self.list.row_at_y(y) else { return false };
        let i = row.index();
        match self.kind(ctx).as_deref() {
            Some(SECTION_TARGET) => {
                let (Ok(src), Some(dest)) = (String::from_utf8(data.data()), self.section_at(i)) else { return false };
                if src == dest {
                    return false;
                }
                // The dragged section takes the place of the one it's dropped on.
                config::update(|s| {
                    let at = s.sidebar_sections.iter().position(|x| x == dest).unwrap_or(0);
                    s.sidebar_sections.retain(|x| *x != src);
                    s.sidebar_sections.insert(at.min(s.sidebar_sections.len()), src.clone());
                });
                refresh_all();
                true
            }
            Some(BOOKMARK_TARGET) => {
                let Ok(src) = String::from_utf8(data.data()) else { return false };
                let dest = self.item_at(i).and_then(|it| it.bookmark);
                if dest.as_deref() == Some(src.as_str()) {
                    return false;
                }
                edit_bookmarks(|b| {
                    let at = dest.as_ref().and_then(|d| b.iter().position(|(u, _)| u == d)).unwrap_or(0);
                    let Some(from) = b.iter().position(|(u, _)| *u == src) else { return };
                    let moved = b.remove(from);
                    b.insert(at.min(b.len()), moved);
                });
                true
            }
            Some(_) => {
                let uris: Vec<String> = data.uris().iter().map(|u| u.to_string()).collect();
                if uris.is_empty() {
                    return false;
                }
                match (self.section_at(i), self.item_at(i)) {
                    (Some("bookmarks"), None) => {
                        for u in uris.iter().filter(|u| is_folder(u)) {
                            let _ = util::add_bookmark(u);
                        }
                        true
                    }
                    (_, Some(Item { bookmark: Some(at), .. })) if bookmarking(self.list.upcast_ref()) && uris.iter().all(|u| is_folder(u)) => {
                        // In the dropped-on bookmark's place, in their order.
                        edit_bookmarks(|b| {
                            let mut pos = b.iter().position(|(u, _)| *u == at).unwrap_or(b.len());
                            for u in &uris {
                                if !b.iter().any(|(x, _)| x == u) {
                                    b.insert(pos, (u.clone(), String::new()));
                                    pos += 1;
                                }
                            }
                        });
                        true
                    }
                    (_, Some(Item { target: Target::Location(dest), .. })) => {
                        let action = crate::pane::choose_action(self.list.upcast_ref(), ctx, &dest);
                        self.request(Request::Drop { uris, dest, action });
                        true
                    }
                    _ => false,
                }
            }
            None => false,
        }
    }
}

/// Whether a drop on a bookmark adds bookmarks: without Shift (which
/// drops into its folder), unless what's dragged is known not to be all
/// folders.
fn bookmarking(widget: &gtk::Widget) -> bool {
    !crate::pane::pointer_modifiers(widget).contains(gdk::ModifierType::SHIFT_MASK) && crate::pane::drag_is_folders() != Some(false)
}

fn is_folder(uri: &str) -> bool {
    gio::File::for_uri(uri).query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) == gio::FileType::Directory
}

impl Item {
    fn blank(section: &'static str) -> Item {
        Item {
            section,
            key: None,
            label: String::new(),
            icon: None,
            tooltip: None,
            target: Target::Location(gio::File::for_path("/")),
            mount: None,
            volume: None,
            drive: None,
            bookmark: None,
            trash: false,
        }
    }
}

fn place(f: gio::File, label: &str, icon: &str) -> Item {
    let tooltip = f.path().map(|p| p.to_string_lossy().to_string()).or_else(|| Some(f.uri().to_string()));
    Item {
        key: Some(f.uri().to_string()),
        label: label.to_string(),
        icon: Some(gio::ThemedIcon::new(icon).upcast()),
        tooltip,
        target: Target::Location(f),
        ..Item::blank("computer")
    }
}

/// Home, Desktop, the user's folders (Documents, Music, …), File System and
/// Trash.
fn computer_items() -> Vec<Item> {
    let home = glib::home_dir();
    let mut out = vec![place(gio::File::for_path(&home), "Home", "user-home-symbolic")];
    let mut seen = vec![home.clone()];
    let mut dir = |path: Option<std::path::PathBuf>, icon: Option<&str>, label: Option<&str>| {
        let Some(path) = path else { return };
        if seen.contains(&path) || !path.is_dir() {
            return;
        }
        seen.push(path.clone());
        let f = gio::File::for_path(&path);
        let info = f.query_info("standard::display-name,standard::symbolic-icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
        let name = label.map(str::to_string).or_else(|| info.as_ref().map(|i| i.display_name().to_string())).unwrap_or_default();
        let mut item = place(f, &name, icon.unwrap_or("folder-symbolic"));
        if icon.is_none() {
            if let Some(i) = info.and_then(|i| i.symbolic_icon()) {
                item.icon = Some(i);
            }
        }
        out.push(item);
    };
    use glib::UserDirectory as U;
    dir(glib::user_special_dir(U::Desktop), Some("user-desktop-symbolic"), Some("Desktop"));
    for d in [U::Documents, U::Downloads, U::Music, U::Pictures, U::Videos] {
        dir(glib::user_special_dir(d), None, None);
    }
    // Other folders in user-dirs.dirs (e.g. XDG_PROJECTS_DIR), except the
    // templates and public ones, which GTK leaves out too.
    for p in extra_user_dirs() {
        dir(Some(p), None, None);
    }
    out.push(place(gio::File::for_path("/"), "File System", "drive-harddisk-symbolic"));
    let trash = gio::File::for_uri("trash:///");
    let full = trash
        .query_info("trash::item-count", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .map(|i| i.attribute_uint32("trash::item-count") > 0)
        .unwrap_or(false);
    let mut t = place(trash, "Trash", if full { "user-trash-full-symbolic" } else { "user-trash-symbolic" });
    t.tooltip = Some("Trash".into());
    t.trash = true;
    out.push(t);
    out
}

fn extra_user_dirs() -> Vec<std::path::PathBuf> {
    const KNOWN: &[&str] = &["DESKTOP", "DOWNLOAD", "TEMPLATES", "PUBLICSHARE", "DOCUMENTS", "MUSIC", "PICTURES", "VIDEOS"];
    let Ok(text) = std::fs::read_to_string(glib::user_config_dir().join("user-dirs.dirs")) else { return vec![] };
    let home = glib::home_dir();
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.trim().split_once('=')?;
            let name = k.strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            if KNOWN.contains(&name) {
                return None;
            }
            let v = v.trim().trim_matches('"');
            Some(match v.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => std::path::PathBuf::from(v),
            })
        })
        .collect()
}

fn bookmark_items() -> Vec<Item> {
    util::read_bookmarks()
        .into_iter()
        .filter_map(|(uri, label)| {
            let f = gio::File::for_uri(&uri);
            // Bookmarks of local folders that are gone are left out.
            if f.is_native() && !f.query_exists(gio::Cancellable::NONE) {
                return None;
            }
            let icon = if f.is_native() { "folder-symbolic" } else { "folder-remote-symbolic" };
            let mut item = place(f, &label, icon);
            item.section = "bookmarks";
            item.key = None;
            item.bookmark = Some(uri);
            Some(item)
        })
        .collect()
}

fn volume_item(v: &gio::Volume, d: Option<&gio::Drive>) -> Item {
    if let Some(m) = v.get_mount() {
        return mount_item(&m, Some(v), d);
    }
    let name = v.name().to_string();
    Item {
        icon: Some(v.symbolic_icon()),
        tooltip: Some(format!("{name} (not mounted)")),
        label: name,
        target: Target::Volume(v.clone()),
        volume: Some(v.clone()),
        drive: d.cloned(),
        ..Item::blank("devices")
    }
}

fn mount_item(m: &gio::Mount, v: Option<&gio::Volume>, d: Option<&gio::Drive>) -> Item {
    let name = m.name().to_string();
    let root = m.root();
    let tooltip = root.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| root.uri().to_string());
    Item {
        icon: Some(m.symbolic_icon()),
        tooltip: Some(tooltip),
        label: name,
        target: Target::Location(root),
        mount: Some(m.clone()),
        volume: v.cloned(),
        drive: d.cloned().or_else(|| m.drive()),
        ..Item::blank("devices")
    }
}

#[derive(Clone, Copy)]
enum Removal {
    Unmount,
    /// Power the drive off ("safely remove").
    Stop,
    Eject,
}

/// The item's drive if it's removable (a USB stick, a card reader), not an
/// internal disk, which must never be powered off from here.
fn removable(item: &Item) -> Option<&gio::Drive> {
    item.drive.as_ref().filter(|d| d.is_removable())
}

fn can_eject(item: &Item) -> bool {
    removable(item).is_some_and(|d| d.can_eject())
        || item.volume.as_ref().is_some_and(|v| v.can_eject())
        || item.mount.as_ref().is_some_and(|m| m.can_eject())
}

/// What the button next to a device does: by default it safely removes
/// (or ejects) removable drives; with "only unmount" set, it just unmounts.
fn eject_kind(item: &Item) -> Option<Removal> {
    let can_unmount = item.mount.as_ref().is_some_and(|m| m.can_unmount());
    if config::with(|s| s.unmount_only) {
        return can_unmount.then_some(Removal::Unmount);
    }
    if removable(item).is_some_and(|d| d.can_stop()) {
        Some(Removal::Stop)
    } else if can_eject(item) {
        Some(Removal::Eject)
    } else if can_unmount {
        Some(Removal::Unmount)
    } else {
        None
    }
}

fn eject_kind_at(s: &Sidebar, index: usize) -> Option<Removal> {
    s.item_at(index as i32).as_ref().and_then(eject_kind)
}

/// Drag a picture of the row (or heading).
pub fn drag_picture(w: &gtk::Widget, ctx: &gdk::DragContext) {
    let (width, height) = (w.allocated_width(), w.allocated_height());
    if let Ok(surface) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height) {
        if let Ok(cr) = gtk::cairo::Context::new(&surface) {
            w.draw(&cr);
        }
        surface.set_device_offset(-8.0, -(height as f64) / 2.0);
        ctx.drag_set_icon_surface(&surface);
    }
}

/// Changes the bookmarks file (uri, label; label empty = the folder's name).
/// The windows watch it and update their bookmarks.
fn edit_bookmarks(f: impl FnOnce(&mut Vec<(String, String)>)) {
    let path = util::bookmarks_file();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut b: Vec<(String, String)> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once(' ') {
            Some((u, n)) => (u.to_string(), n.to_string()),
            None => (l.to_string(), String::new()),
        })
        .collect();
    f(&mut b);
    let mut out = String::new();
    for (u, l) in &b {
        out.push_str(u);
        if !l.is_empty() {
            out.push(' ');
            out.push_str(l);
        }
        out.push('\n');
    }
    let _ = std::fs::write(&path, out);
}

/// Where the desktop's automount settings that Arkha switched off are
/// recorded, one per line, for uninstall.sh to switch back on:
/// `gsettings SCHEMA KEY VALUE`, `xfconf CHANNEL PROPERTY VALUE`,
/// `kde FILE GROUP KEY VALUE` or `ini SECTION KEY VALUE PATH` (`-`: the key
/// wasn't set; the path goes last as it may contain spaces).
fn desktop_automount_file() -> std::path::PathBuf {
    glib::user_config_dir().join("arkha").join("desktop-automount")
}

fn record_desktop_automount(lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let path = desktop_automount_file();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for l in lines {
        if !text.lines().any(|x| x == l) {
            text.push_str(l);
            text.push('\n');
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, text);
}

/// Sets each of `keys` in `[section]` of the INI file at `path` to `off`
/// (missing keys count as on), changing nothing else in the file, and notes
/// the old values in `changed`.
fn ini_disable(path: &std::path::Path, section: &str, keys: &[&str], off: &str, changed: &mut Vec<String>) {
    let Ok(mut text) = std::fs::read_to_string(path) else { return };
    let mut any = false;
    for key in keys {
        let old = ini_get(&text, section, key);
        if old.as_deref() == Some(off) {
            continue;
        }
        text = ini_set(&text, section, key, off);
        changed.push(format!("ini {section} {key} {} {}", old.as_deref().unwrap_or("-"), path.display()));
        any = true;
    }
    if any {
        let _ = std::fs::write(path, text);
    }
}

fn ini_section(line: &str) -> Option<&str> {
    line.trim().strip_prefix('[')?.strip_suffix(']')
}

fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        if let Some(s) = ini_section(line) {
            inside = s == section;
        } else if inside {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// `text` with `key=value` in `[section]`: the existing line replaced, or a
/// new one right under the section's header (the section added if needed).
fn ini_set(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = vec![];
    let (mut inside, mut found_section, mut done) = (false, false, false);
    for line in text.lines() {
        if let Some(s) = ini_section(line) {
            inside = s == section;
            out.push(line.to_string());
            if inside {
                found_section = true;
                // Placeholder for a new key, dropped below if it's found.
                out.push(String::from("\0"));
            }
            continue;
        }
        if inside && !done && line.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            out.push(format!("{key}={value}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    let new_line = format!("{key}={value}");
    let mut placed = done;
    let mut out: Vec<String> = out
        .into_iter()
        .filter_map(|l| {
            if l != "\0" {
                return Some(l);
            }
            // Only the first [section] gets the new key.
            if placed {
                None
            } else {
                placed = true;
                Some(new_line.clone())
            }
        })
        .collect();
    if !found_section {
        if out.last().is_some_and(|l| !l.is_empty()) {
            out.push(String::new());
        }
        out.push(format!("[{section}]"));
        out.push(new_line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Desktop environments mount inserted media themselves. Arkha does that
/// (or, set so, doesn't), so theirs is switched off, every start in case
/// it was switched back on.
fn disable_desktop_automount() {
    // GNOME, Cinnamon and MATE (Caja).
    let mut changed = vec![];
    if let Some(src) = gio::SettingsSchemaSource::default() {
        for schema in ["org.gnome.desktop.media-handling", "org.cinnamon.desktop.media-handling", "org.mate.media-handling"] {
            if !src.lookup(schema, true).is_some_and(|s| s.has_key("automount")) {
                continue;
            }
            let s = gio::Settings::new(schema);
            if s.boolean("automount") && s.set_boolean("automount", false).is_ok() {
                changed.push(format!("gsettings {schema} automount true"));
            }
        }
        gio::Settings::sync();
    }
    // LXQt and LXDE: their file managers (PCManFM-Qt, PCManFM) mount media
    // themselves, set in their config files, one per profile. They read
    // these when they start, so this applies from the next login.
    let conf = glib::user_config_dir();
    let profiles = |app: &str, file: &str| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(conf.join(app))
            .map(|d| d.flatten().map(|e| e.path().join(file)).filter(|p| p.is_file()).collect())
            .unwrap_or_default()
    };
    for path in profiles("pcmanfm-qt", "settings.conf") {
        ini_disable(&path, "Volume", &["MountOnStartup", "MountRemovable"], "false", &mut changed);
    }
    for path in profiles("pcmanfm", "pcmanfm.conf") {
        ini_disable(&path, "volume", &["mount_on_startup", "mount_removable"], "0", &mut changed);
    }
    record_desktop_automount(&changed);
    // Xfce (thunar-volman) and KDE keep theirs elsewhere; their tools are
    // run off the main thread.
    let xfconf = glib::find_program_in_path("xfconf-query");
    let kread = glib::find_program_in_path("kreadconfig6").or_else(|| glib::find_program_in_path("kreadconfig5"));
    let kwrite = glib::find_program_in_path("kwriteconfig6").or_else(|| glib::find_program_in_path("kwriteconfig5"));
    if xfconf.is_none() && (kread.is_none() || kwrite.is_none()) {
        return;
    }
    std::thread::spawn(move || {
        let run = |prog: &std::path::Path, args: &[&str]| {
            std::process::Command::new(prog)
                .args(args)
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let mut changed = vec![];
        if let Some(x) = &xfconf {
            for prop in ["/automount-drives/enabled", "/automount-media/enabled"] {
                if run(x, &["-c", "thunar-volman", "-p", prop]).as_deref() == Some("true")
                    && run(x, &["-c", "thunar-volman", "-p", prop, "-s", "false"]).is_some()
                {
                    changed.push(format!("xfconf thunar-volman {prop} true"));
                }
            }
        }
        if let (Some(r), Some(w)) = (&kread, &kwrite) {
            let (file, group, key) = ("kded_device_automounterrc", "General", "AutomountEnabled");
            let cur = run(r, &["--file", file, "--group", group, "--key", key]).unwrap_or_default();
            if cur != "false" && run(w, &["--file", file, "--group", group, "--key", key, "--type", "bool", "false"]).is_some() {
                let was = if cur.is_empty() { "-" } else { cur.as_str() };
                changed.push(format!("kde {file} {group} {key} {was}"));
            }
        }
        record_desktop_automount(&changed);
    });
}

/// Some drives (a VM's virtual CD drive, for one) never report a disc being
/// taken out: the system only notices when the drive is next opened, so the
/// disc stayed in the list. Drives with removable media are therefore opened
/// for a moment every two seconds while they hold something; with O_NONBLOCK
/// that only makes the kernel check, the disc isn't read.
fn poll_removable_media(monitor: &gio::VolumeMonitor) {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let busy = Arc::new(AtomicBool::new(false));
    let monitor = monitor.clone();
    glib::timeout_add_seconds_local(2, move || {
        let devices: Vec<String> = monitor
            .connected_drives()
            .iter()
            .filter(|d| d.is_media_removable() && d.has_volumes())
            .filter_map(|d| d.identifier("unix-device"))
            .map(|s| s.to_string())
            .collect();
        // Off the main thread: a slow drive mustn't freeze the window.
        if !devices.is_empty() && !busy.swap(true, Ordering::SeqCst) {
            let busy = busy.clone();
            std::thread::spawn(move || {
                for dev in devices {
                    let _ = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&dev);
                }
                busy.store(false, Ordering::SeqCst);
            });
        }
        glib::ControlFlow::Continue
    });
}

/// Programs run on their own (often with a plain window manager) that mount
/// inserted media. They have no setting to switch off, only not starting
/// them, so Arkha can't do that for the user; it says so, once for each.
const AUTOMOUNTERS: &[&str] = &["udiskie", "devmon", "ldm", "udisks-glue"];

fn running_automounters() -> Vec<&'static str> {
    let mut found = vec![];
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) else { continue };
        if let Some(a) = AUTOMOUNTERS.iter().find(|a| **a == comm.trim()) {
            if !found.contains(a) {
                found.push(*a);
            }
        }
    }
    found
}

fn notice_automounters() {
    let seen = config::get().automounters_noticed;
    let new: Vec<&str> = running_automounters().into_iter().filter(|a| !seen.iter().any(|s| s == a)).collect();
    if new.is_empty() {
        return;
    }
    config::update(|s| s.automounters_noticed.extend(new.iter().map(|a| a.to_string())));
    let parent = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::ApplicationWindow>().ok())
        .find(|w| w.is_visible());
    let names = new.join(" and ");
    let one = new.len() == 1;
    let d = gtk::MessageDialog::new(
        parent.as_ref(),
        gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Info,
        gtk::ButtonsType::Ok,
        &format!("{names} {} mounting drives too", if one { "is" } else { "are" }),
    );
    d.set_secondary_text(Some(&format!(
        "{} separately from Arkha and {} inserted drives and media by {}, whatever Arkha's settings are, and Arkha can't switch {} off.\n\nTo leave mounting to Arkha, stop {names} from starting with your session (for example, remove {} from your window manager's autostart) and log in again.\n\nThis message isn't shown again.",
        if one { "It runs" } else { "They run" },
        if one { "mounts" } else { "mount" },
        if one { "itself" } else { "themselves" },
        if one { "it" } else { "them" },
        if one { "it" } else { "them" },
    )));
    d.connect_response(|d, _| d.close());
    d.show();
}

/// Mounts removable media when they're inserted (per the settings). Runs
/// once for the whole program; only media inserted while Arkha runs.
pub fn init_automount() {
    thread_local! {
        static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
    }
    MONITOR.with(|m| {
        if m.borrow().is_some() {
            return;
        }
        disable_desktop_automount();
        // Once the first window is up.
        glib::timeout_add_local_once(std::time::Duration::from_secs(2), notice_automounters);
        let monitor = gio::VolumeMonitor::get();
        poll_removable_media(&monitor);
        monitor.connect_volume_added(|_, v| {
            if !config::with(|s| s.automount) || !v.should_automount() || !v.can_mount() || v.get_mount().is_some() {
                return;
            }
            let op = gtk::MountOperation::new(None::<&gtk::Window>);
            // Errors are left alone: another program (the desktop's own
            // automounter) may have mounted it first.
            v.mount(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, |_| {});
        });
        *m.borrow_mut() = Some(monitor);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_editing() {
        let text = "[Behavior]\nSingleClick=false\n\n[Volume]\nAutoRun=true\nMountRemovable=true\n\n[Window]\nWidth=640\n";
        assert_eq!(ini_get(text, "Volume", "MountRemovable").as_deref(), Some("true"));
        assert_eq!(ini_get(text, "Volume", "MountOnStartup"), None);
        assert_eq!(ini_get(text, "Window", "AutoRun"), None);
        // An existing key is replaced in place, a missing one goes under the
        // section's header; nothing else changes.
        let t = ini_set(text, "Volume", "MountRemovable", "false");
        let t = ini_set(&t, "Volume", "MountOnStartup", "false");
        assert_eq!(t, "[Behavior]\nSingleClick=false\n\n[Volume]\nMountOnStartup=false\nAutoRun=true\nMountRemovable=false\n\n[Window]\nWidth=640\n");
        // A missing section is added at the end.
        assert_eq!(ini_set("[desktop]\nwallpaper=x\n", "volume", "mount_removable", "0"), "[desktop]\nwallpaper=x\n\n[volume]\nmount_removable=0\n");
        assert_eq!(ini_set("", "volume", "mount_removable", "0"), "[volume]\nmount_removable=0\n");
    }
}
