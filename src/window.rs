//! Main window: menubar, toolbar, location bar, sidebar, tabs, status bar,
//! and all the commands (actions) that operate on the current tab.

use gio::prelude::*;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::actions;
use crate::bulkrename;
use crate::config;
use crate::folders;
use crate::keys;
use crate::thumbnailers;
use crate::icons;
use crate::ops::{self, Item, Job, Kind};
use crate::pane::{Event, ModelGet, Pane, ViewMode};
use crate::{clipboard, props, util};

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<AppWindow>>> = const { RefCell::new(Vec::new()) };
}

pub fn all_windows() -> Vec<Rc<AppWindow>> {
    WINDOWS.with(|w| w.borrow().clone())
}

/// Make every window's menus and toolbar match its current tab again.
fn sync_all_windows() {
    for w in all_windows() {
        if let Some(p) = w.current_pane() {
            w.update_for_pane(&p);
        }
    }
}

/// After the default sorting or the per-folder option changed: re-apply each
/// tab's folder settings, or the global sorting when per-folder views are off.
fn reapply_view_settings() {
    let s = config::get();
    for_all_panes(|p| {
        if s.per_folder {
            p.apply_folder_settings();
        } else {
            p.apply_sort(&s.sort_by, s.sort_desc);
        }
    });
    sync_all_windows();
}

/// The window that holds `pane` now (tabs can move between windows).
const BAR_TARGET: &str = "ARKHA_BAR_GROUP";

thread_local! {
    /// The buttons that drag their bar group (see enable_bar_drag).
    static BAR_DRAGGERS: RefCell<Vec<glib::WeakRef<gtk::Widget>>> = const { RefCell::new(Vec::new()) };
}

/// Lets `button` start a drag of a bar group, unless the bar is locked
/// (View → Lock Toolbar).
fn set_bar_draggable(button: &gtk::Widget, locked: bool) {
    if locked {
        button.drag_source_unset();
    } else {
        let targets = [gtk::TargetEntry::new(BAR_TARGET, gtk::TargetFlags::SAME_APP, 0)];
        button.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::MOVE);
    }
}

/// Locks or unlocks the bar in every window.
fn lock_bars(locked: bool) {
    BAR_DRAGGERS.with(|d| {
        for b in d.borrow().iter().filter_map(|w| w.upgrade()) {
            set_bar_draggable(&b, locked);
        }
    });
}

/// Lets `button`, part of location bar group `id`, be dragged onto another
/// group to move the whole group in front of or behind it.
fn enable_bar_drag(button: &gtk::Widget, id: &'static str, group: &gtk::Widget) {
    set_bar_draggable(button, config::with(|s| s.lock_bar));
    BAR_DRAGGERS.with(|d| {
        let mut d = d.borrow_mut();
        // Drop the buttons of closed windows.
        d.retain(|w| w.upgrade().is_some());
        d.push(button.downgrade());
    });
    {
        let group = group.clone();
        button.connect_drag_begin(move |_, ctx| {
            // For tools/lockbar-test.sh (G_MESSAGES_DEBUG=arkha).
            glib::g_debug!("arkha", "bar drag started: {id}");
            // Drag a picture of the whole group.
            let (w, h) = (group.allocated_width(), group.allocated_height());
            if let Ok(surface) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, w, h) {
                if let Ok(cr) = gtk::cairo::Context::new(&surface) {
                    group.draw(&cr);
                }
                surface.set_device_offset(-(w as f64) / 2.0, -(h as f64) / 2.0);
                ctx.drag_set_icon_surface(&surface);
            }
        });
    }
    // A custom target, so the id goes as raw bytes (set_text is only for
    // text targets).
    button.connect_drag_data_get(move |_, _, data, _, _| {
        data.set(&data.target(), 8, id.as_bytes());
    });
    enable_bar_drop(button, id, group);
}

/// Lets a dragged button group be dropped on `widget`, which is part of
/// location bar item `id` (`item` being the whole item).
fn enable_bar_drop(widget: &gtk::Widget, id: &'static str, item: &gtk::Widget) {
    let targets = [gtk::TargetEntry::new(BAR_TARGET, gtk::TargetFlags::SAME_APP, 0)];
    widget.drag_dest_set(gtk::DestDefaults::ALL, &targets, gdk::DragAction::MOVE);
    let group = item.clone();
    widget.connect_drag_data_received(move |b, _, x, _, data, _, _| {
        let Ok(src) = String::from_utf8(data.data()) else { return };
        if src == id || !config::BAR_GROUPS.contains(&src.as_str()) {
            return;
        }
        // In front of this group or behind it, depending on the half hit.
        let after = b.translate_coordinates(&group, x, 0).is_some_and(|(gx, _)| gx > group.allocated_width() / 2);
        config::update(|s| {
            s.bar_order.retain(|g| *g != src);
            let i = s.bar_order.iter().position(|g| g == id).unwrap_or(0);
            s.bar_order.insert(if after { i + 1 } else { i }, src.clone());
        });
        for w in all_windows() {
            w.apply_bar_order();
        }
    });
}

/// The notebook (pane of a window) a tab's page is in.
fn notebook_of(page: &gtk::Widget) -> Option<gtk::Notebook> {
    page.parent()?.downcast().ok()
}

fn owner_of(pane: &Rc<Pane>) -> Option<Rc<AppWindow>> {
    all_windows().into_iter().find(|w| w.panes.borrow().iter().any(|p| Rc::ptr_eq(p, pane)))
}

fn for_all_panes(f: impl Fn(&Rc<Pane>)) {
    for w in all_windows() {
        for p in w.panes.borrow().iter() {
            f(p);
        }
    }
}

/// What Reopen Closed Tab brings back.
struct ClosedTab {
    place: crate::pane::Place,
    back: Vec<crate::pane::Place>,
    fwd: Vec<crate::pane::Place>,
    position: u32,
}

const MAX_CLOSED_TABS: usize = 25;

#[allow(dead_code)]
pub struct AppWindow {
    pub window: gtk::ApplicationWindow,
    notebook: gtk::Notebook,
    /// Split view's extra pane, shown only while split view is on.
    notebook2: gtk::Notebook,
    split: gtk::Paned,
    /// The pane (notebook) the toolbar, menus and shortcuts act on.
    active_nb: RefCell<gtk::Notebook>,
    /// Set while tabs are moved between our own panes, so the brief moment
    /// with no tabs isn't mistaken for the window being emptied.
    moving_tabs: Cell<bool>,
    sidebar: Rc<crate::sidebar::Sidebar>,
    paned: gtk::Paned,
    back_btn: gtk::Button,
    toolbar_labels: Vec<gtk::Label>,
    path_area: gtk::Box,
    zoom_box: gtk::Box,
    view_box: gtk::Box,
    split_btn: gtk::ToggleButton,
    info_btn: gtk::ToggleButton,
    ops_btn: gtk::ToggleButton,
    ops_bar: gtk::ProgressBar,
    ops_pop: gtk::Popover,
    fwd_btn: gtk::Button,
    up_btn: gtk::Button,
    path_box: gtk::Box,
    path_scroll: gtk::ScrolledWindow,
    crumbs: RefCell<Vec<gio::File>>,
    location_entry: gtk::Entry,
    loc_stack: gtk::Stack,
    search_field: gtk::Entry,
    /// The pane and folder the search bar was opened for (Ctrl+F, the
    /// toolbar's search button) while it's shown without results: as in
    /// Caja it goes away when that pane goes elsewhere.
    search_pane: RefCell<Option<(std::rc::Weak<Pane>, String)>>,
    /// The toolbar's search button: pressed while the search bar shows.
    search_btn: gtk::ToggleButton,
    /// Set while the search widgets are updated to match a pane, so their
    /// handlers leave the pane alone.
    search_syncing: Cell<bool>,
    loc_toggle: gtk::ToggleButton,
    status: gtk::Label,
    statusbar: gtk::Box,
    spinner: gtk::Spinner,
    panes: RefCell<Vec<Rc<Pane>>>,
    /// Recently closed tabs, most recent last, for Reopen Closed Tab.
    closed_tabs: RefCell<Vec<ClosedTab>>,
    bookmarks_menu: gio::Menu,
    templates_menu: gio::Menu,
    /// The Edit menu's Undo item, labelled with what it undoes.
    undo_menu: gio::Menu,
    undo_label: RefCell<Option<String>>,
    /// The folders in a folder (path, names), read for completing the
    /// location typed.
    completion_dir: RefCell<Option<(std::path::PathBuf, Vec<String>)>>,
    /// The location entry's text is being set by us: no completion.
    completing: Cell<bool>,
    _bookmark_monitor: RefCell<Option<gio::FileMonitor>>,
}

impl AppWindow {
    pub fn new(app: &gtk::Application, locations: &[gio::File]) -> Rc<AppWindow> {
        Self::create(app, locations, true)
    }

    /// `home_if_empty`: open the home folder when no locations are given;
    /// otherwise the window starts without tabs (to receive a dragged one).
    fn create(app: &gtk::Application, locations: &[gio::File], home_if_empty: bool) -> Rc<AppWindow> {
        let s = config::get();
        let window = gtk::ApplicationWindow::new(app);
        window.set_default_size(s.width, s.height);
        if s.maximized {
            window.maximize();
        }
        window.set_icon_name(Some("system-file-manager"));

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&vbox);

        // ---------------- toolbar
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        toolbar.set_border_width(3);
        toolbar.style_context().add_class("primary-toolbar");
        // Button texts, kept to show or hide them (Preferences → Display).
        let toolbar_labels: RefCell<Vec<gtk::Label>> = RefCell::new(vec![]);
        let tbtn = |icon: &str, tip: &str, action: &str, label: Option<&str>| {
            let b = gtk::Button::new();
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            bx.pack_start(&gtk::Image::from_icon_name(Some(icon), gtk::IconSize::LargeToolbar), false, false, 0);
            if let Some(l) = label {
                let l = gtk::Label::new(Some(l));
                // Not shown by show_all(); its visibility follows the setting.
                l.set_no_show_all(true);
                l.set_visible(s.toolbar_text);
                bx.pack_start(&l, false, false, 0);
                toolbar_labels.borrow_mut().push(l);
            }
            b.add(&bx);
            b.set_relief(gtk::ReliefStyle::None);
            b.set_tooltip_text(Some(tip));
            b.set_action_name(Some(action));
            b.set_focus_on_click(false);
            b
        };
        let back_btn = tbtn("go-previous", "Go to the previous visited location (right-click for history)", "win.back", Some("Back"));
        let fwd_btn = tbtn("go-next", "Go to the next visited location", "win.forward", Some("Forward"));
        let up_btn = tbtn("go-up", "Open the parent folder", "win.up", None);
        let reload_btn = tbtn("view-refresh", "Reload the current location", "win.reload", None);
        let home_btn = tbtn("go-home", "Open your personal folder", "win.home", None);
        let root_btn = tbtn("drive-harddisk", "Browse the file system", "win.go-root", None);
        // Caja's Search for Files button: shows and hides the search bar.
        let search_btn = gtk::ToggleButton::new();
        search_btn.add(&gtk::Image::from_icon_name(Some("edit-find"), gtk::IconSize::LargeToolbar));
        search_btn.set_relief(gtk::ReliefStyle::None);
        search_btn.set_tooltip_text(Some("Search documents and folders by name"));
        search_btn.set_focus_on_click(false);
        for b in [&back_btn, &fwd_btn, &up_btn] {
            toolbar.pack_start(b, false, false, 0);
        }
        toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 4);
        for b in [&reload_btn, &home_btn, &root_btn] {
            toolbar.pack_start(b, false, false, 0);
        }
        toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 4);
        toolbar.pack_start(&search_btn, false, false, 0);
        let spinner = gtk::Spinner::new();
        toolbar.pack_end(&spinner, false, false, 6);

        // ---------------- location bar
        let locbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        locbar.set_border_width(3);
        let loc_toggle = gtk::ToggleButton::new();
        loc_toggle.add(&gtk::Image::from_icon_name(Some("document-edit-symbolic"), gtk::IconSize::Button));
        loc_toggle.set_tooltip_text(Some("Toggle between button and text-based location bar (Ctrl+L)"));
        loc_toggle.set_focus_on_click(false);
        // The path (edit toggle + breadcrumbs or text) is one item that the
        // button groups can be dragged to either side of.
        let path_area = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        path_area.set_hexpand(true);
        path_area.pack_start(&loc_toggle, false, false, 0);
        locbar.pack_start(&path_area, true, true, 0);
        // Separate buttons 3 px apart, as Caja's path bar.
        let path_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let path_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        path_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        path_scroll.add(&path_box);
        path_scroll.set_hexpand(true);
        let location_entry = gtk::Entry::new();
        location_entry.set_hexpand(true);
        location_entry.set_placeholder_text(Some("Type a path, e.g. ~/Documents or /etc"));
        // Ctrl+F: a search field in place of the path, as in Caja. Typing
        // shows only the matching items of the open folder.
        let search_field = gtk::Entry::new();
        search_field.set_hexpand(true);
        search_field.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("edit-find-symbolic"));
        let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let search_label = gtk::Label::with_mnemonic("_Search:");
        search_label.set_mnemonic_widget(Some(&search_field));
        search_label.set_margin_start(6);
        search_box.pack_start(&search_label, false, false, 0);
        search_box.pack_start(&search_field, true, true, 0);
        let loc_stack = gtk::Stack::new();
        loc_stack.add_named(&path_scroll, "path");
        loc_stack.add_named(&location_entry, "entry");
        loc_stack.add_named(&search_box, "search");
        loc_stack.set_hexpand(true);
        path_area.pack_start(&loc_stack, true, true, 0);
        // zoom + view switcher
        let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zoom_box.style_context().add_class("linked");
        for (icon, act, tip) in [
            ("zoom-out-symbolic", "win.zoom-out", "Zoom out"),
            ("zoom-original-symbolic", "win.zoom-normal", "Normal size"),
            ("zoom-in-symbolic", "win.zoom-in", "Zoom in"),
        ] {
            let b = gtk::Button::from_icon_name(Some(icon), gtk::IconSize::Button);
            b.set_action_name(Some(act));
            b.set_tooltip_text(Some(tip));
            b.set_focus_on_click(false);
            zoom_box.pack_start(&b, false, false, 0);
        }
        locbar.pack_start(&zoom_box, false, false, 3);
        let view_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        view_box.style_context().add_class("linked");
        for (icon, target, tip) in [
            ("view-grid-symbolic", "icon", "Icon View (Ctrl+1)"),
            ("view-list-symbolic", "list", "List View (Ctrl+2)"),
            ("view-continuous-symbolic", "compact", "Compact View (Ctrl+3)"),
            ("view-column-symbolic", "columns", "Columns View (Ctrl+4)"),
        ] {
            let b = gtk::ToggleButton::new();
            // view-column isn't in every theme (Adwaita lacks it).
            let names = [icon, "view-paged-symbolic"];
            b.add(&gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&names), gtk::IconSize::Button));
            b.set_action_name(Some("win.view-mode"));
            b.set_action_target_value(Some(&target.to_variant()));
            b.set_tooltip_text(Some(tip));
            b.set_focus_on_click(false);
            // Clicking the pressed one would release it: the view doesn't
            // change, so GTK doesn't press it again.
            let win = window.downgrade();
            b.connect_toggled(move |b| {
                let current = win
                    .upgrade()
                    .and_then(|w| w.lookup_action("view-mode"))
                    .and_then(|a| a.state())
                    .and_then(|v| v.get::<String>());
                if !b.is_active() && current.as_deref() == Some(target) {
                    b.set_active(true);
                }
            });
            view_box.pack_start(&b, false, false, 0);
        }
        locbar.pack_start(&view_box, false, false, 3);
        let split_btn = gtk::ToggleButton::new();
        split_btn.add(&gtk::Image::from_icon_name(Some("view-dual-symbolic"), gtk::IconSize::Button));
        split_btn.set_action_name(Some("win.split-view"));
        split_btn.set_tooltip_text(Some("Split view: show a second pane"));
        split_btn.set_focus_on_click(false);
        locbar.pack_start(&split_btn, false, false, 3);
        let info_btn = gtk::ToggleButton::new();
        // A window with a panel on its right; fallbacks for themes without it.
        let info_icon = gio::ThemedIcon::from_names(&["sidebar-show-right-symbolic", "view-right-pane-symbolic", "dialog-information-symbolic"]);
        info_btn.add(&gtk::Image::from_gicon(&info_icon, gtk::IconSize::Button));
        info_btn.set_action_name(Some("win.info-panel"));
        info_btn.set_tooltip_text(Some("Info panel: the selected item's picture and details, in this folder (F11)"));
        info_btn.set_focus_on_click(false);
        locbar.pack_start(&info_btn, false, false, 3);
        // File operations, like a browser's downloads button: shown while
        // there are any, with their overall progress; click for the list.
        let ops_btn = gtk::ToggleButton::new();
        ops_btn.set_tooltip_text(Some("File operations"));
        ops_btn.set_focus_on_click(false);
        let ops_inner = gtk::Box::new(gtk::Orientation::Vertical, 1);
        ops_inner.add(&gtk::Image::from_icon_name(Some("edit-copy-symbolic"), gtk::IconSize::Button));
        let ops_bar = gtk::ProgressBar::new();
        let bar_css = gtk::CssProvider::new();
        let _ = bar_css.load_from_data(b"progressbar trough, progressbar progress { min-height: 2px; } progressbar trough { min-width: 16px; }");
        ops_bar.style_context().add_provider(&bar_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        ops_inner.add(&ops_bar);
        ops_btn.add(&ops_inner);
        locbar.pack_start(&ops_btn, false, false, 3);
        let ops_pop = gtk::Popover::new(Some(&ops_btn));
        // This window's own list of the operations (it keeps itself current).
        let ops_slot = ops::new_view();
        ops_slot.set_size_request(420, -1);
        let ops_scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        ops_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        ops_scroll.set_propagate_natural_height(true);
        ops_scroll.set_max_content_height(420);
        ops_scroll.add(&ops_slot);
        let open_win = gtk::Button::with_label("Open in Window");
        open_win.set_relief(gtk::ReliefStyle::None);
        open_win.set_margin_top(4);
        open_win.set_margin_bottom(4);
        let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pop_box.add(&ops_scroll);
        pop_box.add(&gtk::Separator::new(gtk::Orientation::Horizontal));
        pop_box.add(&open_win);
        pop_box.show_all();
        ops_pop.add(&pop_box);
        {
            let pop = ops_pop.clone();
            ops_btn.connect_toggled(move |b| {
                if b.is_active() {
                    pop.popup();
                } else {
                    pop.popdown();
                }
            });
        }
        {
            let btn = ops_btn.clone();
            ops_pop.connect_closed(move |_| btn.set_active(false));
        }
        {
            let pop = ops_pop.clone();
            open_win.connect_clicked(move |_| {
                pop.popdown();
                ops::open_window();
            });
        }
        ops_btn.show_all();
        ops_btn.set_no_show_all(true);
        ops_btn.set_visible(false);
        // Each group can be hidden in Preferences; show_all() leaves them be.
        for (w, on) in [(zoom_box.upcast_ref::<gtk::Widget>(), s.show_zoom_buttons), (view_box.upcast_ref(), s.show_view_buttons), (split_btn.upcast_ref(), s.show_split_button), (info_btn.upcast_ref(), s.show_info_button)] {
            w.show_all();
            w.set_no_show_all(true);
            w.set_visible(on);
        }
        // Any group can be dragged onto another, or onto the path, to
        // rearrange the bar.
        enable_bar_drop(path_area.upcast_ref(), "path", path_area.upcast_ref());
        for (id, group) in [("zoom", zoom_box.clone().upcast::<gtk::Widget>()), ("view", view_box.clone().upcast()), ("split", split_btn.clone().upcast()), ("info", info_btn.clone().upcast()), ("ops", ops_btn.clone().upcast())] {
            let buttons = match group.downcast_ref::<gtk::Box>() {
                Some(b) => b.children(),
                None => vec![group.clone()],
            };
            for b in &buttons {
                enable_bar_drag(b, id, &group);
            }
        }

        // ---------------- body
        let sidebar = crate::sidebar::Sidebar::new();
        apply_sidebar_css();
        // Themes like Adwaita put a margin around each tab; make the tabs
        // touch. In split view the pane without focus is dimmed.
        let css = gtk::CssProvider::new();
        let _ = css.load_from_data(b"notebook > header tabs { padding-left: 0; padding-right: 0; } notebook > header tabs tab { margin-left: 0; margin-right: 0; } notebook.arkha-inactive { opacity: 0.7; }");
        let new_notebook = || {
            let nb = gtk::Notebook::new();
            nb.set_scrollable(true);
            nb.set_show_border(false);
            nb.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            // Shared group: tabs can be dragged between windows (see
            // adopt_page) and panes, or out of a window into a new one.
            nb.set_group_name(Some("arkha-tabs"));
            nb
        };
        let notebook = new_notebook();
        // The extra pane of split view (F3), hidden until it's turned on.
        let notebook2 = new_notebook();
        notebook2.set_no_show_all(true);
        let split = gtk::Paned::new(gtk::Orientation::Horizontal);
        split.pack1(&notebook, true, false);
        split.pack2(&notebook2, true, false);
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.pack1(&sidebar.widget, false, false);
        paned.pack2(&split, true, false);
        paned.set_position(s.sidebar_width);

        // ---------------- status bar
        let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        statusbar.set_border_width(3);
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_ellipsize(pango::EllipsizeMode::End);
        status.set_margin_start(6);
        statusbar.pack_start(&status, true, true, 0);

        let menubar_model = gio::Menu::new();
        let bookmarks_menu = gio::Menu::new();
        let templates_menu = gio::Menu::new();

        let this = Rc::new(AppWindow {
            window: window.clone(),
            active_nb: RefCell::new(notebook.clone()),
            moving_tabs: Cell::new(false),
            notebook,
            notebook2,
            split,
            sidebar,
            paned: paned.clone(),
            back_btn,
            toolbar_labels: toolbar_labels.into_inner(),
            path_area,
            zoom_box,
            view_box,
            split_btn,
            info_btn,
            ops_btn,
            ops_bar,
            ops_pop,
            fwd_btn,
            up_btn,
            path_box,
            path_scroll,
            crumbs: RefCell::new(vec![]),
            location_entry,
            loc_stack,
            search_field,
            search_pane: RefCell::new(None),
            search_btn,
            search_syncing: Cell::new(false),
            loc_toggle,
            status,
            statusbar: statusbar.clone(),
            spinner,
            panes: RefCell::new(vec![]),
            closed_tabs: RefCell::new(vec![]),
            bookmarks_menu,
            templates_menu,
            undo_menu: gio::Menu::new(),
            // Not a label update_undo() would give: the first call sets it.
            undo_label: RefCell::new(Some(String::new())),
            completion_dir: RefCell::new(None),
            completing: Cell::new(false),
            _bookmark_monitor: RefCell::new(None),
        });

        this.build_menubar(&menubar_model);
        let menubar = gtk::MenuBar::from_model(&menubar_model);
        vbox.pack_start(&menubar, false, false, 0);
        vbox.pack_start(&toolbar, false, false, 0);
        vbox.pack_start(&locbar, false, false, 0);
        vbox.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
        vbox.pack_start(&paned, true, true, 0);
        vbox.pack_start(&statusbar, false, false, 0);

        this.apply_bar_order();
        {
            let w = Rc::downgrade(&this);
            let update = move || {
                if let Some(s) = w.upgrade() {
                    s.update_ops_button();
                    s.update_undo();
                }
            };
            ops::on_change(update);
        }
        this.update_ops_button();
        this.setup_actions();
        this.update_undo();
        this.setup_signals();
        this.refresh_bookmarks();
        this.refresh_templates();

        let locs: Vec<gio::File> = if locations.is_empty() && home_if_empty { vec![util::home_file()] } else { locations.to_vec() };
        // Opened in the given order: when tabs go after the current one,
        // switch to each as it's added (the first is shown below).
        let after_current = !s.tabs_at_end;
        for l in &locs {
            this.add_tab(l, after_current);
        }
        if !locs.is_empty() {
            this.notebook.set_current_page(Some(0));
        }
        window.show_all();
        this.sidebar.widget.set_visible(s.show_sidebar);
        this.statusbar.set_visible(s.show_statusbar);
        this.apply_path_mode();
        this.update_tabs_visibility();
        WINDOWS.with(|w| w.borrow_mut().push(this.clone()));
        if let Some(p) = this.current_pane() {
            // The first tab's events came before the window was in WINDOWS
            // (so owner_of() didn't find it): catch up on its state (view
            // mode, cover panel, … in the menus).
            this.update_for_pane(&p);
            p.grab_focus();
        }
        this
    }

    // ================================================================ menus

    fn build_menubar(&self, bar: &gio::Menu) {
        let section = |items: &[(&str, &str)]| {
            let m = gio::Menu::new();
            for (label, action) in items {
                m.append(Some(label), Some(action));
            }
            m
        };
        let file = gio::Menu::new();
        file.append_section(None, &section(&[("New _Tab", "win.new-tab"), ("New _Window", "win.new-window")]));
        let create = gio::Menu::new();
        create.append(Some("Create _Folder"), Some("win.new-folder"));
        create.append_submenu(Some("Create _Document"), &self.templates_menu);
        file.append_section(None, &create);
        file.append_section(None, &section(&[("Open in _Terminal", "win.terminal"), ("_Properties", "win.properties")]));
        file.append_section(None, &section(&[("_Empty Trash", "win.empty-trash")]));
        file.append_section(None, &section(&[("_Close Tab", "win.close-tab"), ("_Reopen Closed Tab", "win.reopen-tab"), ("Close _Window", "win.close-window"), ("_Quit", "app.quit")]));
        bar.append_submenu(Some("_File"), &file);

        let edit = gio::Menu::new();
        self.undo_menu.append(Some("_Undo"), Some("win.undo"));
        edit.append_section(None, &self.undo_menu);
        edit.append_section(None, &section(&[("Cu_t", "win.cut"), ("_Copy", "win.copy"), ("_Paste", "win.paste"), ("Paste _Into Folder", "win.paste-into")]));
        edit.append_section(None, &section(&[("Select _All", "win.select-all"), ("Select I_tems Matching…", "win.select-pattern"), ("_Invert Selection", "win.invert-selection")]));
        edit.append_section(None, &section(&[("Dup_licate", "win.duplicate"), ("Ma_ke Link", "win.make-link"), ("_Rename…", "win.rename"), ("Copy T_o…", "win.copy-to"), ("_Move To…", "win.move-to")]));
        edit.append_section(None, &section(&[("Copy to Other _Pane", "win.copy-to-pane"), ("Move to Other Pa_ne", "win.move-to-pane")]));
        edit.append_section(None, &section(&[("Mo_ve to Trash", "win.trash"), ("_Delete", "win.delete"), ("_Restore", "win.restore")]));
        edit.append_section(None, &section(&[("Pr_eferences", "win.preferences")]));
        bar.append_submenu(Some("_Edit"), &edit);

        let view = gio::Menu::new();
        view.append_section(None, &section(&[("_Reload", "win.reload")]));
        view.append_section(None, &section(&[("_Icons", "win.view-mode::icon"), ("_List", "win.view-mode::list"), ("_Compact", "win.view-mode::compact"), ("C_olumns", "win.view-mode::columns")]));
        view.append_section(None, &section(&[("Show _Hidden Files", "win.show-hidden"), ("_Side Pane", "win.show-sidebar"), ("E_xtra Pane", "win.split-view"), ("St_atusbar", "win.show-statusbar"), ("_Info Panel", "win.info-panel")]));
        let sort = gio::Menu::new();
        sort.append_section(None, &section(&[("By _Name", "win.sort-by::name"), ("By _Size", "win.sort-by::size"), ("By _Type", "win.sort-by::type"), ("By Modification _Date", "win.sort-by::mtime")]));
        sort.append_section(None, &section(&[("_Reversed Order", "win.sort-desc"), ("_Folders Before Files", "win.folders-first")]));
        view.append_submenu(Some("Arrange _Items"), &sort);
        view.append_section(None, &section(&[("Zoom _In", "win.zoom-in"), ("Zoom _Out", "win.zoom-out"), ("Normal Si_ze", "win.zoom-normal")]));
        view.append_section(None, &section(&[("Loc_k Toolbar", "win.lock-bar")]));
        bar.append_submenu(Some("_View"), &view);

        let go = gio::Menu::new();
        go.append_section(None, &section(&[("_Back", "win.back"), ("_Forward", "win.forward"), ("_Open Parent", "win.up")]));
        go.append_section(None, &section(&[("_Home Folder", "win.home"), ("_Desktop", "win.go-desktop"), ("File _System", "win.go-root"), ("_Trash", "win.go-trash")]));
        go.append_section(None, &section(&[("_Location…", "win.location"), ("_Filter…", "win.filter")]));
        bar.append_submenu(Some("_Go"), &go);

        let bm = gio::Menu::new();
        bm.append_section(None, &section(&[("_Add Bookmark", "win.add-bookmark"), ("_Edit Bookmarks…", "win.edit-bookmarks")]));
        bm.append_section(None, &self.bookmarks_menu);
        bar.append_submenu(Some("_Bookmarks"), &bm);

        let help = gio::Menu::new();
        help.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
        help.append(Some("_About"), Some("win.about"));
        bar.append_submenu(Some("_Help"), &help);
    }

    fn refresh_bookmarks(&self) {
        self.bookmarks_menu.remove_all();
        for (uri, label) in util::read_bookmarks() {
            let item = gio::MenuItem::new(Some(&label.replace('_', "__")), None);
            item.set_action_and_target_value(Some("win.go-uri"), Some(&uri.to_variant()));
            self.bookmarks_menu.append_item(&item);
        }
        self.sidebar.refresh();
    }

    fn refresh_templates(&self) {
        self.templates_menu.remove_all();
        let empty = gio::Menu::new();
        empty.append(Some("_Empty Document"), Some("win.new-file"));
        self.templates_menu.append_section(None, &empty);
        let tm = gio::Menu::new();
        if let Some(dir) = glib::user_special_dir(glib::UserDirectory::Templates) {
            if dir != glib::home_dir() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    let mut names: Vec<_> = rd.flatten().filter(|e| e.path().is_file()).map(|e| e.path()).collect();
                    names.sort();
                    for p in names {
                        let label = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        let item = gio::MenuItem::new(Some(&label.replace('_', "__")), None);
                        item.set_action_and_target_value(Some("win.new-from-template"), Some(&p.to_string_lossy().to_string().to_variant()));
                        tm.append_item(&item);
                    }
                }
            }
        }
        self.templates_menu.append_section(None, &tm);
    }

    fn open_with_menu(&self, sel: &[crate::pane::RowInfo]) -> gio::Menu {
        let m = gio::Menu::new();
        if let Some(first) = sel.first() {
            let same_type = sel.iter().all(|r| r.ctype == first.ctype);
            if same_type {
                let apps = gio::AppInfo::all_for_type(&first.ctype);
                let default = gio::AppInfo::default_for_type(&first.ctype, false);
                let mut seen = std::collections::HashSet::new();
                for app in default.into_iter().chain(apps) {
                    let Some(id) = app.id() else { continue };
                    if !seen.insert(id.to_string()) {
                        continue;
                    }
                    let item = gio::MenuItem::new(Some(&app.display_name().replace('_', "__")), None);
                    item.set_action_and_target_value(Some("win.open-with"), Some(&id.to_string().to_variant()));
                    m.append_item(&item);
                }
            }
        }
        let other = gio::Menu::new();
        other.append(Some("Other _Application…"), Some("win.open-with-other"));
        m.append_section(None, &other);
        m
    }

    fn context_menu(self: &Rc<Self>, pane: &Rc<Pane>, event: Option<gdk::Event>, on_item: bool) {
        let menu = gio::Menu::new();
        let cfg = config::get();
        let shown = |key: &str| !cfg.hidden_menu_items.iter().any(|h| h == key);
        let sec = |items: &[(&str, &str, &str)]| {
            let m = gio::Menu::new();
            for (k, l, a) in items {
                if shown(k) {
                    m.append(Some(l), Some(a));
                }
            }
            m
        };
        let sel = pane.selected();
        let in_trash = pane.is_trash();
        let on_sel = on_item && !sel.is_empty();
        // Every entry that applies here, by key; the order set in the
        // preferences (actions::menu_order) then lays them out.
        let mut entries: std::collections::HashMap<String, gio::MenuItem> = Default::default();
        let mut add = |k: &str, label: &str, action: &str| {
            entries.insert(k.to_string(), gio::MenuItem::new(Some(label), Some(action)));
        };
        let mut submenus: Vec<(&str, &str, gio::Menu)> = vec![];
        if on_sel {
            let all_dirs = sel.iter().all(|r| r.is_dir);
            if in_trash {
                add("restore", "_Restore", "win.restore");
                add("cut", "Cu_t", "win.cut");
                add("copy", "_Copy", "win.copy");
                add("delete", "_Delete Permanently", "win.delete");
            } else {
                if all_dirs {
                    add("open", "_Open", "win.open");
                    add("open-new-tab", "Open in New _Tab", "win.open-new-tab");
                    add("open-new-window", "Open in New _Window", "win.open-new-window");
                } else {
                    let label = gio::AppInfo::default_for_type(&sel[0].ctype, false)
                        .filter(|_| sel.iter().all(|r| r.ctype == sel[0].ctype))
                        .map(|a| format!("_Open With {}", a.display_name().replace('_', "__")))
                        .unwrap_or_else(|| "_Open".into());
                    add("open", &label, "win.open");
                }
                // Caja's entry in search results.
                if pane.is_searching() && sel.len() == 1 {
                    add("open-parent", "Open parent location", "win.open-parent");
                }
                if shown("open-with") {
                    submenus.push(("open-with", "Open Wit_h", self.open_with_menu(&sel)));
                }
                add("cut", "Cu_t", "win.cut");
                add("copy", "_Copy", "win.copy");
                if sel.len() == 1 && all_dirs {
                    add("paste-into", "Paste _Into Folder", "win.paste-into");
                }
                add("duplicate", "D_uplicate", "win.duplicate");
                add("make-link", "Ma_ke Link", "win.make-link");
                add("rename", "_Rename…", "win.rename");
                // The bulk rename window: "Rename All…" for several items
                // (as in Caja's caja-rename), "Advanced Rename…" for one.
                add("rename-all", if sel.len() > 1 { "Rename _All…" } else { "Advanced Re_name…" }, "win.rename-all");
                add("copy-to", "Copy T_o…", "win.copy-to");
                add("move-to", "_Move To…", "win.move-to");
                add("trash", "Mo_ve to Trash", "win.trash");
                add("delete", "_Delete", "win.delete");
                if util::archiver_command().is_some() {
                    if sel.iter().all(|r| util::is_archive(&r.ctype)) {
                        add("extract", "E_xtract Here", "win.extract");
                    }
                    add("compress", "_Compress…", "win.compress");
                }
                if sel.len() == 1 && all_dirs {
                    add("terminal", "Open in _Terminal", "win.terminal");
                }
            }
            add("properties", "_Properties", "win.properties");
        } else {
            if in_trash {
                add("bg:empty-trash", "_Empty Trash", "win.empty-trash");
            } else {
                add("bg:new-folder", "Create _Folder", "win.new-folder");
                if shown("bg:new-document") {
                    submenus.push(("bg:new-document", "Create _Document", self.templates_menu.clone()));
                }
                add("bg:terminal", "Open in _Terminal", "win.terminal");
                add("bg:paste", "_Paste", "win.paste");
                // The menu bar's View menu without Reload, zooming and
                // Arrange Items (their own entries); each entry can be hidden.
                if shown("bg:view") {
                    let sub = gio::Menu::new();
                    sub.append_section(None, &sec(&[("bg:view-icons", "_Icons", "win.view-mode::icon"), ("bg:view-list", "_List", "win.view-mode::list"), ("bg:view-compact", "_Compact", "win.view-mode::compact"), ("bg:view-columns", "C_olumns", "win.view-mode::columns")]));
                    sub.append_section(None, &sec(&[("bg:show-hidden", "Show _Hidden Files", "win.show-hidden"), ("bg:view-sidepane", "_Side Pane", "win.show-sidebar"), ("bg:view-extrapane", "E_xtra Pane", "win.split-view"), ("bg:view-statusbar", "St_atusbar", "win.show-statusbar")]));
                    // Left out if every entry in it is hidden.
                    if (0..sub.n_items()).any(|i| sub.item_link(i, "section").is_some_and(|m| m.n_items() > 0)) {
                        submenus.push(("bg:view", "_View", sub));
                    }
                }
                if shown("bg:arrange") {
                    let arrange = gio::Menu::new();
                    for (l, a) in [("By _Name", "win.sort-by::name"), ("By _Size", "win.sort-by::size"), ("By _Type", "win.sort-by::type"), ("By Modification _Date", "win.sort-by::mtime")] {
                        arrange.append(Some(l), Some(a));
                    }
                    let more = gio::Menu::new();
                    more.append(Some("_Reversed Order"), Some("win.sort-desc"));
                    more.append(Some("_Folders Before Files"), Some("win.folders-first"));
                    arrange.append_section(None, &more);
                    submenus.push(("bg:arrange", "Arrange _Items", arrange));
                }
                add("bg:add-bookmark", "Add _Bookmark", "win.add-bookmark");
            }
            add("bg:select-all", "Select _All", "win.select-all");
            add("bg:reload", "_Reload", "win.reload");
            add("bg:properties", "_Properties", "win.properties");
        }
        for (k, label, sub) in submenus {
            entries.insert(k.to_string(), gio::MenuItem::new_submenu(Some(label), &sub));
        }
        // Custom actions, only for local folders: their commands get paths.
        let custom = actions::load();
        if !in_trash && pane.current().path().is_some() && sel.iter().all(|r| !on_sel || gio::File::for_uri(&r.uri).path().is_some()) {
            let picked: Vec<(String, bool, String)> = if on_sel { sel.iter().map(|r| (r.name.clone(), r.is_dir, r.ctype.clone())).collect() } else { vec![] };
            for (i, a) in custom.iter().enumerate() {
                if actions::applies(a, &picked) {
                    let target = format!("{i}{}", if on_sel { "" } else { "/bg" });
                    let item = gio::MenuItem::new(Some(&a.name.replace('_', "__")), None);
                    item.set_action_and_target_value(Some("win.custom-action"), Some(&target.to_variant()));
                    entries.insert(actions::custom_key(a), item);
                }
            }
        }
        let order = if on_sel {
            let keys: Vec<String> = custom.iter().filter(|a| a.files || a.folders).map(actions::custom_key).collect();
            actions::menu_order(&cfg.menu_order, actions::ITEM_ORDER, &keys)
        } else {
            let keys: Vec<String> = custom.iter().filter(|a| a.background).map(actions::custom_key).collect();
            actions::menu_order(&cfg.menu_order_bg, actions::BACKGROUND_ORDER, &keys)
        };
        // Sections between separators; empty ones vanish, so separators
        // around entries that don't apply here never double up.
        let mut section = gio::Menu::new();
        for k in &order {
            if k == actions::SEPARATOR {
                if section.n_items() > 0 {
                    menu.append_section(None, &section);
                    section = gio::Menu::new();
                }
            } else if shown(k) {
                if let Some(item) = entries.remove(k) {
                    section.append_item(&item);
                }
            }
        }
        if section.n_items() > 0 {
            menu.append_section(None, &section);
        }
        self.update_action_states();
        let m = gtk::Menu::from_model(&menu);
        m.set_attach_widget(Some(&pane.active_view()));
        m.show_all();
        match event {
            // A right-click in a column of the columns view that's gone
            // since (it led to another folder): at the pointer, in the view.
            Some(e) if e.window().is_some_and(|w| w.is_destroyed()) => {
                let view = pane.active_view();
                let pointer = view.display().default_seat().and_then(|s| s.pointer());
                match (view.window(), pointer) {
                    (Some(win), Some(ptr)) if e.event_type() == gdk::EventType::ButtonPress => {
                        // The same click, moved to the view (GTK wants the
                        // click that opened a menu).
                        let (_, x, y, _) = win.device_position(&ptr);
                        let mut click = e.clone();
                        unsafe {
                            let b = glib::translate::ToGlibPtrMut::<*mut gdk::ffi::GdkEvent>::to_glib_none_mut(&mut click).0 as *mut gdk::ffi::GdkEventButton;
                            if !(*b).window.is_null() {
                                glib::gobject_ffi::g_object_unref((*b).window as *mut _);
                            }
                            (*b).window = glib::translate::ToGlibPtr::<*mut gdk::ffi::GdkWindow>::to_glib_full(&win);
                            (*b).x = x as f64;
                            (*b).y = y as f64;
                        }
                        m.popup_at_pointer(Some(&click));
                    }
                    _ => m.popup_at_widget(&view, gdk::Gravity::Center, gdk::Gravity::NorthWest, None),
                }
            }
            Some(e) => m.popup_at_pointer(Some(&e)),
            None => m.popup_at_widget(&pane.active_view(), gdk::Gravity::Center, gdk::Gravity::NorthWest, None),
        }
    }

    /// Runs custom action `target` ("index", or "index/bg" for the menu of the
    /// folder's empty space) on the current tab's selection.
    fn run_custom_action(self: &Rc<Self>, target: &str) {
        let (idx, bg) = match target.strip_suffix("/bg") {
            Some(i) => (i, true),
            None => (target, false),
        };
        let (Some(a), Some(p)) = (idx.parse::<usize>().ok().and_then(|i| actions::load().into_iter().nth(i)), self.current_pane()) else { return };
        let Some(dir) = p.current().path() else { return };
        let sel: Vec<std::path::PathBuf> = if bg { vec![] } else { p.selected_files().iter().filter_map(|f| f.path()).collect() };
        let cmd = actions::expand(&a.command, &sel, &dir);
        if let Err(e) = util::spawn_in_dir(&[actions::shell(), "-c".into(), cmd], Some(&dir)) {
            util::error_dialog(Some(self.window.upcast_ref()), &format!("Could not run “{}”", a.name), &e);
        }
    }

    // ================================================================ actions

    fn add(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<AppWindow>) + 'static) {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(s) = w.upgrade() {
                f(&s);
            }
        });
        self.window.add_action(&a);
    }

    fn add_str(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<AppWindow>, String) + 'static) {
        let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, p| {
            if let (Some(s), Some(p)) = (w.upgrade(), p.and_then(|p| p.get::<String>())) {
                f(&s, p);
            }
        });
        self.window.add_action(&a);
    }

    fn add_toggle(self: &Rc<Self>, name: &str, init: bool, f: impl Fn(&Rc<AppWindow>, bool) + 'static) {
        let a = gio::SimpleAction::new_stateful(name, None, &init.to_variant());
        let w = Rc::downgrade(self);
        a.connect_activate(move |a, _| {
            let v = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
            a.set_state(&v.to_variant());
            if let Some(s) = w.upgrade() {
                f(&s, v);
            }
        });
        self.window.add_action(&a);
    }

    fn add_radio(self: &Rc<Self>, name: &str, init: &str, f: impl Fn(&Rc<AppWindow>, String) + 'static) {
        let a = gio::SimpleAction::new_stateful(name, Some(glib::VariantTy::STRING), &init.to_variant());
        let w = Rc::downgrade(self);
        a.connect_activate(move |a, p| {
            let Some(v) = p.and_then(|p| p.get::<String>()) else { return };
            a.set_state(&v.to_variant());
            if let Some(s) = w.upgrade() {
                f(&s, v);
            }
        });
        self.window.add_action(&a);
    }

    fn action(&self, name: &str) -> Option<gio::SimpleAction> {
        self.window
            .lookup_action(name)
            .and_then(|a| a.downcast::<gio::SimpleAction>().ok())
    }

    /// Labels Edit → Undo with what it would undo, greyed out with nothing.
    fn update_undo(&self) {
        let label = ops::undo_label();
        if *self.undo_label.borrow() == label {
            return;
        }
        self.undo_menu.remove_all();
        self.undo_menu.append(Some(label.as_deref().unwrap_or("_Undo")), Some("win.undo"));
        self.set_enabled("undo", label.is_some());
        *self.undo_label.borrow_mut() = label;
    }

    fn set_enabled(&self, name: &str, on: bool) {
        if let Some(a) = self.action(name) {
            a.set_enabled(on);
        }
    }

    fn set_state(&self, name: &str, v: glib::Variant) {
        if let Some(a) = self.action(name) {
            a.set_state(&v);
        }
    }

    fn setup_actions(self: &Rc<Self>) {
        let s = config::get();
        self.add("new-tab", |w| {
            let f = w.current_pane().map(|p| p.current()).unwrap_or_else(util::home_file);
            w.add_tab(&f, true);
        });
        self.add("new-window", |w| {
            let f = w.current_pane().map(|p| p.current()).unwrap_or_else(util::home_file);
            if let Some(app) = w.window.application() {
                AppWindow::new(&app, &[f]);
            }
        });
        self.add("close-tab", |w| {
            if let Some(p) = w.current_pane() {
                w.close_tab(&p);
            }
        });
        self.add("reopen-tab", |w| w.reopen_tab());
        self.set_enabled("reopen-tab", false); // until a tab is closed
        self.add("close-window", |w| w.window.close());
        self.add("open", |w| w.open_selection(OpenMode::Normal));
        self.add("open-new-tab", |w| w.open_selection(OpenMode::Tab));
        self.add("open-new-window", |w| w.open_selection(OpenMode::Window));
        self.add_str("custom-action", |w, target| w.run_custom_action(&target));
        self.add_str("open-with", |w, id| w.open_with(&id));
        self.add("open-with-other", |w| w.open_with_other());
        self.add("new-folder", |w| w.new_folder());
        self.add("new-file", |w| w.new_file(None));
        self.add_str("new-from-template", |w, path| w.new_file(Some(path)));
        self.add("cut", |w| w.copy_selection(true));
        self.add("copy", |w| w.copy_selection(false));
        self.add("undo", |w| {
            if let Err(why) = ops::undo(Some(w.window.clone().upcast())) {
                w.flash_status(&why);
            }
        });
        self.add("paste", |w| {
            if let Some(p) = w.current_pane() {
                w.paste_into(p.current());
            }
        });
        self.add("paste-into", |w| {
            if let Some(p) = w.current_pane() {
                if let Some(r) = p.selected().into_iter().find(|r| r.is_dir) {
                    w.paste_into(gio::File::for_uri(&r.uri));
                }
            }
        });
        self.add("select-all", |w| {
            if let Some(p) = w.current_pane() {
                p.select_all();
            }
        });
        self.add("invert-selection", |w| {
            if let Some(p) = w.current_pane() {
                p.invert_selection();
            }
        });
        self.add("select-pattern", |w| w.select_pattern_dialog());
        self.add("rename-all", |w| {
            if let Some(p) = w.current_pane() {
                let files = p.selected_files();
                if !files.is_empty() {
                    bulkrename::show(w.window.upcast_ref(), files);
                }
            }
        });
        self.add("rename", |w| {
            if let Some(p) = w.current_pane() {
                let sel = p.selected();
                match sel.len() {
                    0 => {}
                    1 => p.start_rename(&sel[0].uri),
                    // Several items: the Rename key opens bulk renaming.
                    _ => bulkrename::show(w.window.upcast_ref(), p.selected_files()),
                }
            }
        });
        self.add("duplicate", |w| w.transfer_selection_here(Kind::Copy));
        self.add("make-link", |w| w.transfer_selection_here(Kind::Link));
        self.add("copy-to", |w| w.transfer_to_chosen(Kind::Copy));
        self.add("move-to", |w| w.transfer_to_chosen(Kind::Move));
        self.add("copy-to-pane", |w| w.transfer_to_other_pane(Kind::Copy));
        self.add("move-to-pane", |w| w.transfer_to_other_pane(Kind::Move));
        self.add("trash", |w| w.trash_selection());
        self.add("delete", |w| w.delete_selection());
        self.add("restore", |w| w.restore_selection());
        self.add("empty-trash", |w| w.empty_trash());
        self.add("properties", |w| {
            if let Some(p) = w.current_pane() {
                let mut files = p.selected_files();
                if files.is_empty() {
                    files = vec![p.current()];
                }
                props::show(w.window.upcast_ref(), files);
            }
        });
        self.add("terminal", |w| w.open_terminal());
        self.add("extract", |w| w.archive(true));
        self.add("compress", |w| w.archive(false));
        self.add("back", |w| {
            if let Some(p) = w.current_pane() {
                p.go_back();
            }
        });
        self.add("forward", |w| {
            if let Some(p) = w.current_pane() {
                p.go_forward();
            }
        });
        self.add("up", |w| {
            if let Some(p) = w.current_pane() {
                p.go_up();
            }
        });
        self.add("home", |w| w.go(&util::home_file()));
        self.add("go-root", |w| w.go(&gio::File::for_path("/")));
        self.add("go-trash", |w| w.go(&gio::File::for_uri("trash:///")));
        self.add("go-desktop", |w| {
            let d = glib::user_special_dir(glib::UserDirectory::Desktop).unwrap_or_else(glib::home_dir);
            w.go(&gio::File::for_path(d));
        });
        self.add_str("go-uri", |w, uri| w.go(&gio::File::for_uri(&uri)));
        self.add("reload", |w| {
            if let Some(p) = w.current_pane() {
                p.reload();
            }
        });
        self.add("location", |w| w.show_location_entry(None));
        // Ctrl+F shows the search bar (as in Caja it never hides it: the
        // toolbar's button does).
        self.add("filter", |w| w.show_search());
        self.add("escape", |w| {
            w.loc_toggle.set_active(false);
            let searching = w.current_pane().is_some_and(|p| p.is_searching());
            if w.search_open() && !searching {
                w.hide_search();
            }
            if let Some(p) = w.current_pane() {
                p.grab_focus();
            }
        });
        self.add("open-parent", |w| {
            let Some(p) = w.current_pane() else { return };
            if let [r] = p.selected().as_slice() {
                p.open_parent_location(&r.uri);
            }
        });
        self.add("zoom-in", |w| {
            if let Some(p) = w.current_pane() {
                p.zoom(1);
            }
        });
        self.add("zoom-out", |w| {
            if let Some(p) = w.current_pane() {
                p.zoom(-1);
            }
        });
        self.add("zoom-normal", |w| {
            if let Some(p) = w.current_pane() {
                p.zoom_reset();
            }
        });
        self.add("add-bookmark", |w| {
            if let Some(p) = w.current_pane() {
                let uri = p.current().uri().to_string();
                match util::add_bookmark(&uri) {
                    Ok(true) => {}
                    Ok(false) => w.flash_status("This location is already bookmarked"),
                    Err(e) => util::error_dialog(Some(w.window.upcast_ref()), "Could not add bookmark", &e.to_string()),
                }
            }
        });
        self.add("edit-bookmarks", |w| w.edit_bookmarks());
        // Around from the last tab to the first and back, as in browsers.
        self.add("next-tab", |w| {
            let nb = w.active();
            let n = nb.n_pages();
            if n > 0 {
                nb.set_current_page(Some((nb.current_page().unwrap_or(0) + 1) % n));
            }
        });
        self.add("prev-tab", |w| {
            let nb = w.active();
            let n = nb.n_pages();
            if n > 0 {
                nb.set_current_page(Some((nb.current_page().unwrap_or(0) + n - 1) % n));
            }
        });
        for i in 1..=9u32 {
            self.add(&format!("tab-{i}"), move |w| {
                let nb = w.active();
                let n = if i == 9 { nb.n_pages().saturating_sub(1) } else { i - 1 };
                nb.set_current_page(Some(n));
            });
        }
        self.add("preferences", |w| w.preferences());
        self.add("about", |w| w.about());
        self.add("shortcuts", |w| w.shortcuts());

        self.add_toggle("show-hidden", s.show_hidden, |_, v| {
            config::update(|s| s.show_hidden = v);
            for w in all_windows() {
                w.set_state("show-hidden", v.to_variant());
            }
            for_all_panes(|p| p.set_show_hidden(v));
        });
        // The info panel, shown per folder (see Pane::set_info_panel).
        self.add_toggle("info-panel", false, |w, v| {
            if let Some(p) = w.current_pane() {
                p.set_info_panel(v);
            }
        });
        self.add_toggle("show-sidebar", s.show_sidebar, |w, v| {
            config::update(|s| s.show_sidebar = v);
            w.sidebar.widget.set_visible(v);
        });
        self.add_toggle("focus-follows-mouse", s.focus_follows_mouse, |_, v| {
            config::update(|s| s.focus_follows_mouse = v);
            // One setting: every window's switch shows it.
            for w in all_windows() {
                if let Some(a) = w.window.lookup_action("focus-follows-mouse").and_downcast::<gio::SimpleAction>() {
                    a.set_state(&v.to_variant());
                }
            }
        });
        self.add_toggle("lock-bar", s.lock_bar, |_, v| {
            config::update(|s| s.lock_bar = v);
            lock_bars(v);
            // One setting: every window's menu shows it.
            for w in all_windows() {
                if let Some(a) = w.window.lookup_action("lock-bar").and_downcast::<gio::SimpleAction>() {
                    a.set_state(&v.to_variant());
                }
            }
        });
        self.add_toggle("show-statusbar", s.show_statusbar, |w, v| {
            config::update(|s| s.show_statusbar = v);
            w.statusbar.set_visible(v);
        });
        self.add_toggle("sort-desc", s.sort_desc, |w, v| {
            if let Some(p) = w.current_pane() {
                let (by, _) = p.sort_state();
                w.user_sort(&p, &by, v);
            }
        });
        self.add_toggle("split-view", false, |w, v| w.set_split(v));
        self.add_toggle("folders-first", s.folders_first, |_, v| {
            config::update(|s| s.folders_first = v);
            for_all_panes(|p| p.set_folders_first(v));
        });
        self.add_radio("view-mode", &s.default_view, |w, v| {
            // Only this tab (and, with per-folder views, this folder); the
            // default for new tabs is set in Preferences.
            if let Some(p) = w.current_pane() {
                if config::with(|s| s.per_folder) {
                    folders::set_view(&p.current().uri(), &v);
                }
                p.set_mode(ViewMode::from_str(&v));
                p.grab_focus();
            }
        });
        self.add_radio("sort-by", &s.sort_by, |w, v| {
            if let Some(p) = w.current_pane() {
                let (_, desc) = p.sort_state();
                w.user_sort(&p, &v, desc);
            }
        });

        // accelerators (user-changeable, see keys.rs)
        if let Some(app) = self.window.application() {
            keys::apply(&app);
        }
    }

    fn setup_signals(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        // Let text entries handle their own keys (Ctrl+C, Delete, ...) before accelerators.
        self.window.connect_key_press_event(|win, ev| {
            if util::is_editable_focused(win.upcast_ref()) {
                let key = ev.keyval();
                // Escape goes to the shortcuts (closing the filter or the
                // location bar), except in the icon views' rename editor,
                // where it cancels renaming (as a shortcut it would move the
                // focus away, which renames).
                let is_escape = key == gdk::keys::constants::Escape;
                // Ctrl+Z in a text field never undoes a file operation.
                let is_undo = ev.state().contains(gdk::ModifierType::CONTROL_MASK) && matches!(key.to_lower(), gdk::keys::constants::z);
                if is_undo {
                    win.propagate_key_event(ev);
                    return glib::Propagation::Stop;
                }
                let in_text_view = win.focused_widget().is_some_and(|w| w.is::<gtk::TextView>());
                if (!is_escape || in_text_view) && win.propagate_key_event(ev) {
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        // While typing to select an item, Backspace and Escape edit or end
        // that instead of doing their shortcuts (parent folder, …): the
        // window handles shortcuts before the view gets the key.
        {
            let w = w.clone();
            self.window.connect_key_press_event(move |_, ev| {
                match w.upgrade().and_then(|s| s.current_pane()) {
                    Some(p) if p.typeahead_active() => p.typeahead_key(ev),
                    _ => glib::Propagation::Proceed,
                }
            });
        }
        // Mouse back/forward buttons anywhere in the window.
        {
            let w = w.clone();
            self.window.connect_button_press_event(move |_, ev| {
                if let Some(s) = w.upgrade() {
                    match ev.button() {
                        8 => s.window.activate_action("back", None),
                        9 => s.window.activate_action("forward", None),
                        _ => return glib::Propagation::Proceed,
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        // Back-button history popup on right-click.
        {
            let w = w.clone();
            self.back_btn.connect_button_press_event(move |b, ev| {
                if ev.button() != 3 {
                    return glib::Propagation::Proceed;
                }
                let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
                let Some(p) = s.current_pane() else { return glib::Propagation::Proceed };
                let menu = gtk::Menu::new();
                for (i, f) in p.history_back().iter().take(15).enumerate() {
                    let item = gtk::MenuItem::with_label(&f.title());
                    let pw = Rc::downgrade(&p);
                    item.connect_activate(move |_| {
                        if let Some(p) = pw.upgrade() {
                            p.go_back_n(i + 1);
                        }
                    });
                    menu.append(&item);
                }
                menu.show_all();
                menu.set_attach_widget(Some(b));
                menu.popup_at_widget(b, gdk::Gravity::SouthWest, gdk::Gravity::NorthWest, Some(ev));
                glib::Propagation::Stop
            });
        }
        for nb in [self.notebook.clone(), self.notebook2.clone()] {
            self.setup_notebook_signals(&nb);
        }
        // Focus moving into a pane (e.g. clicking in it) makes it active.
        {
            let w = w.clone();
            self.window.connect_set_focus(move |_, focus| {
                let (Some(s), Some(focus)) = (w.upgrade(), focus) else { return };
                for nb in [s.notebook.clone(), s.notebook2.clone()] {
                    if nb != s.active() && focus.is_ancestor(&nb) {
                        s.set_active(&nb);
                    }
                }
            });
        }
        // Sidebar.
        {
            let w = w.clone();
            self.sidebar.set_handler(move |r| {
                let Some(s) = w.upgrade() else { return };
                match r {
                    crate::sidebar::Request::Open(f, how) => match how {
                        crate::sidebar::OpenIn::Current => s.go(&f),
                        // Like a browser: a middle-clicked place opens in the
                        // background and the current tab stays.
                        crate::sidebar::OpenIn::Tab => {
                            s.add_tab(&f, false);
                        }
                        crate::sidebar::OpenIn::Window => {
                            if let Some(app) = s.window.application() {
                                AppWindow::new(&app, &[f]);
                            }
                        }
                    },
                    crate::sidebar::Request::Drop { uris, dest, action } => s.handle_drop(uris, dest, action),
                    // Leave folders of a device that was unmounted or removed.
                    crate::sidebar::Request::Unmounted(root) => {
                        let panes = s.panes.borrow().clone();
                        for p in panes {
                            let cur = p.current();
                            if cur.equal(&root) || cur.has_prefix(&root) {
                                p.navigate(&gio::File::for_path(glib::home_dir()));
                            }
                        }
                    }
                    crate::sidebar::Request::Status(text) => s.flash_status(&text),
                }
            });
        }
        // The search bar (Ctrl+F), as in Caja: Enter (or its icon) searches
        // the folder and everything below it; on the results it searches
        // again, the text typed meanwhile only changing their title.
        {
            let w = w.clone();
            self.search_field.connect_changed(move |e| {
                let Some(s) = w.upgrade() else { return };
                if s.search_syncing.get() {
                    return;
                }
                if let Some(p) = s.current_pane().filter(|p| p.is_searching()) {
                    p.set_search_text(&e.text());
                    s.window.set_title(&p.title());
                }
            });
        }
        {
            let w = w.clone();
            self.search_field.connect_activate(move |e| {
                let Some(s) = w.upgrade() else { return };
                let text = e.text();
                let Some(p) = s.current_pane() else { return };
                if text.is_empty() {
                    return;
                }
                if p.is_searching() {
                    p.set_search_text(&text);
                    p.reload();
                } else {
                    p.start_search(&text);
                }
            });
            self.search_field.connect_icon_release(|e, pos, _| {
                if pos == gtk::EntryIconPosition::Secondary {
                    e.emit_activate();
                }
            });
        }
        {
            let w = w.clone();
            self.search_btn.connect_toggled(move |b| {
                let Some(s) = w.upgrade() else { return };
                if s.search_syncing.get() {
                    return;
                }
                if b.is_active() {
                    s.show_search();
                    return;
                }
                // Off: from the results back to the folder searched, as in Caja.
                if let Some(p) = s.current_pane().filter(|p| p.is_searching()) {
                    p.navigate(&p.current());
                }
                s.hide_search();
            });
        }
        // Location bar.
        {
            let w = w.clone();
            self.loc_toggle.connect_toggled(move |t| {
                if let Some(s) = w.upgrade() {
                    if t.is_active() {
                        s.loc_stack.set_visible_child_name("entry");
                        if let Some(p) = s.current_pane() {
                            s.set_location_text(&location_text(&p.current()));
                        }
                        s.location_entry.grab_focus();
                        s.location_entry.select_region(0, -1);
                    } else {
                        // With the path always shown as text, the field stays.
                        if config::with(|s| s.path_as_text) {
                            if let Some(p) = s.current_pane() {
                                s.set_location_text(&location_text(&p.current()));
                            }
                            s.location_entry.select_region(0, 0);
                        } else {
                            s.loc_stack.set_visible_child_name("path");
                        }
                        if let Some(p) = s.current_pane() {
                            p.grab_focus();
                        }
                    }
                }
            });
        }
        {
            let w = w.clone();
            self.location_entry.connect_activate(move |e| {
                let Some(s) = w.upgrade() else { return };
                let text = e.text().to_string();
                let text = text.trim();
                if text.is_empty() {
                    return;
                }
                let f = parse_location(text, s.current_pane().map(|p| p.current()));
                match f.query_info("standard::type", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                    Ok(info) if info.file_type() == gio::FileType::Directory => {
                        s.go(&f);
                        s.loc_toggle.set_active(false);
                        if let Some(p) = s.current_pane() {
                            p.grab_focus();
                        }
                    }
                    Ok(_) => {
                        // A file: open its folder and select it.
                        if let (Some(parent), Some(p)) = (f.parent(), s.current_pane()) {
                            p.navigate(&parent);
                            p.select_uris_later(vec![f.uri().to_string()]);
                            s.loc_toggle.set_active(false);
                            p.grab_focus();
                        }
                    }
                    Err(err) => {
                        if !f.is_native() {
                            s.go(&f); // let gvfs try (e.g. sftp://)
                            s.loc_toggle.set_active(false);
                        } else {
                            util::error_dialog(Some(s.window.upcast_ref()), &format!("Could not find “{text}”"), err.message());
                        }
                    }
                }
            });
        }
        {
            let w = w.clone();
            // As in Caja: typing at the end completes the folder name, the
            // added part selected; Tab or → takes it.
            self.location_entry.connect_insert_text(move |_, _, _| {
                let Some(s) = w.upgrade() else { return };
                if s.completing.get() {
                    return;
                }
                let ws = Rc::downgrade(&s);
                glib::idle_add_local_once(move || {
                    if let Some(s) = ws.upgrade() {
                        s.complete_location();
                    }
                });
            });
            self.location_entry.connect_key_press_event(|e, ev| {
                if ev.keyval() == gdk::keys::constants::Tab && ev.state() & gtk::accelerator_get_default_mod_mask() == gdk::ModifierType::empty() {
                    if let Some((_, end)) = e.selection_bounds() {
                        if end == e.text_length() as i32 {
                            e.set_position(-1);
                            return glib::Propagation::Stop;
                        }
                    }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = w.clone();
            self.location_entry.connect_focus_out_event(move |_, _| {
                if let Some(s) = w.upgrade() {
                    let ws = Rc::downgrade(&s);
                    glib::idle_add_local_once(move || {
                        if let Some(s) = ws.upgrade() {
                            if !s.location_entry.has_focus() && s.window.is_active() {
                                s.loc_toggle.set_active(false);
                            }
                        }
                    });
                }
                glib::Propagation::Proceed
            });
        }
        // Bookmarks file changes (shared with Caja / GTK).
        let bf = gio::File::for_path(util::bookmarks_file());
        if let Ok(m) = bf.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            let w = w.clone();
            m.connect_changed(move |_, _, _, _| {
                if let Some(s) = w.upgrade() {
                    s.refresh_bookmarks();
                }
            });
            *self._bookmark_monitor.borrow_mut() = Some(m);
        }
        // Save geometry, clean up.
        {
            let w = w.clone();
            self.window.connect_delete_event(move |win, _| {
                if let Some(s) = w.upgrade() {
                    let (width, height) = win.size();
                    let max = win.is_maximized();
                    let sw = s.paned.position();
                    config::update(|c| {
                        if !max {
                            c.width = width;
                            c.height = height;
                        }
                        c.maximized = max;
                        c.sidebar_width = sw;
                    });
                }
                glib::Propagation::Proceed
            });
        }
        self.window.connect_destroy(move |win| {
            WINDOWS.with(|ws| {
                ws.borrow_mut().retain(|s| {
                    if &s.window == win {
                        for p in s.panes.borrow().iter() {
                            p.shutdown();
                        }
                        false
                    } else {
                        true
                    }
                })
            });
            // The last window closed while operations still run: show them
            // in their own window. They keep Arkha running, and it exits
            // when they're done (the window closes with the last one).
            if WINDOWS.with(|ws| ws.borrow().is_empty()) && ops::is_busy() {
                ops::open_window();
            }
        });
        {
            let w = w.clone();
            self.window.connect_is_active_notify(move |_| {
                if let Some(s) = w.upgrade() {
                    s.update_action_states();
                }
            });
        }
    }

    // ================================================================ tabs

    pub fn add_tab(self: &Rc<Self>, file: &gio::File, switch: bool) -> Rc<Pane> {
        let pane = Pane::new(file);
        // Tabs can move between windows, so these handlers look up the window
        // that holds the tab now instead of remembering this one.
        let wpane = Rc::downgrade(&pane);
        pane.set_handler(move |ev| {
            if let Some(p) = wpane.upgrade() {
                if let Some(s) = owner_of(&p) {
                    s.on_pane_event(&p, ev);
                }
            }
        });
        // In split view, the pane under the pointer can take focus, once the
        // pointer has stayed there a moment: passing over it on the way to
        // the side pane, say, doesn't count.
        {
            let wp = Rc::downgrade(&pane);
            let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
            pane.connect_pointer_hover(move |inside| {
                let taken = pending.borrow_mut().take();
                if let Some(id) = taken {
                    id.remove();
                }
                if !inside || !config::with(|s| s.focus_follows_mouse) {
                    return;
                }
                let wp = wp.clone();
                let focus = move || {
                    let Some(p) = wp.upgrade() else { return };
                    let (Some(s), Some(nb)) = (owner_of(&p), notebook_of(p.root.upcast_ref())) else { return };
                    // Not while typing a location or a new name.
                    if s.split_on() && nb != s.active() && !util::is_editable_focused(s.window.upcast_ref()) {
                        p.grab_focus();
                    }
                };
                let delay = config::with(|s| s.focus_follows_mouse_delay).max(0) as u64;
                if delay == 0 {
                    focus();
                } else {
                    let done = pending.clone();
                    *pending.borrow_mut() = Some(glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                        done.borrow_mut().take();
                        focus();
                    }));
                }
            });
        }
        let tab = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab.pack_start(&pane.tab_label, true, true, 0);
        let close = gtk::Button::from_icon_name(Some("window-close-symbolic"), gtk::IconSize::Menu);
        close.set_relief(gtk::ReliefStyle::None);
        close.set_focus_on_click(false);
        close.set_tooltip_text(Some("Close tab"));
        {
            let wp = Rc::downgrade(&pane);
            close.connect_clicked(move |_| {
                if let Some(p) = wp.upgrade() {
                    if let Some(s) = owner_of(&p) {
                        s.close_tab(&p);
                    }
                }
            });
        }
        tab.pack_start(&close, false, false, 0);
        let ev_box = gtk::EventBox::new();
        ev_box.add(&tab);
        {
            let wp = Rc::downgrade(&pane);
            ev_box.connect_button_press_event(move |_, ev| {
                if ev.button() == 2 {
                    if let Some(p) = wp.upgrade() {
                        if let Some(s) = owner_of(&p) {
                            s.close_tab(&p);
                        }
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        ev_box.show_all();
        self.panes.borrow_mut().push(pane.clone());
        let nb = self.active();
        // At the end of the tab bar (the default), or right after the
        // current tab.
        let after = if config::with(|s| s.tabs_at_end) { None } else { self.current_pane().and_then(|c| nb.page_num(&c.root)) };
        let idx = match after {
            Some(i) => nb.insert_page(&pane.root, Some(&ev_box), Some(i + 1)),
            None => nb.append_page(&pane.root, Some(&ev_box)),
        };
        self.set_tab_options(pane.root.upcast_ref());
        pane.navigate_initial();
        if switch {
            nb.set_current_page(Some(idx));
            pane.grab_focus();
        }
        self.update_tabs_visibility();
        pane
    }

    fn active(&self) -> gtk::Notebook {
        self.active_nb.borrow().clone()
    }

    fn split_on(&self) -> bool {
        self.notebook2.is_visible()
    }

    /// Makes `nb` the pane the toolbar, menus and shortcuts act on.
    fn set_active(self: &Rc<Self>, nb: &gtk::Notebook) {
        *self.active_nb.borrow_mut() = nb.clone();
        self.update_pane_dimming();
        if let Some(p) = self.current_pane() {
            self.update_for_pane(&p);
        }
    }

    fn update_pane_dimming(&self) {
        let (split, active) = (self.split_on(), self.active());
        for nb in [&self.notebook, &self.notebook2] {
            let ctx = nb.style_context();
            if split && *nb != active {
                ctx.add_class("arkha-inactive");
            } else {
                ctx.remove_class("arkha-inactive");
            }
        }
    }

    /// Signals of one pane's notebook: tabs dragged between panes and windows
    /// or out into a new window, switching tabs, and focus activating it.
    fn setup_notebook_signals(self: &Rc<Self>, nb: &gtk::Notebook) {
        let w = Rc::downgrade(self);
        {
            let w = w.clone();
            nb.connect_page_added(move |_, page, _| {
                if let Some(s) = w.upgrade() {
                    s.adopt_page(page);
                }
            });
        }
        {
            let w = w.clone();
            nb.connect_page_removed(move |_, _, _| {
                if let Some(s) = w.upgrade() {
                    s.after_page_removed();
                }
            });
        }
        // A pane with one tab hides its tab bar, which then can't take a
        // drop; show every tab bar while a tab is being dragged.
        nb.connect_drag_begin(|_, _| {
            for w in all_windows() {
                w.notebook.set_show_tabs(true);
                w.notebook2.set_show_tabs(true);
            }
        });
        nb.connect_drag_end(|_, _| {
            glib::idle_add_local_once(|| {
                for w in all_windows() {
                    w.update_tabs_visibility();
                }
            });
        });
        {
            let w = w.clone();
            nb.connect_create_window(move |_, _, x, y| {
                // The notebook only signals while its window exists.
                let app = w.upgrade().and_then(|s| s.window.application()).expect("window of a running app");
                let nw = AppWindow::create(&app, &[], false);
                nw.window.move_(x, y);
                nw.notebook.clone()
            });
        }
        // GTK 3 on Wayland reports a tab dropped where nothing takes it as
        // an error, not as dropped on nothing (gdkselection-wayland.c), so
        // GtkNotebook never opens a window for it: that's done here then,
        // as it would. (The window can't be put where the tab was dropped.)
        {
            let w = w.clone();
            nb.connect_drag_failed(move |nb, _, result| {
                if result != gtk::DragResult::Error || nb.display().type_().name() == "GdkX11Display" {
                    return glib::Propagation::Proceed;
                }
                let (Some(s), Some(page)) = (w.upgrade(), nb.nth_page(nb.current_page())) else { return glib::Propagation::Proceed };
                let Some(app) = s.window.application() else { return glib::Propagation::Proceed };
                let label = nb.tab_label(&page);
                let nw = AppWindow::create(&app, &[], false);
                nb.detach_tab(&page);
                // adopt_page then takes the pane over, as for a dragged tab.
                nw.notebook.append_page(&page, label.as_ref());
                glib::Propagation::Stop
            });
        }
        {
            let w = w.clone();
            nb.connect_switch_page(move |nb, page, _| {
                let Some(s) = w.upgrade() else { return };
                if *nb != s.active() {
                    // Switching tabs in the other pane makes it the active one.
                    if s.split_on() {
                        let nb = nb.clone();
                        let s = s.clone();
                        glib::idle_add_local_once(move || s.set_active(&nb));
                    }
                    return;
                }
                let pane = s.panes.borrow().iter().find(|p| p.root.upcast_ref::<gtk::Widget>() == page).cloned();
                if let Some(p) = pane {
                    s.update_for_pane(&p);
                }
            });
        }
    }

    /// A tab left one of the panes (closed, or dragged elsewhere).
    fn after_page_removed(self: &Rc<Self>) {
        if self.moving_tabs.get() {
            return;
        }
        let (n1, n2) = (self.notebook.n_pages(), self.notebook2.n_pages());
        if n1 == 0 && n2 == 0 {
            // Its last tab went to another window. Close once the move has
            // finished.
            let win = self.window.clone();
            glib::idle_add_local_once(move || win.close());
            return;
        }
        if self.split_on() && (n1 == 0 || n2 == 0) {
            let s = self.clone();
            glib::idle_add_local_once(move || s.collapse_split());
        }
        self.update_tabs_visibility();
    }

    /// Turns split view on (the extra pane opens at the current folder) or
    /// off (closing the extra pane's tabs).
    fn set_split(self: &Rc<Self>, on: bool) {
        if on != self.split_on() {
            if on {
                let loc = self.current_pane().map(|p| p.current()).unwrap_or_else(util::home_file);
                self.notebook2.show();
                let width = self.split.allocated_width();
                if width > 0 {
                    self.split.set_position(width / 2);
                }
                self.set_active(&self.notebook2.clone());
                self.add_tab(&loc, true);
            } else {
                // Keep the pane that has focus and close the other one's tabs.
                let keep_extra = self.active() == self.notebook2;
                let closing = if keep_extra { self.notebook.clone() } else { self.notebook2.clone() };
                self.moving_tabs.set(true);
                while let Some(page) = closing.nth_page(Some(0)) {
                    let pane = self.panes.borrow().iter().find(|p| p.root.upcast_ref::<gtk::Widget>() == &page).cloned();
                    // Shut down while still in the window (see Pane::shutdown).
                    if let Some(p) = &pane {
                        p.shutdown();
                    }
                    closing.remove_page(Some(0));
                    if let Some(p) = pane {
                        self.panes.borrow_mut().retain(|q| !Rc::ptr_eq(q, &p));
                    }
                }
                if keep_extra {
                    self.move_tabs_to_main();
                }
                self.notebook2.hide();
                self.moving_tabs.set(false);
                self.set_active(&self.notebook.clone());
                if let Some(p) = self.current_pane() {
                    p.grab_focus();
                }
            }
        }
        self.set_state("split-view", on.to_variant());
        self.update_pane_dimming();
        self.update_tabs_visibility();
    }

    /// One side of split view lost its last tab: carry on with a single pane
    /// holding the tabs that are left.
    fn collapse_split(self: &Rc<Self>) {
        if !self.split_on() || (self.notebook.n_pages() > 0 && self.notebook2.n_pages() > 0) {
            return;
        }
        self.notebook2.hide();
        // The main pane is the one that stays, so give it the other's tabs.
        self.moving_tabs.set(true);
        self.move_tabs_to_main();
        self.moving_tabs.set(false);
        self.set_active(&self.notebook.clone());
        self.set_state("split-view", false.to_variant());
        self.update_pane_dimming();
        self.update_tabs_visibility();
    }

    /// Moves the extra pane's tabs (keeping their order and the current one)
    /// to the main pane, which is the one a single-pane window shows.
    /// Callers set `moving_tabs` around it.
    fn move_tabs_to_main(&self) {
        let current = self.notebook2.current_page();
        let offset = self.notebook.n_pages();
        while let Some(page) = self.notebook2.nth_page(Some(0)) {
            let label = self.notebook2.tab_label(&page);
            self.notebook2.remove_page(Some(0));
            self.notebook.append_page(&page, label.as_ref());
            self.set_tab_options(&page);
        }
        if let Some(c) = current {
            self.notebook.set_current_page(Some(offset + c));
        }
    }

    /// The current tab of the pane that isn't active (split view only).
    fn other_pane(&self) -> Option<Rc<Pane>> {
        if !self.split_on() {
            return None;
        }
        let nb = if self.active() == self.notebook { &self.notebook2 } else { &self.notebook };
        let page = nb.nth_page(nb.current_page())?;
        self.panes.borrow().iter().find(|p| p.root.upcast_ref::<gtk::Widget>() == &page).cloned()
    }

    fn transfer_to_other_pane(self: &Rc<Self>, kind: Kind) {
        let (Some(p), Some(other)) = (self.current_pane(), self.other_pane()) else { return };
        let uris = p.selected_uris();
        if !uris.is_empty() {
            self.run_transfer(kind, uris, other.current());
        }
    }

    /// Shows the file operations button while there are operations, with
    /// their overall progress.
    fn update_ops_button(&self) {
        let (n, fraction) = ops::summary();
        self.ops_btn.set_visible(n > 0);
        self.ops_bar.set_fraction(fraction);
        self.ops_btn.set_tooltip_text(Some(&match n {
            0 => "File operations".to_string(),
            1 => "1 file operation".to_string(),
            n => format!("{n} file operations"),
        }));
        if n == 0 {
            self.ops_pop.popdown();
        }
    }

    /// Puts the location bar's button groups in the order from the settings.
    fn apply_bar_order(&self) {
        let items = [
            ("path", self.path_area.clone().upcast::<gtk::Widget>()),
            ("zoom", self.zoom_box.clone().upcast()),
            ("view", self.view_box.clone().upcast()),
            ("split", self.split_btn.clone().upcast()),
            ("info", self.info_btn.clone().upcast()),
            ("ops", self.ops_btn.clone().upcast()),
        ];
        let Some(bar) = self.path_area.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else { return };
        for (i, id) in config::get().bar_order.iter().enumerate() {
            if let Some((_, w)) = items.iter().find(|(wid, _)| wid == id) {
                bar.reorder_child(w, i as i32);
            }
        }
    }

    fn set_tab_options(&self, page: &gtk::Widget) {
        let Some(nb) = notebook_of(page) else { return };
        nb.set_tab_reorderable(page, true);
        nb.set_tab_detachable(page, true);
        // Tabs share the bar equally; past their minimum width the bar scrolls.
        nb.set_child_tab_expand(page, true);
    }

    /// A tab was dropped into this window from another one: move its pane
    /// over to this window's list.
    fn adopt_page(self: &Rc<Self>, page: &gtk::Widget) {
        if self.panes.borrow().iter().any(|p| p.root.upcast_ref::<gtk::Widget>() == page) {
            return; // one of ours (added by add_tab)
        }
        for other in all_windows() {
            if Rc::ptr_eq(&other, self) {
                continue;
            }
            let pos = other.panes.borrow().iter().position(|p| p.root.upcast_ref::<gtk::Widget>() == page);
            if let Some(pos) = pos {
                let pane = other.panes.borrow_mut().remove(pos);
                self.panes.borrow_mut().push(pane.clone());
                self.set_tab_options(page);
                if let Some(nb) = notebook_of(page) {
                    nb.set_current_page(nb.page_num(page));
                    self.set_active(&nb);
                }
                self.update_tabs_visibility();
                other.update_tabs_visibility();
                self.update_for_pane(&pane);
                self.window.present();
                pane.grab_focus();
                return;
            }
        }
    }

    fn close_tab(self: &Rc<Self>, pane: &Rc<Pane>) {
        if self.panes.borrow().len() <= 1 {
            self.window.close();
            return;
        }
        let nb = notebook_of(pane.root.upcast_ref()).unwrap_or_else(|| self.notebook.clone());
        if let Some(n) = nb.page_num(&pane.root) {
            let (back, fwd) = pane.history();
            let mut closed = self.closed_tabs.borrow_mut();
            closed.push(ClosedTab { place: pane.place(), back, fwd, position: n });
            if closed.len() > MAX_CLOSED_TABS {
                closed.remove(0);
            }
            drop(closed);
            // The focus leaves the tab while it's still there: moved only
            // after its view is gone, accessibility (which follows the
            // focus) gets the gone view handed to it (gtk_widget_get_parent
            // assertion).
            if self.window.focused_widget().is_some_and(|f| f.is_ancestor(&pane.root)) {
                self.window.set_focus(None::<&gtk::Widget>);
            }
            // Shut down while still in the window (see Pane::shutdown).
            pane.shutdown();
            nb.remove_page(Some(n));
        }
        self.set_enabled("reopen-tab", true);
        // (Done already if it was in a notebook.)
        pane.shutdown();
        self.panes.borrow_mut().retain(|p| !Rc::ptr_eq(p, pane));
        self.update_tabs_visibility();
        if let Some(p) = self.current_pane() {
            self.update_for_pane(&p);
            p.grab_focus();
        }
    }

    /// Brings back the most recently closed tab where it was, with its history.
    fn reopen_tab(self: &Rc<Self>) {
        let Some(t) = self.closed_tabs.borrow_mut().pop() else { return };
        let pane = self.add_tab(&t.place.folder, true);
        pane.set_history(t.back, t.fwd);
        if t.place.search.is_some() {
            pane.show_place(t.place);
        }
        let nb = self.active();
        let last = nb.n_pages().saturating_sub(1);
        nb.reorder_child(&pane.root, Some(t.position.min(last)));
        self.set_enabled("reopen-tab", !self.closed_tabs.borrow().is_empty());
        self.update_for_pane(&pane);
    }

    fn update_tabs_visibility(&self) {
        let always = config::with(|s| s.always_show_tabs);
        for nb in [&self.notebook, &self.notebook2] {
            nb.set_show_tabs(nb.n_pages() > 1 || always);
        }
    }

    pub fn current_pane(&self) -> Option<Rc<Pane>> {
        let nb = self.active();
        let n = nb.current_page()?;
        let page = nb.nth_page(Some(n))?;
        self.panes.borrow().iter().find(|p| p.root.upcast_ref::<gtk::Widget>() == &page).cloned()
    }

    fn is_current(&self, pane: &Rc<Pane>) -> bool {
        self.current_pane().map(|c| Rc::ptr_eq(&c, pane)).unwrap_or(false)
    }

    pub fn go(self: &Rc<Self>, f: &gio::File) {
        if let Some(p) = self.current_pane() {
            p.navigate(f);
            p.grab_focus();
        } else {
            self.add_tab(f, true);
        }
    }

    // ================================================================ pane events

    fn on_pane_event(self: &Rc<Self>, pane: &Rc<Pane>, ev: Event) {
        match ev {
            Event::Location => {
                if self.is_current(pane) {
                    self.update_for_pane(pane);
                }
            }
            Event::Selection => {
                if self.is_current(pane) {
                    self.status.set_text(&pane.status_text());
                    self.update_action_states();
                }
            }
            Event::Loading(on) => {
                if self.is_current(pane) {
                    if on {
                        self.spinner.start();
                    } else {
                        self.spinner.stop();
                    }
                }
            }
            Event::Activate => self.open_selection(OpenMode::Normal),
            Event::Context { event, on_item } => self.context_menu(pane, event, on_item),
            Event::OpenNewTab(f) => {
                self.add_tab(&f, false);
            }
            Event::OpenFile(r) => {
                // As when opening it normally: files in the trash show their
                // properties instead.
                if pane.is_trash() {
                    props::show(self.window.upcast_ref(), vec![gio::File::for_uri(&r.uri)]);
                } else {
                    self.launch_files(&[r]);
                }
            }
            Event::Back => pane.go_back(),
            Event::Forward => pane.go_forward(),
            Event::Drop { uris, dest, action } => self.handle_drop(uris, dest, action),
            Event::Rename { uri, new_name } => self.rename(pane, &uri, &new_name),
            Event::Sort { by, desc } => self.user_sort(pane, &by, desc),
        }
    }

    /// A sort order the user picked in `pane`: remembered for its folder (and
    /// applied to other tabs showing it) with per-folder views on, otherwise
    /// the new sorting everywhere.
    fn user_sort(self: &Rc<Self>, pane: &Rc<Pane>, by: &str, desc: bool) {
        if config::with(|s| s.per_folder) {
            let folder = pane.current();
            folders::set_sort(&folder.uri(), by, desc);
            for_all_panes(|p| {
                if p.current().equal(&folder) {
                    p.apply_sort(by, desc);
                }
            });
        } else {
            config::update(|s| {
                s.sort_by = by.to_string();
                s.sort_desc = desc;
            });
            for_all_panes(|p| p.apply_sort(by, desc));
        }
        sync_all_windows();
    }

    fn update_for_pane(self: &Rc<Self>, pane: &Rc<Pane>) {
        self.sync_search(pane);
        let f = pane.current();
        self.window.set_title(&pane.title());
        self.update_crumbs(&f);
        if self.loc_stack.visible_child_name().as_deref() == Some("entry") && !self.location_entry.has_focus() {
            self.set_location_text(&location_text(&f));
        }
        self.sidebar.set_location(&f);
        self.set_state("view-mode", pane.mode().as_str().to_variant());
        self.set_state("info-panel", pane.info_panel_on().to_variant());
        let (by, desc) = pane.sort_state();
        self.set_state("sort-by", by.to_variant());
        self.set_state("sort-desc", desc.to_variant());
        self.status.set_text(&pane.status_text());
        if pane.is_loading() {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
        self.update_action_states();
    }

    fn update_action_states(&self) {
        let Some(p) = self.current_pane() else { return };
        let sel = p.selected();
        let has = !sel.is_empty();
        let in_trash = p.is_trash();
        let searching = p.is_searching();
        let writable = !in_trash;
        self.set_enabled("back", p.can_back());
        self.set_enabled("forward", p.can_forward());
        self.set_enabled("up", p.current().parent().is_some());
        for a in ["cut", "copy", "trash", "delete", "copy-to", "move-to"] {
            self.set_enabled(a, has);
        }
        let other_ok = self.other_pane().is_some_and(|o| !o.is_trash() && !o.current().equal(&p.current()));
        self.set_enabled("copy-to-pane", has && other_ok);
        self.set_enabled("move-to-pane", has && writable && other_ok);
        for a in ["open", "open-with-other"] {
            self.set_enabled(a, has);
        }
        let dirs = has && sel.iter().all(|r| r.is_dir);
        self.set_enabled("open-new-tab", dirs);
        self.set_enabled("open-new-window", dirs);
        // Bulk renaming works on local paths, like caja-rename.
        let local = has && !in_trash && sel.iter().all(|r| gio::File::for_uri(&r.uri).path().is_some());
        // Rename with several items opens bulk renaming; with one it renames
        // in place (the bulk window is then in the context menu only).
        self.set_enabled("rename", (sel.len() == 1 && !in_trash) || (sel.len() > 1 && local));
        self.set_enabled("rename-all", local);
        self.set_enabled("duplicate", has && writable);
        self.set_enabled("make-link", has && writable && sel.iter().all(|r| gio::File::for_uri(&r.uri).is_native()));
        // Search results are no folder to put anything in, as in Caja.
        self.set_enabled("paste", writable && !searching);
        self.set_enabled("paste-into", sel.len() == 1 && dirs && writable);
        self.set_enabled("new-folder", writable && !searching);
        self.set_enabled("new-file", writable && !searching);
        self.set_enabled("new-from-template", writable && !searching);
        self.set_enabled("restore", has && in_trash && sel.iter().all(|r| !r.orig.is_empty()));
        self.set_enabled("terminal", p.current().path().is_some());
    }

    fn flash_status(self: &Rc<Self>, text: &str) {
        self.status.set_text(text);
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            if let Some(s) = w.upgrade() {
                if let Some(p) = s.current_pane() {
                    s.status.set_text(&p.status_text());
                }
            }
        });
    }

    // ================================================================ path bar

    fn update_crumbs(self: &Rc<Self>, f: &gio::File) {
        // Keep deeper crumbs if we moved to an ancestor in the same chain.
        let keep = self.crumbs.borrow().iter().any(|c| c.equal(f));
        if !keep {
            let mut chain = vec![];
            let home = util::home_file();
            let mut cur = Some(f.clone());
            while let Some(c) = cur {
                chain.push(c.clone());
                if c.equal(&home) {
                    break;
                }
                cur = c.parent();
            }
            chain.reverse();
            *self.crumbs.borrow_mut() = chain;
        }
        for c in self.path_box.children() {
            self.path_box.remove(&c);
        }
        let home = util::home_file();
        let mut active_btn = None;
        let crumbs = self.crumbs.borrow().clone();
        for c in crumbs.iter() {
            let b = gtk::ToggleButton::new();
            // Caja's classes: the theme pads them as buttons with text.
            b.style_context().add_class("text-button");
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            let (icon, label) = if c.equal(&home) {
                (Some("user-home"), "Home".to_string())
            } else if c.uri() == "trash:///" {
                (Some("user-trash"), "Trash".to_string())
            } else if c.parent().is_none() {
                if c.path().is_some() {
                    (Some("drive-harddisk"), "File System".to_string())
                } else {
                    (Some("folder-remote"), util::display_name(c))
                }
            } else {
                (None, util::display_name(c))
            };
            if let Some(i) = icon {
                b.style_context().add_class("image-button");
                bx.pack_start(&gtk::Image::from_icon_name(Some(i), gtk::IconSize::Menu), false, false, 0);
            }
            let l = gtk::Label::new(Some(&label));
            l.set_ellipsize(pango::EllipsizeMode::Middle);
            l.set_max_width_chars(28);
            bx.pack_start(&l, false, false, 0);
            b.add(&bx);
            b.set_focus_on_click(false);
            let is_active = c.equal(f);
            b.set_active(is_active);
            let bold = pango::AttrList::new();
            bold.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            if is_active {
                active_btn = Some(b.clone());
                l.set_attributes(Some(&bold));
            } else {
                // Room for the name in bold, as in Caja: a button keeps its
                // width when its folder becomes the open one.
                let layout = l.create_pango_layout(Some(&label));
                let normal = layout.pixel_size().0;
                layout.set_attributes(Some(&bold));
                let wide = layout.pixel_size().0;
                l.set_margin_start((wide - normal) / 2);
                l.set_margin_end((wide - normal + 1) / 2);
            }
            let w = Rc::downgrade(self);
            let target = c.clone();
            b.connect_clicked(move |b| {
                if let Some(s) = w.upgrade() {
                    let cur = s.current_pane().map(|p| p.current());
                    if cur.map(|c| c.equal(&target)).unwrap_or(false) {
                        b.set_active(true);
                        return;
                    }
                    s.go(&target);
                }
            });
            // A middle-click opens the folder in a new tab, in the background,
            // as in Caja and as a middle-clicked place in the side pane. The
            // press is kept from the path bar, whose middle-click autoscrolls.
            b.connect_button_press_event(|_, ev| match ev.button() {
                2 => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            });
            {
                let w = Rc::downgrade(self);
                let target = c.clone();
                b.connect_button_release_event(move |b, ev| {
                    if ev.button() != 2 {
                        return glib::Propagation::Proceed;
                    }
                    // Only if let go over the crumb, as with a click.
                    let (x, y) = ev.position();
                    let inside = x >= 0.0 && y >= 0.0 && x < b.allocated_width() as f64 && y < b.allocated_height() as f64;
                    if let (true, Some(s)) = (inside, w.upgrade()) {
                        s.add_tab(&target, false);
                    }
                    glib::Propagation::Stop
                });
            }
            // Drop onto crumbs.
            // No DestDefaults::DROP: it finishes with delete=true on a move,
            // which would make the source view drop its row.
            // No DestDefaults::MOTION either: it refuses other drags instead
            // of letting the widgets around the crumb take them.
            b.drag_dest_set(
                gtk::DestDefaults::empty(),
                &crate::pane::drop_targets(),
                gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK,
            );
            {
                // Same rules as dropping into a file view (Ctrl copies, a
                // plain drag moves on the same filesystem, ...).
                let target = c.clone();
                b.connect_drag_motion(move |b, ctx, _, _, time| {
                    if b.drag_dest_find_target(ctx, None).is_none() {
                        return false;
                    }
                    ctx.drag_status(crate::pane::choose_action(b.upcast_ref(), ctx, &target), time);
                    b.drag_highlight();
                    true
                });
            }
            b.connect_drag_leave(|b, _, _| b.drag_unhighlight());
            let target = c.clone();
            b.connect_drag_drop(move |b, ctx, _, _, time| {
                match b.drag_dest_find_target(ctx, None) {
                    // Out of an archive: the archive manager saves the files.
                    Some(t) if crate::xds::is(&t) => {
                        if !crate::xds::start(b, ctx, &target, time) {
                            ctx.drag_finish(false, false, time);
                        }
                        true
                    }
                    Some(t) => {
                        b.drag_get_data(ctx, &t, time);
                        true
                    }
                    // Not files (e.g. a location bar button group being
                    // rearranged): let the path area around it take it.
                    None => false,
                }
            });
            {
                let w = Rc::downgrade(self);
                let target = c.clone();
                b.connect_drag_data_received(move |_, ctx, _, _, sel, _, time| {
                    if crate::xds::is(&sel.target()) {
                        crate::xds::finish(ctx, sel, time);
                        return;
                    }
                    let uris: Vec<String> = sel.uris().iter().map(|u| u.to_string()).collect();
                    let action = ctx.selected_action();
                    ctx.drag_finish(!uris.is_empty(), false, time);
                    if let Some(s) = w.upgrade() {
                        if !uris.is_empty() {
                            s.handle_drop(uris, target.clone(), action);
                        }
                    }
                });
            }
            self.path_box.pack_start(&b, false, false, 0);
        }
        self.path_box.show_all();
        if let Some(b) = active_btn {
            let sw = self.path_scroll.clone();
            glib::idle_add_local_once(move || {
                // The window may have been closed meanwhile: a destroyed
                // scrolled window has no scrollbar (and no adjustment).
                // Nor if it's not in a shown window (see
                // Pane::scroll_columns_to_end).
                if b.parent().is_none() || sw.hscrollbar().is_none() || !sw.is_realized() {
                    return;
                }
                let adj = sw.hadjustment();
                let alloc = b.allocation();
                let right = (alloc.x() + alloc.width()) as f64;
                if right > adj.value() + adj.page_size() {
                    adj.set_value(right - adj.page_size());
                } else if (alloc.x() as f64) < adj.value() {
                    adj.set_value(alloc.x() as f64);
                }
            });
        }
    }

    fn search_open(&self) -> bool {
        self.loc_stack.visible_child_name().as_deref() == Some("search")
    }

    /// Updates the search widgets without their handlers acting on it.
    fn syncing_search(&self, f: impl FnOnce()) {
        self.search_syncing.set(true);
        f();
        self.search_syncing.set(false);
    }

    /// Shows the search bar (Ctrl+F, the toolbar's button), empty, for the
    /// current pane's folder; it just takes the focus if it's shown.
    fn show_search(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        if !self.search_open() {
            self.loc_toggle.set_active(false);
            *self.search_pane.borrow_mut() = Some((Rc::downgrade(&p), p.current().uri().to_string()));
            self.syncing_search(|| self.search_field.set_text(""));
            self.loc_stack.set_visible_child_name("search");
        }
        self.syncing_search(|| self.search_btn.set_active(true));
        self.search_field.grab_focus();
    }

    /// Back to the path (or the location field).
    fn hide_search(&self) {
        self.search_pane.borrow_mut().take();
        self.loc_stack.set_visible_child_name(if config::with(|s| s.path_as_text) { "entry" } else { "path" });
        self.syncing_search(|| self.search_btn.set_active(false));
    }

    /// The search bar for `pane`, as in Caja: shown with the search's text
    /// while its results are, and one opened with Ctrl+F stays only while
    /// its pane shows the folder it was opened in.
    fn sync_search(&self, pane: &Rc<Pane>) {
        if let Some(text) = pane.search_text() {
            if !self.search_open() || self.search_field.text() != text {
                self.syncing_search(|| self.search_field.set_text(&text));
            }
            self.search_pane.borrow_mut().take();
            self.loc_stack.set_visible_child_name("search");
            self.syncing_search(|| self.search_btn.set_active(true));
            return;
        }
        if !self.search_open() {
            return;
        }
        let keep = self.search_pane.borrow().as_ref().is_some_and(|(w, uri)| w.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane)) && *uri == pane.current().uri());
        if !keep {
            self.hide_search();
        }
    }

    /// Breadcrumbs with an edit toggle, or (path_as_text) always the text
    /// field, without the toggle.
    fn apply_path_mode(&self) {
        let text = config::with(|s| s.path_as_text);
        self.loc_toggle.set_visible(!text);
        if self.search_open() {
            return;
        }
        if text {
            self.loc_stack.set_visible_child_name("entry");
            if let Some(p) = self.current_pane() {
                self.set_location_text(&location_text(&p.current()));
            }
        } else if !self.loc_toggle.is_active() {
            self.loc_stack.set_visible_child_name("path");
        }
    }

    fn show_location_entry(self: &Rc<Self>, text: Option<&str>) {
        self.loc_toggle.set_active(true);
        if let Some(t) = text {
            self.set_location_text(t);
            self.location_entry.set_position(-1);
        } else {
            self.location_entry.grab_focus();
            self.location_entry.select_region(0, -1);
        }
    }

    /// Set the entry text without triggering completion.
    fn set_location_text(&self, t: &str) {
        self.completing.set(true);
        self.location_entry.set_text(t);
        self.completing.set(false);
        // Folders may have changed since the last time.
        self.completion_dir.replace(None);
    }

    /// Caja's location completion: when the cursor is at the end of what's
    /// typed, add the rest of the folder name it's the start of, selected.
    fn complete_location(&self) {
        let e = &self.location_entry;
        let len = e.text_length() as i32;
        if !e.has_focus() || e.position() != len || e.selection_bounds().is_some() {
            return;
        }
        let text = e.text().to_string();
        let (dir_text, base) = match text.rfind('/') {
            Some(i) => (&text[..=i], &text[i + 1..]),
            None => ("", text.as_str()),
        };
        if base.is_empty() || text.contains("://") {
            return;
        }
        // The folder the name is typed in: absolute, in ~, or relative to
        // the open folder.
        let dir = if dir_text.starts_with('/') {
            std::path::PathBuf::from(dir_text)
        } else if let Some(rest) = dir_text.strip_prefix("~/") {
            glib::home_dir().join(rest)
        } else if dir_text.starts_with('~') {
            // ~user/… isn't supported.
            return;
        } else {
            match self.current_pane().and_then(|p| p.current().path()) {
                Some(cur) => cur.join(dir_text),
                None => return,
            }
        };
        let stale = self.completion_dir.borrow().as_ref().is_none_or(|(d, _)| *d != dir);
        if stale {
            let names = std::fs::read_dir(&dir)
                .map(|rd| {
                    // The listing knows each entry's type: only links need
                    // a look at what they point to (a round trip each on a
                    // network share).
                    rd.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())))
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            self.completion_dir.replace(Some((dir, names)));
        }
        let suffix = self.completion_dir.borrow().as_ref().and_then(|(_, names)| completion_suffix(base, names));
        if let Some(suffix) = suffix {
            self.completing.set(true);
            let mut pos = len;
            e.insert_text(&suffix, &mut pos);
            self.completing.set(false);
            e.select_region(len, -1);
        }
    }

    // ================================================================ commands

    fn open_selection(self: &Rc<Self>, mode: OpenMode) {
        let Some(p) = self.current_pane() else { return };
        let sel = p.selected();
        if sel.is_empty() {
            return;
        }
        if p.is_trash() {
            // Opening files from the trash makes little sense; show properties instead.
            if sel.iter().all(|r| !r.is_dir) {
                props::show(self.window.upcast_ref(), p.selected_files());
                return;
            }
        }
        let (dirs, files): (Vec<_>, Vec<_>) = sel.into_iter().partition(|r| r.is_dir);
        let mut first = matches!(mode, OpenMode::Normal);
        for d in &dirs {
            let f = gio::File::for_uri(&d.uri);
            match mode {
                OpenMode::Window => {
                    if let Some(app) = self.window.application() {
                        AppWindow::new(&app, &[f]);
                    }
                }
                _ if first => {
                    first = false;
                    p.navigate(&f);
                }
                _ => {
                    self.add_tab(&f, false);
                }
            }
        }
        if !files.is_empty() {
            self.launch_files(&files);
        }
    }

    fn launch_files(self: &Rc<Self>, files: &[crate::pane::RowInfo]) {
        // Executable scripts and programs: ask, like Caja.
        if files.len() == 1 {
            let f = gio::File::for_uri(&files[0].uri);
            let ct = &files[0].ctype;
            let execable = f
                .query_info("access::can-execute", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
                .map(|i| i.boolean("access::can-execute"))
                .unwrap_or(false);
            let is_prog = ct == "application/x-executable"
                || ct == "application/x-sharedlib"
                || ct == "application/x-pie-executable"
                || ct == "application/vnd.appimage";
            let is_script = gio::content_type_is_a(ct, "text/plain") && execable;
            if execable && f.path().is_some() && (is_prog || is_script) {
                self.ask_run(&f, is_script);
                return;
            }
        }
        // Group by default application.
        let mut groups: Vec<(gio::AppInfo, Vec<gio::File>)> = vec![];
        let mut unknown = vec![];
        for r in files {
            match gio::AppInfo::default_for_type(&r.ctype, false) {
                Some(app) => {
                    let id = app.id().map(|s| s.to_string());
                    match groups.iter_mut().find(|(a, _)| a.id().map(|s| s.to_string()) == id) {
                        Some((_, v)) => v.push(gio::File::for_uri(&r.uri)),
                        None => groups.push((app, vec![gio::File::for_uri(&r.uri)])),
                    }
                }
                None => unknown.push(r.clone()),
            }
        }
        for (app, files) in groups {
            self.launch_app(&app, &files);
        }
        if let Some(r) = unknown.first() {
            self.choose_app(&r.ctype, unknown.iter().map(|r| gio::File::for_uri(&r.uri)).collect());
        }
    }

    fn launch_app(&self, app: &gio::AppInfo, files: &[gio::File]) {
        let ctx = self.window.display().app_launch_context();
        let res = if app.supports_files() || files.iter().all(|f| f.is_native()) {
            app.launch(files, ctx.as_ref())
        } else {
            let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
            let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
            app.launch_uris(&refs, ctx.as_ref())
        };
        if let Err(e) = res {
            util::error_dialog(Some(self.window.upcast_ref()), &format!("Could not open with {}", app.display_name()), e.message());
        }
    }

    fn ask_run(self: &Rc<Self>, f: &gio::File, is_script: bool) {
        let name = util::display_name(f);
        let d = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question,
            gtk::ButtonsType::None,
            &format!("Do you want to run “{name}”, or display its contents?"),
        );
        d.set_secondary_text(Some(&format!("“{name}” is an executable file.")));
        let term = util::terminal_command();
        if term.is_some() {
            d.add_button("Run in _Terminal", gtk::ResponseType::Other(1));
        }
        if is_script {
            d.add_button("_Display", gtk::ResponseType::Other(2));
        }
        d.add_button("_Cancel", gtk::ResponseType::Cancel);
        d.add_button("_Run", gtk::ResponseType::Other(3));
        d.set_default_response(gtk::ResponseType::Other(3));
        let w = Rc::downgrade(self);
        let f = f.clone();
        d.connect_response(move |d, r| {
            d.close();
            let Some(s) = w.upgrade() else { return };
            let path = f.path().unwrap();
            let dir = path.parent().map(|p| p.to_path_buf());
            let res = match r {
                gtk::ResponseType::Other(1) => {
                    let t = term.clone().unwrap();
                    let argv = util::terminal_run_argv(&t, &path.to_string_lossy());
                    util::spawn_in_dir(&argv, dir.as_deref())
                }
                gtk::ResponseType::Other(2) => {
                    if let Some(app) = gio::AppInfo::default_for_type("text/plain", false) {
                        s.launch_app(&app, std::slice::from_ref(&f));
                    }
                    Ok(())
                }
                gtk::ResponseType::Other(3) => util::spawn_in_dir(&[path.to_string_lossy().to_string()], dir.as_deref()),
                _ => Ok(()),
            };
            if let Err(e) = res {
                util::error_dialog(Some(s.window.upcast_ref()), "Could not run the program", &e);
            }
        });
        d.show();
    }

    fn choose_app(self: &Rc<Self>, ctype: &str, files: Vec<gio::File>) {
        let d = gtk::AppChooserDialog::for_content_type(Some(&self.window), gtk::DialogFlags::MODAL, ctype);
        if let Ok(w) = d.widget().downcast::<gtk::AppChooserWidget>() {
            w.set_show_fallback(true);
            w.set_show_other(true);
        }
        let ws = Rc::downgrade(self);
        d.connect_response(move |d, r| {
            if r == gtk::ResponseType::Ok {
                if let (Some(app), Some(s)) = (d.app_info(), ws.upgrade()) {
                    s.launch_app(&app, &files);
                }
            }
            d.close();
        });
        d.show();
    }

    fn open_with(self: &Rc<Self>, id: &str) {
        let Some(p) = self.current_pane() else { return };
        let Some(app) = gio::DesktopAppInfo::new(id) else { return };
        self.launch_app(app.upcast_ref(), &p.selected_files());
    }

    fn open_with_other(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        let sel = p.selected();
        if let Some(r) = sel.first() {
            self.choose_app(&r.ctype.clone(), p.selected_files());
        }
    }

    fn new_folder(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        let dir = p.current();
        let name = util::unique_name(&dir, "Untitled Folder", true);
        let f = dir.child(&name);
        match f.make_directory(gio::Cancellable::NONE) {
            Ok(()) => {
                p.add_file_now(&f);
                p.rename_later(f.uri().to_string());
            }
            Err(e) => util::error_dialog(Some(self.window.upcast_ref()), "Could not create the folder", e.message()),
        }
    }

    fn new_file(self: &Rc<Self>, template: Option<String>) {
        let Some(p) = self.current_pane() else { return };
        let dir = p.current();
        let res = match &template {
            None => {
                let name = util::unique_name(&dir, "Untitled Document", false);
                let f = dir.child(&name);
                f.create(gio::FileCreateFlags::NONE, gio::Cancellable::NONE).map(|_| f)
            }
            Some(t) => {
                let src = gio::File::for_path(t);
                let base = src.basename().map(|b| b.to_string_lossy().to_string()).unwrap_or_default();
                let name = util::unique_name(&dir, &base, false);
                let f = dir.child(&name);
                src.copy(&f, gio::FileCopyFlags::NONE, gio::Cancellable::NONE, None).map(|_| f)
            }
        };
        match res {
            Ok(f) => {
                p.add_file_now(&f);
                p.rename_later(f.uri().to_string());
            }
            Err(e) => util::error_dialog(Some(self.window.upcast_ref()), "Could not create the document", e.message()),
        }
    }

    fn rename(self: &Rc<Self>, pane: &Rc<Pane>, uri: &str, new_name: &str) {
        if new_name.contains('/') {
            util::error_dialog(Some(self.window.upcast_ref()), "Invalid name", "File names cannot contain “/”.");
            return;
        }
        let f = gio::File::for_uri(uri);
        if let Some(parent) = f.parent() {
            let target = parent.child(new_name);
            if target.query_exists(gio::Cancellable::NONE) && !target.equal(&f) {
                util::error_dialog(
                    Some(self.window.upcast_ref()),
                    &format!("The name “{new_name}” is already used in this location."),
                    "Please use a different name.",
                );
                return;
            }
        }
        match f.set_display_name(new_name, gio::Cancellable::NONE) {
            Ok(nf) => {
                pane.refresh_file(&f);
                pane.add_file_now(&nf);
                pane.select_uris(&[nf.uri().to_string()]);
            }
            Err(e) => util::error_dialog(Some(self.window.upcast_ref()), &format!("Could not rename “{}”", util::display_name(&f)), e.message()),
        }
    }

    fn copy_selection(&self, cut: bool) {
        let Some(p) = self.current_pane() else { return };
        let uris = p.selected_uris();
        if uris.is_empty() {
            return;
        }
        clipboard::set(uris, cut);
        for_all_panes(|p| p.redraw());
    }

    fn paste_into(self: &Rc<Self>, dest: gio::File) {
        let w = Rc::downgrade(self);
        clipboard::get(move |cut, uris| {
            let Some(s) = w.upgrade() else { return };
            if uris.is_empty() {
                s.flash_status("There is nothing on the clipboard to paste.");
                return;
            }
            let kind = if cut { Kind::Move } else { Kind::Copy };
            s.run_transfer(kind, uris, dest.clone());
            if cut {
                clipboard::clear_after_cut();
                for_all_panes(|p| p.redraw());
            }
        });
    }

    /// Run a copy/move/link and select the results if the destination is open.
    fn run_transfer(self: &Rc<Self>, kind: Kind, uris: Vec<String>, dest: gio::File) {
        if dest.uri().starts_with("trash:") {
            if kind == Kind::Move {
                self.run_job(Job::Trash(uris));
            }
            return;
        }
        let items = uris
            .into_iter()
            .map(|u| Item { src: u, dest_dir: dest.uri().to_string(), name: None })
            .collect();
        let w = Rc::downgrade(self);
        let dest2 = dest.clone();
        ops::run(Some(self.window.clone().upcast()), Job::Transfer { kind, items }, move |created| {
            if let Some(s) = w.upgrade() {
                if let Some(p) = s.current_pane() {
                    if p.current().equal(&dest2) && !created.is_empty() {
                        let files: Vec<gio::File> = created.iter().map(|c| gio::File::for_uri(c)).collect();
                        p.add_files_now(&files);
                        p.select_uris(&created);
                    }
                }
            }
        });
    }

    fn run_job(self: &Rc<Self>, job: Job) {
        ops::run(Some(self.window.clone().upcast()), job, |_| {});
    }

    fn transfer_selection_here(self: &Rc<Self>, kind: Kind) {
        let Some(p) = self.current_pane() else { return };
        let uris = p.selected_uris();
        if !uris.is_empty() {
            self.run_transfer(kind, uris, p.current());
        }
    }

    fn transfer_to_chosen(self: &Rc<Self>, kind: Kind) {
        let Some(p) = self.current_pane() else { return };
        let uris = p.selected_uris();
        if uris.is_empty() {
            return;
        }
        let title = if kind == Kind::Move { "Move To" } else { "Copy To" };
        let d = gtk::FileChooserDialog::new(Some(title), Some(&self.window), gtk::FileChooserAction::SelectFolder);
        d.add_button("_Cancel", gtk::ResponseType::Cancel);
        d.add_button(if kind == Kind::Move { "_Move" } else { "_Copy" }, gtk::ResponseType::Accept);
        d.set_default_response(gtk::ResponseType::Accept);
        let _ = d.set_current_folder_file(&p.current());
        d.set_modal(true);
        let w = Rc::downgrade(self);
        d.connect_response(move |d, r| {
            if r == gtk::ResponseType::Accept {
                if let (Some(dest), Some(s)) = (d.file(), w.upgrade()) {
                    s.run_transfer(kind, uris.clone(), dest);
                }
            }
            d.close();
        });
        d.show();
    }

    fn trash_selection(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        if p.is_trash() {
            self.delete_selection();
            return;
        }
        let uris = p.selected_uris();
        if uris.is_empty() {
            return;
        }
        // Where there's no trash (a phone, many network shares), ask once
        // whether to delete permanently, instead of a trash question and
        // then an error for every item.
        let can_trash = gio::File::for_uri(&uris[0])
            .query_info("access::can-trash", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, gio::Cancellable::NONE)
            .map(|i| !i.has_attribute("access::can-trash") || i.boolean("access::can-trash"))
            .unwrap_or(true);
        if !can_trash {
            let sel = p.selected();
            let q = if sel.len() == 1 {
                format!("“{}” can't be moved to the Trash. Delete it permanently?", sel[0].name)
            } else {
                format!("These {} items can't be moved to the Trash. Delete them permanently?", sel.len())
            };
            if self.confirm(&q, "This location has no trash. If you delete an item, it will be permanently lost.", "_Delete", true) {
                self.run_job(Job::Delete(uris));
            }
            return;
        }
        if config::with(|s| s.confirm_trash) {
            let sel = p.selected();
            let q = if sel.len() == 1 { format!("Move “{}” to the Trash?", sel[0].name) } else { format!("Move {} items to the Trash?", sel.len()) };
            if !self.confirm(&q, "", "Move to _Trash", true) {
                return;
            }
        }
        self.run_job(Job::Trash(uris));
    }

    fn delete_selection(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        let sel = p.selected();
        if sel.is_empty() {
            return;
        }
        let q = if sel.len() == 1 {
            format!("Are you sure you want to permanently delete “{}”?", sel[0].name)
        } else {
            format!("Are you sure you want to permanently delete the {} selected items?", sel.len())
        };
        // Delete is the default, so Shift+Delete then Enter deletes.
        if self.confirm(&q, "If you delete an item, it will be permanently lost.", "_Delete", true) {
            self.run_job(Job::Delete(sel.into_iter().map(|r| r.uri).collect()));
        }
    }

    fn restore_selection(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        let items: Vec<Item> = p
            .selected()
            .into_iter()
            .filter(|r| !r.orig.is_empty())
            .map(|r| Item {
                src: r.uri.clone(),
                dest_dir: gio::File::for_path(&r.orig).uri().to_string(),
                name: Some(r.name.clone()),
            })
            .collect();
        if !items.is_empty() {
            ops::run(Some(self.window.clone().upcast()), Job::Transfer { kind: Kind::Move, items }, |_| {});
        }
    }

    fn empty_trash(self: &Rc<Self>) {
        if self.confirm(
            "Empty all items from the Trash?",
            "All items in the Trash will be permanently deleted.",
            "_Empty Trash",
            false,
        ) {
            self.run_job(Job::EmptyTrash);
        }
    }

    /// `ok_default`: Enter confirms (and the button has focus) instead of
    /// cancelling.
    fn confirm(&self, primary: &str, secondary: &str, ok: &str, ok_default: bool) -> bool {
        let d = gtk::MessageDialog::new(
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Warning,
            gtk::ButtonsType::None,
            primary,
        );
        if !secondary.is_empty() {
            d.set_secondary_text(Some(secondary));
        }
        d.add_button("_Cancel", gtk::ResponseType::Cancel);
        let ok_btn = d.add_button(ok, gtk::ResponseType::Ok);
        if ok_default {
            d.set_default_response(gtk::ResponseType::Ok);
            ok_btn.grab_focus();
        } else {
            d.set_default_response(gtk::ResponseType::Cancel);
        }
        let r = d.run();
        d.close();
        r == gtk::ResponseType::Ok
    }

    pub fn handle_drop(self: &Rc<Self>, uris: Vec<String>, dest: gio::File, action: gdk::DragAction) {
        // Ignore drops of items onto the folder they're already in.
        let all_here = uris.iter().all(|u| {
            gio::File::for_uri(u).parent().map(|p| p.equal(&dest)).unwrap_or(false)
        });
        if all_here && action != gdk::DragAction::LINK {
            return;
        }
        if action == gdk::DragAction::ASK {
            let menu = gtk::Menu::new();
            for (label, kind) in [("_Move Here", Some(Kind::Move)), ("_Copy Here", Some(Kind::Copy)), ("_Link Here", Some(Kind::Link)), ("Cancel", None)] {
                if label == "Cancel" {
                    menu.append(&gtk::SeparatorMenuItem::new());
                }
                let item = gtk::MenuItem::with_mnemonic(label);
                let w = Rc::downgrade(self);
                let uris = uris.clone();
                let dest = dest.clone();
                item.connect_activate(move |_| {
                    if let (Some(s), Some(k)) = (w.upgrade(), kind) {
                        s.run_transfer(k, uris.clone(), dest.clone());
                    }
                });
                menu.append(&item);
            }
            menu.show_all();
            menu.set_attach_widget(Some(&self.window));
            menu.popup_at_pointer(None);
            return;
        }
        let kind = if action == gdk::DragAction::MOVE {
            Kind::Move
        } else if action == gdk::DragAction::LINK {
            Kind::Link
        } else {
            Kind::Copy
        };
        self.run_transfer(kind, uris, dest);
    }

    fn open_terminal(self: &Rc<Self>) {
        let Some(p) = self.current_pane() else { return };
        let dir = p
            .selected()
            .into_iter()
            .find(|r| r.is_dir)
            .map(|r| gio::File::for_uri(&r.uri))
            .unwrap_or_else(|| p.current());
        let Some(path) = dir.path() else { return };
        let Some(term) = util::terminal_command() else {
            util::error_dialog(Some(self.window.upcast_ref()), "No terminal found", "Install mate-terminal or set $TERMINAL.");
            return;
        };
        let mut argv = vec![term.clone()];
        if term.ends_with("mate-terminal") || term.ends_with("gnome-terminal") || term.ends_with("xfce4-terminal") {
            argv.push(format!("--working-directory={}", path.to_string_lossy()));
        }
        if let Err(e) = util::spawn_in_dir(&argv, Some(&path)) {
            util::error_dialog(Some(self.window.upcast_ref()), "Could not open a terminal", &e);
        }
    }

    fn archive(self: &Rc<Self>, extract: bool) {
        let Some(p) = self.current_pane() else { return };
        let Some(tool) = util::archiver_command() else { return };
        let paths: Vec<String> = p
            .selected_files()
            .iter()
            .filter_map(|f| f.path())
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        if paths.is_empty() {
            return;
        }
        let mut argv = vec![tool.clone()];
        if extract {
            argv.push("--extract-here".into());
        } else if tool.ends_with("xarchiver") {
            argv.push("--compress".into());
        } else {
            argv.push("--add".into());
        }
        argv.extend(paths);
        if let Err(e) = util::spawn_in_dir(&argv, p.current().path().as_deref()) {
            util::error_dialog(Some(self.window.upcast_ref()), "Could not start the archive manager", &e);
        }
    }

    fn select_pattern_dialog(self: &Rc<Self>) {
        let d = gtk::Dialog::with_buttons(
            Some("Select Items Matching"),
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("_Cancel", gtk::ResponseType::Cancel), ("_Select", gtk::ResponseType::Ok)],
        );
        d.set_default_response(gtk::ResponseType::Ok);
        let bx = d.content_area();
        bx.set_spacing(6);
        bx.set_border_width(12);
        bx.add(&gtk::Label::new(Some("Pattern (e.g. *.jpg, report-??.pdf):")));
        let e = gtk::Entry::new();
        e.set_activates_default(true);
        e.set_text("*");
        bx.add(&e);
        d.show_all();
        let w = Rc::downgrade(self);
        d.connect_response(move |d, r| {
            if r == gtk::ResponseType::Ok {
                if let Some(p) = w.upgrade().and_then(|s| s.current_pane()) {
                    p.select_pattern(&e.text());
                }
            }
            d.close();
        });
    }

    fn edit_bookmarks(self: &Rc<Self>) {
        thread_local! {
            static OPEN: RefCell<glib::WeakRef<gtk::Dialog>> = RefCell::default();
        }
        if self.present_open(&OPEN.with(|o| o.borrow().upgrade())) {
            return;
        }
        let d = gtk::Dialog::with_buttons(
            Some("Edit Bookmarks"),
            Some(&self.window),
            gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("_Close", gtk::ResponseType::Close)],
        );
        d.set_default_size(460, 360);
        let bx = d.content_area();
        bx.set_border_width(8);
        bx.pack_start(&bookmark_editor(d.upcast_ref()), true, true, 0);
        d.connect_response(|d, _| d.close());
        OPEN.with(|o| o.borrow().set(Some(&d)));
        d.show_all();
    }

    /// Brings `open` (Preferences or Edit Bookmarks, shown once at a time,
    /// as in Caja) to the front of this window; false if it isn't open.
    fn present_open(&self, open: &Option<gtk::Dialog>) -> bool {
        let Some(d) = open else { return false };
        d.set_transient_for(Some(&self.window));
        // Its "win.*" check buttons act on the window it's in front of.
        d.insert_action_group("win", Some(&self.window));
        d.present();
        true
    }

    fn preferences(self: &Rc<Self>) {
        thread_local! {
            static OPEN: RefCell<glib::WeakRef<gtk::Dialog>> = RefCell::default();
        }
        if self.present_open(&OPEN.with(|o| o.borrow().upgrade())) {
            return;
        }
        let s = config::get();
        let d = gtk::Dialog::new();
        OPEN.with(|o| o.borrow().set(Some(&d)));
        d.set_title("File Manager Preferences");
        d.set_transient_for(Some(&self.window));
        d.set_destroy_with_parent(true);
        d.set_default_size(680, 480);
        // Dialogs don't see the window's actions, which the "win.*" check buttons use.
        d.insert_action_group("win", Some(&self.window));

        // Nemo-style layout: a list of pages on the left, the page on the right.
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        let pages = gtk::ListBox::new();
        pages.style_context().add_class("sidebar");
        pages.set_size_request(170, -1);
        let add_page = |name: &str, title: &str, icon: &str, content: &gtk::Box| {
            let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
            sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            sw.add(content);
            stack.add_named(&sw, name);
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hb.set_border_width(8);
            hb.add(&gtk::Image::from_icon_name(Some(icon), gtk::IconSize::Menu));
            hb.add(&gtk::Label::new(Some(title)));
            let row = gtk::ListBoxRow::new();
            row.add(&hb);
            row.set_widget_name(name);
            pages.add(&row);
        };
        let page = || {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
            b.set_border_width(14);
            b
        };
        let head = |b: &gtk::Box, t: &str| {
            let l = gtk::Label::new(None);
            l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(t)));
            l.set_xalign(0.0);
            if !b.children().is_empty() {
                l.set_margin_top(8);
            }
            b.add(&l);
        };
        let indent = |b: &gtk::Box, w: &gtk::Widget| {
            w.set_margin_start(40);
            b.add(w);
        };
        // "Label:  [combo]" rows; the size group lines up the combos.
        let labels = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        let option = |b: &gtk::Box, text: &str, w: &gtk::Widget| {
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let l = gtk::Label::new(Some(text));
            l.set_xalign(0.0);
            labels.add_widget(&l);
            hb.add(&l);
            hb.add(w);
            indent(b, hb.upcast_ref());
        };
        let check = |label: &str| gtk::CheckButton::with_mnemonic(label);
        // Related switches on one line: "Label:  [x] A  [x] B".
        let inline = |b: &gtk::Box, text: &str, items: &[&gtk::Widget]| {
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            hb.add(&gtk::Label::new(Some(text)));
            for w in items {
                hb.add(*w);
            }
            indent(b, hb.upcast_ref());
            hb
        };
        // The zoom table: per view a row with the zoom level dropdown and an
        // editable list of levels. `get`/`set` pick that view's
        // (levels, size) out of the settings.
        let zoom_grid = gtk::Grid::new();
        zoom_grid.set_row_spacing(6);
        zoom_grid.set_column_spacing(12);
        zoom_grid.set_margin_start(40);
        for (col, text) in [(1, "Zoom level"), (2, "Zoom levels to step through")] {
            let l = gtk::Label::new(Some(text));
            l.set_xalign(0.0);
            l.style_context().add_class("dim-label");
            zoom_grid.attach(&l, col, 0, 1, 1);
        }
        let zoom_row = |row: i32, title: &str, defaults: &'static [i32], get: fn(&config::Settings) -> (Vec<i32>, i32), set: fn(&mut config::Settings, Option<Vec<i32>>, i32)| {
            let l = gtk::Label::new(Some(title));
            l.set_xalign(0.0);
            zoom_grid.attach(&l, 0, row, 1, 1);
            let combo = gtk::ComboBoxText::new();
            let fill = {
                let combo = combo.clone();
                move || {
                    let (levels, size) = get(&config::get());
                    combo.remove_all();
                    for l in &levels {
                        combo.append(Some(&l.to_string()), &format!("{l} px"));
                    }
                    combo.set_active_id(Some(&size.to_string()));
                }
            };
            fill();
            // Zooming in a window changes the level while the dialog is open.
            {
                let fill = fill.clone();
                d.connect_focus_in_event(move |_, _| {
                    fill();
                    glib::Propagation::Proceed
                });
            }
            combo.connect_changed(move |c| {
                if let Some(px) = c.active_id().and_then(|i| i.parse().ok()) {
                    if get(&config::get()).1 != px {
                        config::update(|s| set(s, None, px));
                        for_all_panes(|p| p.apply_zoom());
                    }
                }
            });
            zoom_grid.attach(&combo, 1, row, 1, 1);
            let entry = gtk::Entry::new();
            entry.set_text(&config::format_levels(&get(&config::get()).0));
            entry.set_width_chars(26);
            entry.set_tooltip_text(Some("Sizes in pixels (8–512) that Zoom In and Zoom Out step through, separated by commas"));
            let reset = gtk::Button::from_icon_name(Some("edit-undo-symbolic"), gtk::IconSize::Button);
            reset.set_tooltip_text(Some("Restore the default zoom levels"));
            reset.set_sensitive(get(&config::get()).0 != defaults);
            let apply = {
                let reset = reset.clone();
                move |e: &gtk::Entry| {
                    // An emptied field goes back to the saved levels.
                    if e.text().trim().is_empty() {
                        e.set_text(&config::format_levels(&get(&config::get()).0));
                    }
                    match config::parse_levels(&e.text()) {
                        Some(levels) => {
                            e.style_context().remove_class("error");
                            reset.set_sensitive(levels != defaults);
                            e.set_text(&config::format_levels(&levels));
                            if levels != get(&config::get()).0 {
                                let size = config::snap(&levels, get(&config::get()).1);
                                config::update(|s| set(s, Some(levels), size));
                                fill();
                                for_all_panes(|p| p.apply_zoom());
                            }
                        }
                        None => e.style_context().add_class("error"),
                    }
                }
            };
            let apply = Rc::new(apply);
            {
                let apply = apply.clone();
                let entry = entry.clone();
                reset.connect_clicked(move |_| {
                    entry.set_text(&config::format_levels(defaults));
                    apply(&entry);
                });
            }
            {
                let apply = apply.clone();
                entry.connect_activate(move |e| apply(e));
            }
            entry.connect_focus_out_event(move |e, _| {
                apply(e);
                glib::Propagation::Proceed
            });
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            hb.add(&entry);
            hb.add(&reset);
            zoom_grid.attach(&hb, 2, row, 1, 1);
        };

        // Views
        let v = page();
        head(&v, "Default View");
        let view = gtk::ComboBoxText::new();
        view.append(Some("icon"), "Icon View");
        view.append(Some("list"), "List View");
        view.append(Some("compact"), "Compact View");
        view.append(Some("columns"), "Columns View");
        view.set_active_id(Some(&s.default_view));
        view.connect_changed(|c| {
            if let Some(id) = c.active_id() {
                config::update(|s| s.default_view = id.to_string());
            }
        });
        option(&v, "View new folders using:", view.upcast_ref());
        let arrange = gtk::ComboBoxText::new();
        for (id, label) in [("name", "By Name"), ("size", "By Size"), ("type", "By Type"), ("mtime", "By Modification Date")] {
            arrange.append(Some(id), label);
        }
        arrange.set_active_id(Some(&s.sort_by));
        arrange.connect_changed(|c| {
            if let Some(id) = c.active_id() {
                config::update(|s| s.sort_by = id.to_string());
                reapply_view_settings();
            }
        });
        option(&v, "Arrange items:", arrange.upcast_ref());
        let rev = setting_check("_Reverse sort", None, s.sort_desc, |s, v| s.sort_desc = v, |_| reapply_view_settings());
        indent(&v, rev.upcast_ref());
        let ff = check("Sort _folders before files");
        ff.set_action_name(Some("win.folders-first"));
        indent(&v, ff.upcast_ref());
        head(&v, "Individual Folders");
        let per = setting_check(
            "Remember _view and sorting for each folder",
            Some("Changing the view or sorting then only affects the folder you're in, and it's restored the next time you open that folder. Other folders use the defaults above."),
            s.per_folder,
            |s, v| s.per_folder = v,
            |_| reapply_view_settings(),
        );
        indent(&v, per.upcast_ref());
        let remembered = |n: usize| match n {
            0 => "No folders remembered".to_string(),
            1 => "1 folder remembered".to_string(),
            n => format!("{n} folders remembered"),
        };
        let count = gtk::Label::new(Some(&remembered(folders::count())));
        count.style_context().add_class("dim-label");
        let forget = gtk::Button::with_mnemonic("_Forget All");
        forget.set_tooltip_text(Some("Forget the view and sorting of every folder, so they all use the defaults again"));
        forget.set_sensitive(folders::count() > 0);
        {
            let count = count.clone();
            forget.connect_clicked(move |b| {
                folders::forget_all();
                count.set_text(&remembered(0));
                b.set_sensitive(false);
                reapply_view_settings();
            });
        }
        {
            // Folders get remembered while the dialog is open.
            let (count, forget) = (count.clone(), forget.clone());
            d.connect_focus_in_event(move |_, _| {
                let n = folders::count();
                count.set_text(&remembered(n));
                forget.set_sensitive(n > 0);
                glib::Propagation::Proceed
            });
        }
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hb.add(&count);
        hb.add(&forget);
        indent(&v, hb.upcast_ref());
        head(&v, "Zoom");
        zoom_row(1, "Icon view", &config::ICON_SIZES, |s| (s.icon_levels.clone(), s.icon_size), |s, levels, size| {
            if let Some(l) = levels {
                s.icon_levels = l;
            }
            s.icon_size = size;
        });
        zoom_row(2, "List view", &config::LIST_SIZES, |s| (s.list_levels.clone(), s.list_size), |s, levels, size| {
            if let Some(l) = levels {
                s.list_levels = l;
            }
            s.list_size = size;
        });
        zoom_row(3, "Compact view", &config::COMPACT_SIZES, |s| (s.compact_levels.clone(), s.compact_size), |s, levels, size| {
            if let Some(l) = levels {
                s.compact_levels = l;
            }
            s.compact_size = size;
        });
        v.add(&zoom_grid);
        add_page("views", "Views", "view-grid-symbolic", &v);

        // Behavior
        let b = page();
        head(&b, "Opening");
        let single = setting_check("_Single click to open items", None, s.single_click, |s, v| s.single_click = v, |_| for_all_panes(|p| p.apply_click_policy()));
        indent(&b, single.upcast_ref());
        head(&b, "Tabs");
        let at_end = setting_check(
            "Add new tabs at the _end",
            Some("Like a browser: new tabs go to the end of the tab bar. Turned off, they open right after the current tab."),
            s.tabs_at_end,
            |s, v| s.tabs_at_end = v,
            |_| {},
        );
        indent(&b, at_end.upcast_ref());
        head(&b, "Split View");
        let ffm = check("_Focus the pane under the mouse pointer");
        ffm.set_tooltip_text(Some("With two panes open (F3), moving the pointer over a pane makes it the active one, without clicking (Shift+F3 switches this)"));
        ffm.set_action_name(Some("win.focus-follows-mouse"));
        indent(&b, ffm.upcast_ref());
        let delay = gtk::SpinButton::with_range(0.0, 2000.0, 50.0);
        delay.set_value(s.focus_follows_mouse_delay as f64);
        delay.set_tooltip_text(Some("How long the pointer has to stay over a pane before it becomes the active one, so passing over it on the way elsewhere (the side pane, say) doesn't change the active pane"));
        delay.connect_value_changed(|d| {
            let v = d.value_as_int();
            config::update(|s| s.focus_follows_mouse_delay = v);
        });
        ffm.bind_property("active", &delay, "sensitive").sync_create().build();
        let after = inline(&b, "After the pointer rests on it for", &[delay.upcast_ref(), gtk::Label::new(Some("ms")).upcast_ref()]);
        after.set_margin_start(after.margin_start() + 24);
        head(&b, "Trash");
        let confirm = setting_check("Ask before moving files to the T_rash", None, s.confirm_trash, |s, v| s.confirm_trash = v, |_| {});
        indent(&b, confirm.upcast_ref());
        head(&b, "Removable Media");
        let automount = check("Mount drives and media when they're _inserted");
        automount.set_tooltip_text(Some(&match crate::sidebar::automount_desktop() {
            Some(desktop) => format!("Flash drives, memory cards, discs… are mounted right away. This is {desktop}'s own setting: changing it here changes it there too. Turned off, they're mounted when you click them in the side pane."),
            None => "Flash drives, memory cards, discs… inserted while Arkha runs are mounted right away. Turned off, they're mounted when you click them in the side pane.".to_string(),
        }));
        automount.set_active(crate::sidebar::automount_on());
        automount.connect_toggled(|c| crate::sidebar::set_automount(c.is_active()));
        // It may have been changed in the desktop's settings meanwhile.
        d.connect_focus_in_event(glib::clone!(@weak automount => @default-return glib::Propagation::Proceed, move |_, _| {
            let on = crate::sidebar::automount_on();
            if automount.is_active() != on {
                automount.set_active(on);
            }
            glib::Propagation::Proceed
        }));
        indent(&b, automount.upcast_ref());
        let unmount_only = setting_check(
            "The eject button only _unmounts",
            Some("Clicking the button next to a drive in the side pane unmounts it, but doesn't eject it or power it off (the drive stays in the list, ready to be mounted again)"),
            s.unmount_only,
            |s, v| s.unmount_only = v,
            |_| crate::sidebar::refresh_all(),
        );
        indent(&b, unmount_only.upcast_ref());
        head(&b, "Scrolling");
        let auto = setting_check(
            "_Middle-click on empty space (or an item in the list view) to autoscroll",
            Some("Like Firefox: a marker appears where you clicked and the view scrolls towards the pointer, faster the further away it is. Click, press a key or use the wheel to stop (after dragging with the button held, letting go stops it). In the list view this works on files and folders too, if the list can scroll; otherwise, or with Shift, a middle-click opens them there (a folder in a new tab)."),
            s.autoscroll,
            |s, v| s.autoscroll = v,
            |_| {},
        );
        indent(&b, auto.upcast_ref());
        add_page("behavior", "Behavior", "view-more-symbolic", &b);

        // Display
        let dp = page();
        head(&dp, "Files");
        let hid = check("Show _hidden and backup files");
        hid.set_action_name(Some("win.show-hidden"));
        indent(&dp, hid.upcast_ref());
        let stripes = setting_check("_List view", None, s.striped_rows, |s, v| s.striped_rows = v, |_| for_all_panes(|p| p.redraw()));
        let rename_stripes = setting_check(
            "_Bulk rename list",
            Some("Shade every second row of the file list in the Rename All… window (applies the next time it opens)"),
            s.rename_striped_rows,
            |s, v| s.rename_striped_rows = v,
            |_| {},
        );
        inline(&dp, "Alternating row colors:", &[stripes.upcast_ref(), rename_stripes.upcast_ref()]);
        let iec = setting_check(
            "Show file sizes in _IEC units (KiB, MiB, GiB)",
            Some("Sizes in powers of 1024 (1 KiB = 1024 bytes) instead of powers of 1000 (1 kB = 1000 bytes)"),
            s.iec_units,
            |s, v| s.iec_units = v,
            |v| {
                util::set_iec_units(v);
                // Sizes are formatted when rows are made.
                for_all_panes(|p| p.reload());
            },
        );
        indent(&dp, iec.upcast_ref());
        head(&dp, "Window");
        let side = check("Show the side _pane");
        side.set_action_name(Some("win.show-sidebar"));
        indent(&dp, side.upcast_ref());
        let status = check("Show the status_bar");
        status.set_action_name(Some("win.show-statusbar"));
        indent(&dp, status.upcast_ref());
        let status_hidden = setting_check(
            "Say how many items are _hidden",
            Some("The status bar adds how many items of the folder are hidden, as in “12 items (3 hidden)”"),
            s.status_hidden,
            |s, v| s.status_hidden = v,
            |_| {
                for w in all_windows() {
                    if let Some(p) = w.current_pane() {
                        w.status.set_text(&p.status_text());
                    }
                }
            },
        );
        // A sub-option of the status bar: only of use while it's shown.
        status.bind_property("active", &status_hidden, "sensitive").sync_create().build();
        indent(&dp, status_hidden.upcast_ref());
        status_hidden.set_margin_start(status_hidden.margin_start() + 24);
        let tabs = setting_check(
            "Always show the _tab bar",
            Some("Also show it when a window has only one tab, so that tab can be dragged to another window or out into a new one"),
            s.always_show_tabs,
            |s, v| s.always_show_tabs = v,
            |_| all_windows().iter().for_each(|w| w.update_tabs_visibility()),
        );
        indent(&dp, tabs.upcast_ref());
        let text = setting_check(
            "Show te_xt on toolbar buttons",
            Some("The Back and Forward buttons show their name next to the icon"),
            s.toolbar_text,
            |s, v| s.toolbar_text = v,
            |v| {
                for w in all_windows() {
                    for l in &w.toolbar_labels {
                        l.set_visible(v);
                    }
                }
            },
        );
        indent(&dp, text.upcast_ref());
        head(&dp, "Location Bar");
        let as_text = setting_check(
            "Always show the _location as text",
            Some("Show the folder's path as editable text instead of buttons for each folder"),
            s.path_as_text,
            |s, v| s.path_as_text = v,
            |_| all_windows().iter().for_each(|w| w.apply_path_mode()),
        );
        indent(&dp, as_text.upcast_ref());
        let bar_button = |label: &str, tip: &str, on: bool, set: fn(&mut config::Settings, bool), widget: fn(&AppWindow) -> gtk::Widget| {
            setting_check(label, Some(tip), on, set, move |v| all_windows().iter().for_each(|w| widget(w).set_visible(v)))
        };
        let zoom_b = bar_button("_Zoom", "Zoom in, zoom out and the zoom level", s.show_zoom_buttons, |s, v| s.show_zoom_buttons = v, |w| w.zoom_box.clone().upcast());
        let view_b = bar_button("Vi_ew", "Icon, list and compact view", s.show_view_buttons, |s, v| s.show_view_buttons = v, |w| w.view_box.clone().upcast());
        let split_b = bar_button("_Split view", "Show or hide the extra pane (F3)", s.show_split_button, |s, v| s.show_split_button = v, |w| w.split_btn.clone().upcast());
        let info_b = bar_button("_Info panel", "Show or hide the info panel in the folder you're in (F11)", s.show_info_button, |s, v| s.show_info_button = v, |w| w.info_btn.clone().upcast());
        inline(&dp, "Buttons:", &[zoom_b.upcast_ref(), view_b.upcast_ref(), split_b.upcast_ref(), info_b.upcast_ref()]);
        add_page("display", "Display", "view-reveal-symbolic", &dp);

        // Previews: thumbnails and the Media Info tab
        let pv = page();
        head(&pv, "Thumbnails");
        let th = setting_check(
            "Show thumb_nails",
            Some("Images get thumbnails built in; other file types through the custom thumbnailers below, or the ones installed on the system"),
            s.thumbnails,
            |s, v| s.thumbnails = v,
            |_| for_all_panes(|p| p.reload()),
        );
        indent(&pv, th.upcast_ref());
        let thumbs = thumbnailers_editor(d.upcast_ref());
        thumbs.set_margin_start(40);
        pv.add(&thumbs);
        head(&pv, "Media Info");
        let mi = setting_check(
            "Show a _Media Info tab in file properties",
            Some("Shows what the mediainfo program reports about a file: codecs, resolution, duration, bit rate and more"),
            s.media_info,
            |s, v| s.media_info = v,
            |_| {},
        );
        indent(&pv, mi.upcast_ref());
        let kinds = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        kinds.add(&gtk::Label::new(Some("For:")));
        let kind = |label: &str, on: bool, set: fn(&mut config::Settings, bool)| kinds.add(&setting_check(label, None, on, set, |_| {}));
        kind("_Videos", s.media_info_video, |s, v| s.media_info_video = v);
        kind("_Audio", s.media_info_audio, |s, v| s.media_info_audio = v);
        kind("_Images", s.media_info_image, |s, v| s.media_info_image = v);
        kind("_Other files", s.media_info_other, |s, v| s.media_info_other = v);
        kinds.set_sensitive(s.media_info);
        {
            let kinds = kinds.clone();
            mi.connect_toggled(move |c| kinds.set_sensitive(c.is_active()));
        }
        indent(&pv, kinds.upcast_ref());
        // mediainfo is optional: without it the settings stay visible but
        // greyed out, so it's clear what installing it would add.
        if !props::mediainfo_available() {
            mi.set_sensitive(false);
            kinds.set_sensitive(false);
            let note = hint_label("Install mediainfo to use this: the tab shows codecs, resolution, duration, bit rate and more.");
            indent(&pv, note.upcast_ref());
        }
        head(&pv, "Info Panel");
        let note = hint_label("Beside the files (View → Info Panel, F11, or its toolbar button), in the folders where you turn it on. It shows the selected item: a folder's cover picture, an image, or a file's thumbnail. Only local folders are searched for covers.");
        indent(&pv, note.upcast_ref());
        let names = gtk::Entry::new();
        names.set_text(&s.cover_names);
        names.set_width_chars(36);
        names.set_tooltip_text(Some("File names to use as a folder's cover, separated by semicolons, in order of preference. Upper and lower case don't matter."));
        let apply_names = |e: &gtk::Entry| {
            let mut v = e.text().trim().to_string();
            if v.is_empty() {
                v = config::Settings::default().cover_names;
                e.set_text(&v);
            }
            if v != config::get().cover_names {
                config::update(|s| s.cover_names = v);
                for_all_panes(|p| p.refresh_info());
            }
        };
        names.connect_activate(apply_names);
        names.connect_focus_out_event(move |e, _| {
            apply_names(e);
            glib::Propagation::Proceed
        });
        option(&pv, "Cover file names:", names.upcast_ref());
        let choice = |items: &[(&str, &str)], current: &str, set: fn(&mut config::Settings, String)| {
            let c = gtk::ComboBoxText::new();
            for (id, label) in items {
                c.append(Some(id), label);
            }
            c.set_active_id(Some(current));
            c.connect_changed(move |c| {
                if let Some(id) = c.active_id() {
                    config::update(|s| set(s, id.to_string()));
                    for_all_panes(|p| p.refresh_info());
                }
            });
            c
        };
        let other = choice(&[("summary", "Show their number and total size"), ("last", "Keep showing the last item"), ("current", "Show the open folder"), ("hide", "Hide the panel")], &s.info_several, |s, v| s.info_several = v);
        option(&pv, "With several items selected:", other.upcast_ref());
        let missing = choice(&[("icon", "Show its icon and the details"), ("none", "Show nothing")], &s.info_no_cover, |s, v| s.info_no_cover = v);
        option(&pv, "Folders without a cover:", missing.upcast_ref());
        let details = setting_check("Show the _details (type, size, date, …)", None, s.info_details, |s, v| s.info_details = v, |_| for_all_panes(|p| p.refresh_info()));
        indent(&pv, details.upcast_ref());
        add_page("previews", "Previews", "image-x-generic-symbolic", &pv);

        // Side Pane
        let sp = page();
        head(&sp, "Computer Entries");
        let hint = hint_label("Unchecked entries are hidden. Drag an entry by its handle, or select it and use the arrows, to move it; they can also be dragged in the side pane itself.");
        indent(&sp, hint.upcast_ref());
        let sidebar = self.sidebar.clone();
        let (places, places_refresh) = places_editor(move || sidebar.places());
        indent(&sp, places.upcast_ref());
        // Entries dragged in the side pane while the dialog is open.
        d.connect_focus_in_event(move |_, _| {
            places_refresh();
            glib::Propagation::Proceed
        });
        head(&sp, "Appearance");
        let custom = check("Custom _row height:");
        let height = gtk::SpinButton::with_range(16.0, 80.0, 1.0);
        height.set_value(if s.sidebar_row_height > 0 { s.sidebar_row_height } else { 36 } as f64);
        custom.set_active(s.sidebar_row_height > 0);
        height.set_sensitive(custom.is_active());
        let set_height = {
            let (custom, height) = (custom.clone(), height.clone());
            move || {
                let h = if custom.is_active() { height.value_as_int() } else { 0 };
                height.set_sensitive(custom.is_active());
                config::update(|s| s.sidebar_row_height = h);
                apply_sidebar_css();
            }
        };
        {
            let f = set_height.clone();
            custom.connect_toggled(move |_| f());
        }
        height.connect_value_changed(move |_| set_height());
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hb.add(&custom);
        hb.add(&height);
        hb.add(&gtk::Label::new(Some("pixels")));
        indent(&sp, hb.upcast_ref());
        add_page("sidepane", "Side Pane", "view-list-symbolic", &sp);

        // Context Menu: order and visibility of the entries, per menu
        let cm = page();
        head(&cm, "Context Menu Entries");
        let hint = hint_label("Drag an entry by its handle, or select it and use the arrows, to move it. Unchecked entries are hidden. Entries that don't apply are left out of the menu (Restore outside the Trash, say).");
        indent(&cm, hint.upcast_ref());
        let menus = gtk::Stack::new();
        let (items_editor, items_refresh) = menu_order_editor(false);
        let (bg_editor, bg_refresh) = menu_order_editor(true);
        menus.add_titled(&items_editor, "items", "Files and Folders");
        menus.add_titled(&bg_editor, "background", "A Folder's Empty Space");
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&menus));
        switcher.set_halign(gtk::Align::Start);
        indent(&cm, switcher.upcast_ref());
        indent(&cm, menus.upcast_ref());
        add_page("contextmenu", "Context Menu", "open-menu-symbolic", &cm);

        // Custom Actions (in the context menus)
        let ca = page();
        head(&ca, "Custom Actions in the Context Menu");
        let editor = custom_actions_editor(d.upcast_ref());
        editor.set_margin_start(40);
        ca.add(&editor);
        add_page("customactions", "Custom Actions", "system-run-symbolic", &ca);

        // Keyboard
        if let Some(app) = self.window.application() {
            let kb = page();
            head(&kb, "Keyboard Shortcuts");
            let editor = keybindings_editor(d.upcast_ref(), &app);
            editor.set_margin_start(40);
            kb.pack_start(&editor, true, true, 0);
            add_page("keyboard", "Keyboard", "input-keyboard-symbolic", &kb);
        }

        {
            let stack = stack.clone();
            pages.connect_row_selected(move |_, row| {
                if let Some(r) = row {
                    // Custom actions may have changed since the lists were filled.
                    if r.widget_name() == "contextmenu" {
                        items_refresh();
                        bg_refresh();
                    }
                    stack.set_visible_child_name(&r.widget_name());
                }
            });
        }
        pages.select_row(pages.row_at_index(0).as_ref());
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hb.add(&pages);
        hb.add(&gtk::Separator::new(gtk::Orientation::Vertical));
        hb.add(&stack);
        let area = d.content_area();
        area.set_border_width(0);
        area.pack_start(&hb, true, true, 0);
        d.connect_response(|d, _| d.close());
        d.show_all();
    }

    fn about(&self) {
        let d = gtk::AboutDialog::new();
        d.set_transient_for(Some(&self.window));
        d.set_modal(true);
        d.set_program_name("Arkha");
        d.set_version(Some(env!("CARGO_PKG_VERSION")));
        d.set_comments(Some("A Caja-style file manager written in Rust and GTK 3."));
        d.set_logo_icon_name(Some("system-file-manager"));
        d.set_license_type(gtk::License::Gpl30);
        d.connect_response(|d, _| d.close());
        d.show();
    }

    fn shortcuts(&self) {
        let d = gtk::Dialog::with_buttons(Some("Keyboard Shortcuts"), Some(&self.window), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Close", gtk::ResponseType::Close)]);
        d.set_default_size(-1, 560);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(24);
        grid.set_border_width(14);
        let mut r = 0;
        let mut add = |k: &str, v: &str| {
            let kl = gtk::Label::new(None);
            if v.is_empty() {
                kl.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(k)));
                kl.set_margin_top(if r == 0 { 0 } else { 8 });
            } else {
                kl.set_text(k);
            }
            kl.set_xalign(0.0);
            grid.attach(&kl, 0, r, 1, 1);
            let vl = gtk::Label::new(Some(v));
            vl.set_xalign(0.0);
            vl.style_context().add_class("dim-label");
            grid.attach(&vl, 1, r, 1, 1);
            r += 1;
        };
        // The current shortcuts (they can be changed in Preferences).
        let mut group = "";
        for b in keys::BINDINGS {
            let current = keys::current(b.action);
            if current.is_empty() {
                continue;
            }
            if b.group != group {
                group = b.group;
                add(group, "");
            }
            add(&keys::labels(&current), b.label);
        }
        add("Mouse", "");
        add("Middle-click a folder", "Open in new tab");
        add("Drag with Ctrl / Shift / Alt", "Copy / Move / Ask");
        add("Ctrl+scroll", "Zoom");
        add("Back / forward mouse buttons", "Back / Forward");
        let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sw.set_propagate_natural_width(true);
        sw.add(&grid);
        d.content_area().pack_start(&sw, true, true, 0);
        d.connect_response(|d, _| d.close());
        d.show_all();
    }
}

const MENU_ROW_TARGET: &str = "ARKHA_MENU_ROW";
const PLACE_ROW_TARGET: &str = "ARKHA_PLACE_ROW";

/// A check box for a setting: `set` stores it when it's toggled, then
/// `then` applies it.
fn setting_check(label: &str, tip: Option<&str>, on: bool, set: fn(&mut config::Settings, bool), then: impl Fn(bool) + 'static) -> gtk::CheckButton {
    let c = gtk::CheckButton::with_mnemonic(label);
    c.set_tooltip_text(tip);
    c.set_active(on);
    c.connect_toggled(move |c| {
        let v = c.is_active();
        config::update(|s| set(s, v));
        then(v);
    });
    c
}

/// A dimmed explanation under a setting, wrapped.
fn hint_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(60);
    l.style_context().add_class("dim-label");
    l
}

/// A list the user puts in order, for the preferences: entries by key,
/// each row with a handle to drag it by; the selected one moves with the
/// arrows (see arrows). The context menus and the side pane's places use it.
struct OrderList {
    widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    order: Rc<RefCell<Vec<String>>>,
    /// Fills the list from `order`, selecting row `select`.
    rebuild: Rc<dyn Fn(Option<usize>)>,
    /// Moves an entry, tells the list's owner (`changed`) and rebuilds.
    move_to: Rc<dyn Fn(usize, usize)>,
}

impl OrderList {
    fn selected(&self) -> Option<usize> {
        self.list.selected_row().map(|r| r.index() as usize)
    }

    /// Buttons moving the selected entry up and down.
    fn arrows(self: &Rc<Self>) -> (gtk::Button, gtk::Button) {
        let up = gtk::Button::from_icon_name(Some("go-up-symbolic"), gtk::IconSize::Button);
        up.set_tooltip_text(Some("Move up"));
        let down = gtk::Button::from_icon_name(Some("go-down-symbolic"), gtk::IconSize::Button);
        down.set_tooltip_text(Some("Move down"));
        let ol = self.clone();
        up.connect_clicked(move |_| {
            if let Some(i) = ol.selected().filter(|i| *i > 0) {
                (ol.move_to)(i, i - 1);
            }
        });
        let ol = self.clone();
        down.connect_clicked(move |_| {
            if let Some(i) = ol.selected().filter(|i| i + 1 < ol.order.borrow().len()) {
                (ol.move_to)(i, i + 1);
            }
        });
        (up, down)
    }
}

/// An OrderList whose rows show `content(key)` beside their handle;
/// `changed` gets the new order when the user moves an entry. `target` is
/// the rows' own kind of drag: they can only be dropped in their list.
fn order_list(target: &'static str, content: impl Fn(&str) -> gtk::Widget + 'static, changed: impl Fn(&[String]) + 'static) -> Rc<OrderList> {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_min_content_height(150);
    // Grows with the preferences window (the boxes around pass it on).
    sw.set_vexpand(true);
    sw.add(&list);
    let order: Rc<RefCell<Vec<String>>> = Rc::default();
    let handle_icon = if gtk::IconTheme::default().is_some_and(|t| t.has_icon("list-drag-handle-symbolic")) { "list-drag-handle-symbolic" } else { "open-menu-symbolic" };
    let rebuild: Rc<dyn Fn(Option<usize>)> = {
        let (list, order, sw) = (list.clone(), order.clone(), sw.clone());
        Rc::new(move |select| {
            for c in list.children() {
                list.remove(&c);
            }
            for (i, k) in order.borrow().iter().enumerate() {
                let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                hb.set_margin_start(4);
                hb.set_margin_end(8);
                let handle = gtk::EventBox::new();
                handle.add(&gtk::Image::from_icon_name(Some(handle_icon), gtk::IconSize::Menu));
                handle.set_tooltip_text(Some("Drag to move"));
                let targets = [gtk::TargetEntry::new(target, gtk::TargetFlags::SAME_APP, 0)];
                handle.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::MOVE);
                handle.connect_drag_begin(|h, ctx| {
                    if let Some(row) = h.ancestor(gtk::ListBoxRow::static_type()) {
                        crate::sidebar::drag_picture(&row, ctx);
                    }
                });
                // A custom target, so the row's index goes as raw bytes.
                handle.connect_drag_data_get(move |_, _, data, _, _| {
                    data.set(&data.target(), 8, i.to_string().as_bytes());
                });
                hb.add(&handle);
                let c = content(k);
                c.set_hexpand(true);
                hb.add(&c);
                let row = gtk::ListBoxRow::new();
                row.add(&hb);
                list.add(&row);
            }
            list.show_all();
            if let Some(r) = select.and_then(|i| list.row_at_index(i as i32)) {
                list.select_row(Some(&r));
                // Keep the moved entry in view (once the new rows have
                // their sizes).
                let adj = sw.vadjustment();
                glib::idle_add_local_once(move || {
                    // Not once the dialog is closed (see
                    // Pane::scroll_columns_to_end).
                    if !r.is_realized() {
                        return;
                    }
                    let a = r.allocation();
                    adj.clamp_page(a.y() as f64, (a.y() + a.height()) as f64);
                });
            }
        })
    };
    let move_to: Rc<dyn Fn(usize, usize)> = {
        let (order, rebuild) = (order.clone(), rebuild.clone());
        Rc::new(move |from, to| {
            let to = {
                let mut o = order.borrow_mut();
                if from >= o.len() || from == to {
                    return;
                }
                let k = o.remove(from);
                let to = to.min(o.len());
                o.insert(to, k);
                to
            };
            changed(&order.borrow());
            rebuild(Some(to));
        })
    };
    // Dropping a dragged row: it takes the place of the row it's dropped on.
    let targets = [gtk::TargetEntry::new(target, gtk::TargetFlags::SAME_APP, 0)];
    list.drag_dest_set(gtk::DestDefaults::empty(), &targets, gdk::DragAction::MOVE);
    list.connect_drag_motion(|l, ctx, _, y, time| {
        match l.row_at_y(y) {
            Some(r) => l.drag_highlight_row(&r),
            None => l.drag_unhighlight_row(),
        }
        ctx.drag_status(gdk::DragAction::MOVE, time);
        true
    });
    list.connect_drag_leave(|l, _, _| l.drag_unhighlight_row());
    // After a drag the list misses the next click (its click tracking
    // never saw the drag's button release), so rows are selected on press
    // here. Root coordinates: the press may come from a row's own widgets.
    list.connect_button_press_event(|l, e| {
        if e.button() == 1 {
            if let Some((_, oy)) = l.window().map(|w| w.origin()).map(|(_, x, y)| (x, y)) {
                if let Some(r) = l.row_at_y(e.root().1 as i32 - oy) {
                    l.select_row(Some(&r));
                }
            }
        }
        glib::Propagation::Proceed
    });
    list.connect_drag_drop(|l, ctx, _, _, time| {
        match l.drag_dest_find_target(ctx, None) {
            Some(t) => l.drag_get_data(ctx, &t, time),
            None => ctx.drag_finish(false, false, time),
        }
        true
    });
    {
        let (order, move_to) = (order.clone(), move_to.clone());
        list.connect_drag_data_received(move |l, ctx, _, y, data, _, time| {
            l.drag_unhighlight_row();
            let from = String::from_utf8(data.data()).ok().and_then(|t| t.parse::<usize>().ok());
            let last = order.borrow().len().saturating_sub(1);
            let to = l.row_at_y(y).map_or(last, |r| r.index() as usize);
            ctx.drag_finish(from.is_some(), false, time);
            if let Some(from) = from {
                move_to(from, to);
            }
        });
    }
    Rc::new(OrderList { widget: sw, list, order, rebuild, move_to })
}

/// A checkbox showing or hiding an entry: `key` in the hidden list that
/// `hidden` picks out of the settings. `then` runs after a change.
fn shown_check(label: &gtk::Widget, key: &str, hidden: fn(&mut config::Settings) -> &mut Vec<String>, then: fn()) -> gtk::CheckButton {
    let cb = gtk::CheckButton::new();
    cb.add(label);
    cb.set_active(!config::with(|s| hidden(&mut s.clone()).iter().any(|h| h == key)));
    let key = key.to_string();
    cb.connect_toggled(move |c| {
        let show = c.is_active();
        config::update(|s| {
            let list = hidden(s);
            list.retain(|x| *x != key);
            if !show {
                list.push(key.clone());
            }
        });
        then();
    });
    cb
}

/// The order of one context menu (`bg`: the one for a folder's empty space)
/// as a list: entries are moved by dragging their handle or with the arrow
/// buttons, shown or hidden with their checkbox, and separators can be
/// added and removed. Returns the editor and a function that reloads it
/// (custom actions may have been added or removed meanwhile).
fn menu_order_editor(bg: bool) -> (gtk::Box, Rc<dyn Fn()>) {
    let defaults = if bg { actions::BACKGROUND_ORDER } else { actions::ITEM_ORDER };
    let labels = if bg { actions::BACKGROUND_ENTRIES } else { actions::ITEM_ENTRIES };
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let save = move |o: &[String]| {
        let o = o.to_vec();
        config::update(|s| if bg { s.menu_order_bg = o } else { s.menu_order = o });
    };
    let row = move |k: &str| -> gtk::Widget {
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if k == actions::SEPARATOR {
            let l = gtk::Label::new(Some("Separator"));
            l.style_context().add_class("dim-label");
            hb.add(&l);
            let line = gtk::Separator::new(gtk::Orientation::Horizontal);
            line.set_hexpand(true);
            line.set_valign(gtk::Align::Center);
            hb.add(&line);
            return hb.upcast();
        }
        let action = actions::load().into_iter().find(|a| actions::custom_key(a) == k);
        let label = match &action {
            Some(a) => a.name.clone(),
            None => labels.iter().find(|(key, _)| *key == k).map_or(k.to_string(), |(_, l)| l.to_string()),
        };
        hb.add(&shown_check(gtk::Label::new(Some(&label)).upcast_ref(), k, |s| &mut s.hidden_menu_items, || {}));
        if action.is_some() {
            let l = gtk::Label::new(Some("custom action"));
            l.style_context().add_class("dim-label");
            hb.add(&l);
        }
        hb.upcast()
    };
    let ol = order_list(MENU_ROW_TARGET, row, save);
    outer.add(&ol.widget);
    let refresh: Rc<dyn Fn()> = {
        let ol = ol.clone();
        Rc::new(move || {
            let custom = actions::load();
            let keys: Vec<String> = custom.iter().filter(|a| if bg { a.background } else { a.files || a.folders }).map(actions::custom_key).collect();
            let cfg = config::get();
            *ol.order.borrow_mut() = actions::menu_order(if bg { &cfg.menu_order_bg } else { &cfg.menu_order }, defaults, &keys);
            (ol.rebuild)(None);
        })
    };
    refresh();
    let (up, down) = ol.arrows();
    let add_sep = gtk::Button::with_mnemonic("Add _Separator");
    add_sep.set_tooltip_text(Some("Add a separator below the selected entry"));
    let remove_sep = gtk::Button::with_mnemonic("Re_move Separator");
    remove_sep.set_sensitive(false);
    let reset = gtk::Button::with_mnemonic("Reset _Order");
    reset.set_tooltip_text(Some("Put every entry back in its original place (which entries are hidden stays as it is)"));
    {
        let ol = ol.clone();
        add_sep.connect_clicked(move |_| {
            let at = ol.selected().map_or(ol.order.borrow().len(), |i| i + 1);
            ol.order.borrow_mut().insert(at, actions::SEPARATOR.to_string());
            save(&ol.order.borrow());
            (ol.rebuild)(Some(at));
        });
    }
    {
        let ol = ol.clone();
        remove_sep.connect_clicked(move |_| {
            let Some(i) = ol.selected() else { return };
            if ol.order.borrow().get(i).map(String::as_str) != Some(actions::SEPARATOR) {
                return;
            }
            ol.order.borrow_mut().remove(i);
            save(&ol.order.borrow());
            let last = ol.order.borrow().len().saturating_sub(1);
            (ol.rebuild)(Some(i.min(last)));
        });
    }
    {
        let refresh = refresh.clone();
        reset.connect_clicked(move |_| {
            config::update(|s| if bg { s.menu_order_bg.clear() } else { s.menu_order.clear() });
            refresh();
        });
    }
    {
        let (order, remove_sep) = (ol.order.clone(), remove_sep.clone());
        ol.list.connect_row_selected(move |_, row| {
            let sep = row.and_then(|r| order.borrow().get(r.index() as usize).cloned()).as_deref() == Some(actions::SEPARATOR);
            remove_sep.set_sensitive(sep);
        });
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.add(&up);
    buttons.add(&down);
    buttons.add(&add_sep);
    buttons.add(&remove_sep);
    buttons.pack_end(&reset, false, false, 0);
    outer.add(&buttons);
    // The View submenu keeps its own order; its entries can be hidden.
    if bg {
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hb.add(&gtk::Label::new(Some("View submenu:")));
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(4);
        flow.set_column_spacing(12);
        flow.set_hexpand(true);
        for key in actions::VIEW_SUBMENU {
            let label = labels.iter().find(|(k, _)| k == key).map_or(*key, |(_, l)| l);
            let l = gtk::Label::new(Some(label.trim_start_matches("View › ")));
            flow.add(&shown_check(l.upcast_ref(), key, |s| &mut s.hidden_menu_items, || {}));
        }
        hb.add(&flow);
        outer.add(&hb);
    }
    (outer, refresh)
}

/// The side pane's Computer entries: which are shown (their checkbox) and
/// in which order (dragging their handle, the arrows, or dragging them in
/// the side pane itself). Returns the editor and a function that reloads
/// it (they may have been dragged in the side pane meanwhile).
fn places_editor(places: impl Fn() -> Vec<crate::sidebar::Place> + 'static) -> (gtk::Box, Rc<dyn Fn()>) {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let shown: Rc<RefCell<Vec<crate::sidebar::Place>>> = Rc::default();
    let row = {
        let shown = shown.clone();
        move |k: &str| -> gtk::Widget {
            let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            if let Some(p) = shown.borrow().iter().find(|p| p.key == k) {
                if let Some(icon) = &p.icon {
                    hb.add(&gtk::Image::from_gicon(icon, gtk::IconSize::Menu));
                }
                hb.add(&gtk::Label::new(Some(&p.label)));
            }
            // In a box: the rest of the row selects it, not the checkbox.
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add(&shown_check(hb.upcast_ref(), k, |s| &mut s.hidden_places, crate::sidebar::refresh_all));
            row.upcast()
        }
    };
    let ol = order_list(PLACE_ROW_TARGET, row, |o| {
        let o = o.to_vec();
        config::update(|s| s.places_order = o);
        crate::sidebar::refresh_all();
    });
    outer.add(&ol.widget);
    let refresh: Rc<dyn Fn()> = {
        let ol = ol.clone();
        Rc::new(move || {
            let mut p = places();
            let mut seen = vec![];
            p.retain(|p| {
                let new = !seen.contains(&p.key);
                seen.push(p.key.clone());
                new
            });
            // Unchanged: the rows stay. (The dialog gets the focus back
            // during a drag of a row on Wayland, and rebuilt, the dragged
            // row would be gone before it gives its data.)
            let keys: Vec<String> = p.iter().map(|p| p.key.clone()).collect();
            let same_labels = shown.borrow().iter().map(|p| &p.label).eq(p.iter().map(|p| &p.label));
            if *ol.order.borrow() == keys && same_labels {
                return;
            }
            *ol.order.borrow_mut() = keys;
            *shown.borrow_mut() = p;
            let sel = ol.selected();
            (ol.rebuild)(sel);
        })
    };
    refresh();
    let (up, down) = ol.arrows();
    let reset = gtk::Button::with_mnemonic("Reset _Order");
    reset.set_tooltip_text(Some("Put every entry back in its usual place (which entries are hidden stays as it is)"));
    {
        let refresh = refresh.clone();
        reset.connect_clicked(move |_| {
            config::update(|s| s.places_order.clear());
            crate::sidebar::refresh_all();
            refresh();
        });
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.add(&up);
    buttons.add(&down);
    buttons.pack_end(&reset, false, false, 0);
    outer.add(&buttons);
    (outer, refresh)
}

/// List of the GTK bookmarks (shared with Caja and file choosers) with rename,
/// reorder, add and remove. Changes are written straight to the bookmarks file.
fn bookmark_editor(parent: &gtk::Window) -> gtk::Box {
    let store = gtk::ListStore::new(&[String::static_type(), String::static_type()]);
    for (uri, label) in util::read_bookmarks() {
        store.insert_with_values(None, &[(0, &label), (1, &uri)]);
    }
    let tv = gtk::TreeView::with_model(&store);
    tv.set_reorderable(true);
    let name_r = gtk::CellRendererText::new();
    name_r.set_editable(true);
    let c1 = gtk::TreeViewColumn::new();
    c1.set_title("Name");
    CellLayoutExt::pack_start(&c1, &name_r, true);
    CellLayoutExt::add_attribute(&c1, &name_r, "text", 0);
    c1.set_expand(true);
    let c2 = gtk::TreeViewColumn::new();
    let loc_r = gtk::CellRendererText::new();
    loc_r.set_property("ellipsize", pango::EllipsizeMode::Middle);
    c2.set_title("Location");
    CellLayoutExt::pack_start(&c2, &loc_r, true);
    CellLayoutExt::add_attribute(&c2, &loc_r, "text", 1);
    c2.set_expand(true);
    tv.append_column(&c1);
    tv.append_column(&c2);
    let save = {
        let store = store.clone();
        move || {
            let list: Vec<(String, String)> = store.rows().iter().map(|it| (store.gv::<String>(it, 1), store.gv::<String>(it, 0))).collect();
            let _ = util::edit_bookmarks(|b| *b = list);
        }
    };
    let save = Rc::new(save);
    {
        let store = store.clone();
        let save = save.clone();
        name_r.connect_edited(move |_, path, text| {
            if let Some(it) = store.iter(&path) {
                store.set(&it, &[(0, &text.to_string())]);
                save();
            }
        });
    }
    {
        let save = save.clone();
        store.connect_row_deleted(move |_, _| save());
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&tv);
    sw.set_vexpand(true);
    let rm = gtk::Button::with_mnemonic("_Remove");
    {
        let tv = tv.clone();
        let store = store.clone();
        rm.connect_clicked(move |_| {
            if let Some((_, it)) = tv.selection().selected() {
                store.remove(&it);
            }
        });
    }
    let hint = gtk::Label::new(Some("Drag to reorder. Double-click a name to rename it."));
    hint.style_context().add_class("dim-label");
    hint.set_xalign(0.0);
    let add = gtk::Button::with_mnemonic("_Add…");
    {
        let store = store.clone();
        let parent = parent.clone();
        add.connect_clicked(move |_| {
            let fc = gtk::FileChooserDialog::with_buttons(
                Some("Add Bookmark"),
                Some(&parent),
                gtk::FileChooserAction::SelectFolder,
                &[("_Cancel", gtk::ResponseType::Cancel), ("_Add", gtk::ResponseType::Accept)],
            );
            if fc.run() == gtk::ResponseType::Accept {
                if let Some(f) = fc.file() {
                    let uri = f.uri().to_string();
                    if let Ok(true) = util::add_bookmark(&uri) {
                        let label = f.basename().map(|b| b.to_string_lossy().to_string()).unwrap_or_else(|| uri.clone());
                        store.insert_with_values(None, &[(0, &label), (1, &uri)]);
                    }
                }
            }
            fc.close();
        });
    }
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.pack_start(&sw, true, true, 0);
    let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hb.pack_start(&hint, true, true, 0);
    hb.pack_end(&rm, false, false, 0);
    hb.pack_end(&add, false, false, 0);
    bx.pack_start(&hb, false, false, 0);
    bx
}

/// Every action with its shortcuts, grouped. Double-click (or Change…) to
/// record a new shortcut; Clear removes it, Reset brings back the default.
fn keybindings_editor(parent: &gtk::Window, app: &gtk::Application) -> gtk::Box {
    // label, shortcuts, action ("" for group rows), weight
    let store = gtk::TreeStore::new(&[String::static_type(), String::static_type(), String::static_type(), i32::static_type()]);
    let refill = {
        let store = store.clone();
        move || {
            store.clear();
            let mut group: Option<(&str, gtk::TreeIter)> = None;
            for b in keys::BINDINGS {
                if group.as_ref().map(|(g, _)| *g) != Some(b.group) {
                    let it = store.insert_with_values(None, None, &[(0, &b.group), (1, &""), (2, &""), (3, &700)]);
                    group = Some((b.group, it));
                }
                let parent = group.as_ref().map(|(_, it)| *it);
                // Changed shortcuts are shown in bold.
                let weight = if keys::is_default(b.action) { 400 } else { 700 };
                let current = keys::current(b.action);
                let text = if current.is_empty() { "—".to_string() } else { keys::labels(&current) };
                store.insert_with_values(parent.as_ref(), None, &[(0, &b.label), (1, &text), (2, &b.action), (3, &weight)]);
            }
        }
    };
    refill();
    let tv = gtk::TreeView::with_model(&store);
    tv.set_show_expanders(false);
    tv.set_level_indentation(0);
    tv.set_enable_search(true);
    tv.set_search_column(0);
    for (i, title) in ["Action", "Shortcut"].into_iter().enumerate() {
        let r = gtk::CellRendererText::new();
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_expand(true);
        CellLayoutExt::pack_start(&c, &r, true);
        CellLayoutExt::add_attribute(&c, &r, "text", i as i32);
        CellLayoutExt::add_attribute(&c, &r, "weight", 3);
        tv.append_column(&c);
    }
    tv.expand_all();
    let selected = {
        let tv = tv.clone();
        move || -> Option<String> {
            let (m, it) = tv.selection().selected()?;
            let action: String = m.value(&it, 2).get().ok()?;
            (!action.is_empty()).then_some(action)
        }
    };
    let after = {
        let (refill, tv, selected) = (refill.clone(), tv.clone(), selected.clone());
        move || {
            let keep = selected();
            refill();
            tv.expand_all();
            // Re-select the row that was changed.
            if let Some(action) = keep {
                let i = keys::BINDINGS.iter().position(|b| b.action == action).unwrap_or(0);
                let group_idx = keys::BINDINGS[..=i].iter().fold((0, ""), |(n, g), b| if b.group != g { (n + 1, b.group) } else { (n, g) }).0 - 1;
                let in_group = keys::BINDINGS[..i].iter().filter(|b| b.group == keys::BINDINGS[i].group).count();
                let path = gtk::TreePath::from_indicesv(&[group_idx, in_group as i32]);
                tv.selection().select_path(&path);
                tv.scroll_to_cell(Some(&path), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
            }
        }
    };
    let after = Rc::new(after);
    let change = {
        let (parent, app, after, selected) = (parent.clone(), app.clone(), after.clone(), selected.clone());
        move || {
            let Some(action) = selected() else { return };
            let Some(b) = keys::BINDINGS.iter().find(|b| b.action == action) else { return };
            let Some(accel) = capture_shortcut(&parent, b.label) else { return };
            if let Some(other) = keys::used_by(&accel, &action) {
                let q = gtk::MessageDialog::new(
                    Some(&parent),
                    gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
                    gtk::MessageType::Question,
                    gtk::ButtonsType::None,
                    &format!("“{}” is already the shortcut for “{}”", keys::labels(std::slice::from_ref(&accel)), other.label),
                );
                q.set_secondary_text(Some(&format!("Use it for “{}” instead? “{}” will no longer have it.", b.label, other.label)));
                q.add_button("_Cancel", gtk::ResponseType::Cancel);
                q.add_button("_Reassign", gtk::ResponseType::Accept);
                let ok = q.run() == gtk::ResponseType::Accept;
                q.close();
                if !ok {
                    return;
                }
                let want = gtk::accelerator_parse(&accel);
                let rest: Vec<String> = keys::current(other.action).into_iter().filter(|a| gtk::accelerator_parse(a) != want).collect();
                keys::set(&app, other.action, Some(rest));
            }
            keys::set(&app, &action, Some(vec![accel]));
            after();
        }
    };
    let change = Rc::new(change);
    let btn_change = gtk::Button::with_mnemonic("C_hange…");
    let btn_clear = gtk::Button::with_mnemonic("C_lear");
    btn_clear.set_tooltip_text(Some("Remove the shortcut"));
    let btn_reset = gtk::Button::with_mnemonic("R_eset");
    btn_reset.set_tooltip_text(Some("Go back to the default shortcut"));
    let btn_reset_all = gtk::Button::with_mnemonic("Reset _All");
    btn_reset_all.set_tooltip_text(Some("Go back to the default shortcuts for every action"));
    {
        let change = change.clone();
        btn_change.connect_clicked(move |_| change());
    }
    {
        let change = change.clone();
        tv.connect_row_activated(move |_, _, _| change());
    }
    for (btn, clear) in [(&btn_clear, true), (&btn_reset, false)] {
        let (app, after, selected) = (app.clone(), after.clone(), selected.clone());
        btn.connect_clicked(move |_| {
            if let Some(action) = selected() {
                keys::set(&app, &action, if clear { Some(vec![]) } else { None });
                after();
            }
        });
    }
    {
        let (app, after) = (app.clone(), after.clone());
        btn_reset_all.connect_clicked(move |_| {
            keys::reset_all(&app);
            after();
        });
    }
    let sync = {
        let (b1, b2, b3) = (btn_change.clone(), btn_clear.clone(), btn_reset.clone());
        move |action: Option<String>| {
            b1.set_sensitive(action.is_some());
            b2.set_sensitive(action.as_deref().is_some_and(|a| !keys::current(a).is_empty()));
            b3.set_sensitive(action.as_deref().is_some_and(|a| !keys::is_default(a)));
        }
    };
    sync(None);
    {
        let selected = selected.clone();
        tv.selection().connect_changed(move |_| sync(selected()));
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.pack_start(&btn_change, false, false, 0);
    buttons.pack_start(&btn_clear, false, false, 0);
    buttons.pack_start(&btn_reset, false, false, 0);
    buttons.pack_end(&btn_reset_all, false, false, 0);
    let hint = gtk::Label::new(Some("Double-click an action to give it a new shortcut. Changed shortcuts are shown in bold."));
    hint.set_xalign(0.0);
    hint.set_line_wrap(true);
    hint.style_context().add_class("dim-label");
    let frame = gtk::Frame::new(None);
    frame.add(&tv);
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.add(&buttons);
    bx.add(&hint);
    bx.pack_start(&frame, true, true, 0);
    bx
}

/// Waits for a key combination; None if cancelled with Escape or Cancel.
fn capture_shortcut(parent: &gtk::Window, label: &str) -> Option<String> {
    let d = gtk::Dialog::with_buttons(Some("Set Shortcut"), Some(parent), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Cancel", gtk::ResponseType::Cancel)]);
    let msg = gtk::Label::new(None);
    msg.set_markup(&format!("Press the new shortcut for <b>{}</b>,\nor Escape to cancel.", glib::markup_escape_text(label)));
    msg.set_justify(gtk::Justification::Center);
    let note = gtk::Label::new(None);
    note.style_context().add_class("dim-label");
    let bx = d.content_area();
    bx.set_spacing(8);
    bx.set_border_width(18);
    bx.add(&msg);
    bx.add(&note);
    let result: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let result = result.clone();
        d.connect_key_press_event(move |d, ev| {
            if ev.is_modifier() {
                return glib::Propagation::Stop;
            }
            let key = ev.keyval().to_lower();
            let mods = ev.state() & gtk::accelerator_get_default_mod_mask();
            if key == gdk::keys::constants::Escape && mods.is_empty() {
                d.response(gtk::ResponseType::Cancel);
                return glib::Propagation::Stop;
            }
            // A plain character key would get in the way of typing names.
            let typing = key.to_unicode().is_some_and(|c| !c.is_control())
                && !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::MOD1_MASK | gdk::ModifierType::SUPER_MASK);
            if typing || !gtk::accelerator_valid(*key, mods) {
                note.set_text("That key needs Ctrl, Alt or Super with it.");
                return glib::Propagation::Stop;
            }
            *result.borrow_mut() = Some(gtk::accelerator_name(*key, mods).map(|n| n.to_string()).unwrap_or_default());
            d.response(gtk::ResponseType::Ok);
            glib::Propagation::Stop
        });
    }
    d.show_all();
    let ok = d.run() == gtk::ResponseType::Ok;
    d.close();
    let r = result.borrow_mut().take();
    if ok { r.filter(|a| !a.is_empty()) } else { None }
}

/// List of custom thumbnailers (commands for file types the built-in image
/// thumbnailer doesn't do) with Add / Edit / Remove; saved straight away.
fn thumbnailers_editor(parent: &gtk::Window) -> gtk::Box {
    let list: Rc<RefCell<Vec<thumbnailers::Thumbnailer>>> = Rc::new(RefCell::new(thumbnailers::load()));
    let store = gtk::ListStore::new(&[String::static_type(), String::static_type(), String::static_type()]);
    let refill = {
        let (store, list) = (store.clone(), list.clone());
        move || {
            store.clear();
            for t in list.borrow().iter() {
                // Ones whose program is missing are skipped when making thumbnails.
                let name = if thumbnailers::installed(t) { t.name.clone() } else { format!("{} (not installed)", t.name) };
                store.insert_with_values(None, &[(0, &name), (1, &t.types), (2, &t.command)]);
            }
        }
    };
    refill();
    let tv = gtk::TreeView::with_model(&store);
    for (i, title) in ["Name", "File types", "Command"].into_iter().enumerate() {
        let r = gtk::CellRendererText::new();
        r.set_property("ellipsize", pango::EllipsizeMode::End);
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_expand(true);
        c.set_resizable(true);
        CellLayoutExt::pack_start(&c, &r, true);
        CellLayoutExt::add_attribute(&c, &r, "text", i as i32);
        tv.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_min_content_height(90);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.add(&tv);
    let selected = {
        let tv = tv.clone();
        move || tv.selection().selected().and_then(|(m, it)| m.path(&it)?.indices().first().map(|&i| i as usize))
    };
    // Save, and redo thumbnails in the open folders with the new list.
    let changed = {
        let (list, refill) = (list.clone(), refill.clone());
        move || {
            thumbnailers::save(&list.borrow());
            icons::reload_thumbnailers();
            refill();
            for_all_panes(|p| p.reload());
        }
    };
    let changed = Rc::new(changed);
    let add = gtk::Button::with_mnemonic("Add…");
    let edit = gtk::Button::with_mnemonic("Edit…");
    let remove = gtk::Button::with_mnemonic("Remove");
    {
        let (list, changed, parent) = (list.clone(), changed.clone(), parent.clone());
        add.connect_clicked(move |_| {
            if let Some(t) = edit_thumbnailer(&parent, "Add Thumbnailer", &thumbnailers::Thumbnailer::default()) {
                list.borrow_mut().push(t);
                changed();
            }
        });
    }
    let do_edit = {
        let (list, changed, parent, selected) = (list.clone(), changed.clone(), parent.clone(), selected.clone());
        Rc::new(move || {
            let Some(i) = selected() else { return };
            let cur = list.borrow()[i].clone();
            if let Some(t) = edit_thumbnailer(&parent, "Edit Thumbnailer", &cur) {
                list.borrow_mut()[i] = t;
                changed();
            }
        })
    };
    {
        let do_edit = do_edit.clone();
        edit.connect_clicked(move |_| do_edit());
    }
    tv.connect_row_activated(move |_, _, _| do_edit());
    {
        let (list, changed, selected) = (list.clone(), changed.clone(), selected.clone());
        remove.connect_clicked(move |_| {
            if let Some(i) = selected() {
                list.borrow_mut().remove(i);
                changed();
            }
        });
    }
    {
        let (edit, remove) = (edit.clone(), remove.clone());
        let sync = move |on: bool| {
            edit.set_sensitive(on);
            remove.set_sensitive(on);
        };
        sync(false);
        tv.selection().connect_changed(move |_| sync(selected().is_some()));
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.pack_start(&add, false, false, 0);
    buttons.pack_start(&edit, false, false, 0);
    buttons.pack_start(&remove, false, false, 0);
    let hint = hint_label("Custom thumbnailers make thumbnails for other file types, e.g. videos with ffmpeg. The first one whose file types match and whose program is installed is used. File types none of them handles use the thumbnailers installed on the system, as Caja does: PDF with Evince, office documents, audio cover art and so on.");
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.add(&sw);
    bx.add(&buttons);
    bx.add(&hint);
    bx
}

/// Dialog to create or change a custom thumbnailer; None if cancelled.
fn edit_thumbnailer(parent: &gtk::Window, title: &str, cur: &thumbnailers::Thumbnailer) -> Option<thumbnailers::Thumbnailer> {
    let d = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Cancel", gtk::ResponseType::Cancel), ("_Save", gtk::ResponseType::Ok)],
    );
    d.set_default_response(gtk::ResponseType::Ok);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_border_width(12);
    let entry = |text: &str, placeholder: &str| {
        let e = gtk::Entry::new();
        e.set_text(text);
        e.set_placeholder_text(Some(placeholder));
        e.set_activates_default(true);
        e.set_hexpand(true);
        e
    };
    let name = entry(&cur.name, "e.g. Videos");
    let types = entry(&cur.types, "e.g. video/*");
    let command = entry(&cur.command, "e.g. ffmpegthumbnailer -i %i -o %o -s %s -f");
    command.set_width_chars(56);
    let help = gtk::Label::new(Some("File types are MIME types, separated by “;” (“*” matches anything). In the command, %i is the file, %o the image to write (PNG or JPEG) and %s the size in pixels. It runs with sh and may take up to 30 seconds."));
    help.set_line_wrap(true);
    help.set_max_width_chars(56);
    help.set_xalign(0.0);
    help.style_context().add_class("dim-label");
    let rows: [(&str, &gtk::Widget); 4] = [("_Name:", name.upcast_ref()), ("File _types:", types.upcast_ref()), ("_Command:", command.upcast_ref()), ("", help.upcast_ref())];
    for (i, (t, w)) in rows.iter().enumerate() {
        let l = gtk::Label::with_mnemonic(t);
        l.set_xalign(0.0);
        l.set_mnemonic_widget(Some(*w));
        grid.attach(&l, 0, i as i32, 1, 1);
        grid.attach(*w, 1, i as i32, 1, 1);
    }
    d.content_area().add(&grid);
    let validate = {
        let (d, types, command) = (d.clone(), types.clone(), command.clone());
        move || d.set_response_sensitive(gtk::ResponseType::Ok, !types.text().trim().is_empty() && !command.text().trim().is_empty())
    };
    validate();
    for e in [&types, &command] {
        let v = validate.clone();
        e.connect_changed(move |_| v());
    }
    d.show_all();
    let ok = d.run() == gtk::ResponseType::Ok;
    let types_text = types.text().trim().to_string();
    let result = thumbnailers::Thumbnailer {
        name: Some(name.text().trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| types_text.clone()),
        types: types_text,
        command: command.text().trim().to_string(),
        system: false,
    };
    d.close();
    ok.then_some(result)
}

/// List of custom context menu actions with Add / Edit / Remove / reorder;
/// every change is saved straight away.
fn custom_actions_editor(parent: &gtk::Window) -> gtk::Box {
    let list: Rc<RefCell<Vec<actions::CustomAction>>> = Rc::new(RefCell::new(actions::load()));
    let store = gtk::ListStore::new(&[String::static_type(), String::static_type()]);
    let refill = {
        let (store, list) = (store.clone(), list.clone());
        move || {
            store.clear();
            for a in list.borrow().iter() {
                store.insert_with_values(None, &[(0, &a.name), (1, &a.command)]);
            }
        }
    };
    refill();
    let tv = gtk::TreeView::with_model(&store);
    for (i, title) in ["Name", "Command"].into_iter().enumerate() {
        let r = gtk::CellRendererText::new();
        r.set_property("ellipsize", pango::EllipsizeMode::End);
        let c = gtk::TreeViewColumn::new();
        c.set_title(title);
        c.set_expand(true);
        CellLayoutExt::pack_start(&c, &r, true);
        CellLayoutExt::add_attribute(&c, &r, "text", i as i32);
        tv.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_min_content_height(130);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.add(&tv);
    let selected = {
        let tv = tv.clone();
        move || tv.selection().selected().and_then(|(m, it)| m.path(&it)?.indices().first().map(|&i| i as usize))
    };
    let changed = {
        let (list, refill, tv) = (list.clone(), refill.clone(), tv.clone());
        move |select: Option<usize>| {
            actions::save(&list.borrow());
            refill();
            if let Some(i) = select {
                tv.selection().select_path(&gtk::TreePath::from_indicesv(&[i as i32]));
            }
        }
    };
    let changed = Rc::new(changed);
    let add = gtk::Button::with_mnemonic("_Add…");
    let edit = gtk::Button::with_mnemonic("_Edit…");
    let remove = gtk::Button::with_mnemonic("_Remove");
    let up = gtk::Button::from_icon_name(Some("go-up-symbolic"), gtk::IconSize::Button);
    up.set_tooltip_text(Some("Move up"));
    let down = gtk::Button::from_icon_name(Some("go-down-symbolic"), gtk::IconSize::Button);
    down.set_tooltip_text(Some("Move down"));
    {
        let (list, changed, parent) = (list.clone(), changed.clone(), parent.clone());
        add.connect_clicked(move |_| {
            let blank = actions::CustomAction { id: actions::new_id(), files: true, folders: true, ..Default::default() };
            if let Some(a) = edit_custom_action(&parent, "Add Custom Action", &blank) {
                list.borrow_mut().push(a);
                let n = list.borrow().len();
                changed(Some(n - 1));
            }
        });
    }
    let do_edit = {
        let (list, changed, parent, selected) = (list.clone(), changed.clone(), parent.clone(), selected.clone());
        move || {
            let Some(i) = selected() else { return };
            let cur = list.borrow()[i].clone();
            if let Some(a) = edit_custom_action(&parent, "Edit Custom Action", &cur) {
                list.borrow_mut()[i] = a;
                changed(Some(i));
            }
        }
    };
    let do_edit = Rc::new(do_edit);
    {
        let do_edit = do_edit.clone();
        edit.connect_clicked(move |_| do_edit());
    }
    tv.connect_row_activated(move |_, _, _| do_edit());
    {
        let (list, changed, selected) = (list.clone(), changed.clone(), selected.clone());
        remove.connect_clicked(move |_| {
            if let Some(i) = selected() {
                list.borrow_mut().remove(i);
                changed(None);
            }
        });
    }
    for (btn, delta) in [(&up, -1i32), (&down, 1)] {
        let (list, changed, selected) = (list.clone(), changed.clone(), selected.clone());
        btn.connect_clicked(move |_| {
            let Some(i) = selected() else { return };
            let j = i as i32 + delta;
            if j < 0 || j as usize >= list.borrow().len() {
                return;
            }
            list.borrow_mut().swap(i, j as usize);
            changed(Some(j as usize));
        });
    }
    // Buttons that need a selected action.
    let sync = {
        let (edit, remove, up, down, list) = (edit.clone(), remove.clone(), up.clone(), down.clone(), list.clone());
        move |sel: Option<usize>| {
            let n = list.borrow().len();
            edit.set_sensitive(sel.is_some());
            remove.set_sensitive(sel.is_some());
            up.set_sensitive(sel.is_some_and(|i| i > 0));
            down.set_sensitive(sel.is_some_and(|i| i + 1 < n));
        }
    };
    sync(None);
    tv.selection().connect_changed(move |_| sync(selected()));
    let hint = hint_label("Commands run in the current folder. %f selected file, %F all selected files, %n / %N their names, %d current folder.");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.pack_start(&add, false, false, 0);
    buttons.pack_start(&edit, false, false, 0);
    buttons.pack_start(&remove, false, false, 0);
    buttons.pack_end(&down, false, false, 0);
    buttons.pack_end(&up, false, false, 0);
    // The shell commands run with, as `<shell> -c <command>`.
    let shell = gtk::Entry::new();
    shell.set_text(&config::get().action_shell);
    shell.set_placeholder_text(Some("sh"));
    shell.set_width_chars(16);
    shell.set_tooltip_text(Some("A shell that accepts -c, such as sh, bash, zsh or fish, or its full path. Empty means sh."));
    let check_shell = |e: &gtk::Entry| {
        if actions::shell_found(&actions::shell()) {
            e.style_context().remove_class("error");
        } else {
            e.style_context().add_class("error");
        }
    };
    check_shell(&shell);
    shell.connect_changed(move |e| {
        let v = e.text().trim().to_string();
        config::update(|s| s.action_shell = v);
        check_shell(e);
    });
    let shell_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let shell_label = gtk::Label::with_mnemonic("Run commands _with:");
    shell_label.set_mnemonic_widget(Some(&shell));
    shell_row.add(&shell_label);
    shell_row.add(&shell);
    let bx = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bx.add(&sw);
    bx.add(&buttons);
    bx.add(&shell_row);
    bx.add(&hint);
    bx
}

/// Dialog to create or change a custom action; None if cancelled.
fn edit_custom_action(parent: &gtk::Window, title: &str, cur: &actions::CustomAction) -> Option<actions::CustomAction> {
    let d = gtk::Dialog::with_buttons(
        Some(title),
        Some(parent),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        &[("_Cancel", gtk::ResponseType::Cancel), ("_Save", gtk::ResponseType::Ok)],
    );
    d.set_default_response(gtk::ResponseType::Ok);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(12);
    grid.set_border_width(12);
    let label = |t: &str| {
        let l = gtk::Label::with_mnemonic(t);
        l.set_xalign(0.0);
        l
    };
    let name = gtk::Entry::new();
    name.set_text(&cur.name);
    name.set_activates_default(true);
    name.set_hexpand(true);
    let command = gtk::Entry::new();
    command.set_text(&cur.command);
    command.set_activates_default(true);
    command.set_width_chars(40);
    command.set_placeholder_text(Some("e.g. gimp %F"));
    let help = gtk::Label::new(Some("%f first selected file, %F all selected files, %n / %N their names, %d current folder, %% a % sign. Paths are quoted for you."));
    help.set_line_wrap(true);
    help.set_max_width_chars(50);
    help.set_xalign(0.0);
    help.style_context().add_class("dim-label");
    let files = gtk::CheckButton::with_mnemonic("_Files");
    files.set_active(cur.files);
    let folders = gtk::CheckButton::with_mnemonic("F_olders");
    folders.set_active(cur.folders);
    let background = gtk::CheckButton::with_mnemonic("Empty space in a _folder");
    background.set_active(cur.background);
    let show = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    show.add(&files);
    show.add(&folders);
    show.add(&background);
    // Which files: by type, by name pattern, or both (either matches).
    let chosen: Vec<&str> = cur.types.split(';').map(str::trim).collect();
    let types = gtk::FlowBox::new();
    types.set_selection_mode(gtk::SelectionMode::None);
    types.set_max_children_per_line(3);
    types.set_column_spacing(12);
    let type_checks: Vec<(&'static str, gtk::CheckButton)> = actions::FILE_TYPES
        .iter()
        .map(|(id, label)| {
            let c = gtk::CheckButton::with_label(label);
            c.set_active(chosen.contains(id));
            types.add(&c);
            (*id, c)
        })
        .collect();
    let patterns = gtk::Entry::new();
    patterns.set_text(&cur.patterns);
    patterns.set_activates_default(true);
    patterns.set_placeholder_text(Some("e.g. *.srt; *.nfo"));
    let which = gtk::Label::new(Some("A file needs one of the checked types or a name matching a pattern; with neither set, the action is offered for any file. Folders only go by the patterns."));
    which.set_line_wrap(true);
    which.set_max_width_chars(50);
    which.set_xalign(0.0);
    which.style_context().add_class("dim-label");
    // File types only concern files.
    types.set_sensitive(cur.files);
    {
        let types = types.clone();
        files.connect_toggled(move |c| types.set_sensitive(c.is_active()));
    }
    let rows: [(&str, &gtk::Widget); 7] = [
        ("_Name:", name.upcast_ref()),
        ("_Command:", command.upcast_ref()),
        ("", help.upcast_ref()),
        ("Show for:", show.upcast_ref()),
        ("File types:", types.upcast_ref()),
        ("Name _patterns:", patterns.upcast_ref()),
        ("", which.upcast_ref()),
    ];
    for (i, (t, w)) in rows.iter().enumerate() {
        let l = label(t);
        l.set_mnemonic_widget(Some(*w));
        grid.attach(&l, 0, i as i32, 1, 1);
        grid.attach(*w, 1, i as i32, 1, 1);
    }
    d.content_area().add(&grid);
    // Saving needs a name, a command and somewhere to show up.
    let validate = {
        let (d, name, command, files, folders, background) = (d.clone(), name.clone(), command.clone(), files.clone(), folders.clone(), background.clone());
        move || {
            let ok = !name.text().trim().is_empty()
                && !command.text().trim().is_empty()
                && (files.is_active() || folders.is_active() || background.is_active());
            d.set_response_sensitive(gtk::ResponseType::Ok, ok);
        }
    };
    validate();
    for e in [&name, &command] {
        let v = validate.clone();
        e.connect_changed(move |_| v());
    }
    for c in [&files, &folders, &background] {
        let v = validate.clone();
        c.connect_toggled(move |_| v());
    }
    d.show_all();
    let ok = d.run() == gtk::ResponseType::Ok;
    let result = actions::CustomAction {
        id: if cur.id.is_empty() { actions::new_id() } else { cur.id.clone() },
        name: name.text().trim().to_string(),
        command: command.text().trim().to_string(),
        files: files.is_active(),
        folders: folders.is_active(),
        background: background.is_active(),
        patterns: patterns.text().trim().to_string(),
        types: type_checks.iter().filter(|(_, c)| c.is_active()).map(|(id, _)| *id).collect::<Vec<_>>().join(";"),
    };
    d.close();
    ok.then_some(result)
}

thread_local! {
    static SIDEBAR_CSS: gtk::CssProvider = {
        let css = gtk::CssProvider::new();
        if let Some(screen) = gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(&screen, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        css
    };
}

/// Side pane row height from the settings. The rows are separate widgets, so
/// this needs a screen-wide provider; the class keeps it off file choosers.
fn apply_sidebar_css() {
    let h = config::with(|s| s.sidebar_row_height);
    // The eject buttons are no taller than their icon, so device rows can be
    // as low as the others.
    let mut data = String::from("list.arkha-sidebar button.sidebar-button { min-height: 0; min-width: 0; padding: 2px; margin: 0; }");
    if h > 0 {
        data.push_str(&format!(" list.arkha-sidebar row.sidebar-row {{ min-height: {h}px; }}"));
    }
    SIDEBAR_CSS.with(|css| {
        let _ = css.load_from_data(data.as_bytes());
    });
}

#[derive(Clone, Copy)]
enum OpenMode {
    Normal,
    Tab,
    Window,
}

fn location_text(f: &gio::File) -> String {
    match f.path() {
        Some(p) => p.to_string_lossy().to_string(),
        None => f.uri().to_string(),
    }
}

/// What completing `base` adds, with the folders `dirs` there: the rest of
/// the only name starting with it (with a "/"), or of the part all the
/// names starting with it have in common. Case matters; hidden folders
/// only once a dot is typed.
fn completion_suffix(base: &str, dirs: &[String]) -> Option<String> {
    let mut common: Option<String> = None;
    for n in dirs.iter().filter(|n| n.starts_with(base) && (base.starts_with('.') || !n.starts_with('.'))) {
        let full = format!("{n}/");
        common = Some(match common {
            None => full,
            Some(c) => c.chars().zip(full.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect(),
        });
    }
    common.and_then(|c| c.get(base.len()..).filter(|rest| !rest.is_empty()).map(str::to_string))
}

fn parse_location(text: &str, base: Option<gio::File>) -> gio::File {
    // Up to a NUL, as a text field would have it: paths can't hold one.
    let text = text.split('\0').next().unwrap_or_default();
    // "~" and "~/…" are in the home folder; "~notes" is a name like any other.
    if text == "~" || text.starts_with("~/") {
        return gio::File::for_path(format!("{}{}", glib::home_dir().to_string_lossy(), &text[1..]));
    }
    if text.starts_with('/') {
        return gio::File::for_path(text);
    }
    if text.contains("://") || text.starts_with("trash:") {
        return gio::File::for_uri(text);
    }
    match base {
        Some(b) => b.resolve_relative_path(text),
        None => gio::File::for_path(text),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_completion() {
        let dirs: Vec<String> = [".ssh", ".cache", ".cargo", "Desktop", "Documents", "Downloads", "Projects", "Projects-old", "Ünï"]
            .map(String::from)
            .to_vec();
        let c = |base: &str| completion_suffix(base, &dirs);
        assert_eq!(c(".ss").as_deref(), Some("h/"));
        assert_eq!(c(".car").as_deref(), Some("go/"));
        assert_eq!(c("Pro").as_deref(), Some("jects"));
        assert_eq!(c("Projects-").as_deref(), Some("old/"));
        assert_eq!(c("Ü").as_deref(), Some("nï/"));
        assert_eq!(c("Projects/"), None);
        // Several names, nothing more in common.
        assert_eq!(c("D"), None);
        assert_eq!(c("Do"), None);
        assert_eq!(c(".ca"), None);
        // Case matters, as in Caja.
        assert_eq!(c("doc"), None);
        assert_eq!(c("x"), None);
        // Hidden folders only once a dot is typed.
        let one: Vec<String> = vec![".hidden".into(), "shown".into()];
        assert_eq!(completion_suffix("s", &one).as_deref(), Some("hown/"));
        assert_eq!(completion_suffix(".", &one).as_deref(), Some("hidden/"));
        // Already complete.
        assert_eq!(c("Downloads/"), None);
    }

    #[test]
    fn typed_locations() {
        let home = glib::home_dir();
        let base = || Some(gio::File::for_path("/srv/data"));
        assert_eq!(parse_location("~", None).path(), Some(home.clone()));
        assert_eq!(parse_location("~/Music", None).path(), Some(home.join("Music")));
        assert_eq!(parse_location("/etc", base()).path(), Some("/etc".into()));
        assert_eq!(parse_location("sub/dir", base()).path(), Some("/srv/data/sub/dir".into()));
        assert_eq!(parse_location("../x", base()).path(), Some("/srv/x".into()));
        // A name starting with "~" in the current folder.
        assert_eq!(parse_location("~notes", base()).path(), Some("/srv/data/~notes".into()));
        assert_eq!(parse_location("trash:///", None).uri(), "trash:///");
        assert_eq!(parse_location("sftp://host/srv", None).uri(), "sftp://host/srv");
        // Shown as a path for local files, else as the URI.
        assert_eq!(location_text(&gio::File::for_path("/a b")), "/a b");
        assert_eq!(location_text(&gio::File::for_uri("trash:///")), "trash:///");
    }

    #[test]
    fn random_typed_locations() {
        crate::testutil::cases(2000, |r| {
            let text = r.text(20);
            let base = r.chance(2).then(|| gio::File::for_path("/srv/data"));
            let _ = parse_location(&text, base);
            let dirs: Vec<String> = (0..r.below(6)).map(|_| r.text(8)).collect();
            // What's added leads to one of the folders.
            if let Some(more) = completion_suffix(&text, &dirs) {
                assert!(dirs.iter().any(|d| format!("{d}/").starts_with(&format!("{text}{more}"))), "{text:?} + {more:?} in {dirs:?}");
            }
        });
    }
}
