//! The menu bar and the context menus.

use super::*;

/// Arrange Items' sort orders: the name and the first three details (the
/// usual ones), or (`more`) the others there are menu items for.
fn sort_section(more: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    if !more {
        menu.append(Some("By _Name"), Some("win.sort-by::name"));
    }
    for (i, d) in crate::details::DETAILS.iter().enumerate() {
        if let Some(label) = d.menu.filter(|_| (i >= 3) == more) {
            menu.append(Some(label), Some(&format!("win.sort-by::{}", d.id)));
        }
    }
    menu
}

impl AppWindow {
    pub(super) fn build_menubar(&self, bar: &gio::Menu) {
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
        file.append_section(None, &section(&[("Move Tab _Left", "win.tab-left"), ("Move Tab Ri_ght", "win.tab-right")]));
        file.append_section(None, &section(&[("_Close Tab", "win.close-tab"), ("_Reopen Closed Tab", "win.reopen-tab"), ("Close _Window", "win.close-window"), ("Close _All Windows", "win.close-all-windows"), ("_Quit", "app.quit")]));
        bar.append_submenu(Some("_File"), &file);

        let edit = gio::Menu::new();
        self.undo_menu.append(Some("_Undo"), Some("win.undo"));
        self.undo_menu.append(Some("_Redo"), Some("win.redo"));
        edit.append_section(None, &self.undo_menu);
        edit.append_section(None, &section(&[("Cu_t", "win.cut"), ("_Copy", "win.copy"), ("_Paste", "win.paste"), ("Paste _Into Folder", "win.paste-into")]));
        edit.append_section(None, &section(&[("Select _All", "win.select-all"), ("Select I_tems Matching…", "win.select-pattern"), ("_Invert Selection", "win.invert-selection")]));
        edit.append_section(None, &section(&[("Dup_licate", "win.duplicate"), ("Ma_ke Link", "win.make-link"), ("_Rename…", "win.rename"), ("Copy T_o…", "win.copy-to"), ("_Move To…", "win.move-to")]));
        edit.append_section(None, &section(&[("Copy to Other _Pane", "win.copy-to-pane"), ("Move to Other Pa_ne", "win.move-to-pane")]));
        edit.append_section(None, &section(&[("Mo_ve to Trash", "win.trash"), ("_Delete", "win.delete"), ("_Restore", "win.restore")]));
        edit.append_section(None, &section(&[("_Backgrounds and Emblems…", "win.backgrounds"), ("Pr_eferences", "win.preferences")]));
        bar.append_submenu(Some("_Edit"), &edit);

        let view = gio::Menu::new();
        view.append_section(None, &section(&[("_Reload", "win.reload"), ("Sto_p", "win.stop")]));
        view.append_section(None, &section(&[("_Icons", "win.view-mode::icon"), ("_List", "win.view-mode::list"), ("_Compact", "win.view-mode::compact"), ("C_olumns", "win.view-mode::columns")]));
        view.append_section(None, &section(&[("Show _Hidden Files", "win.show-hidden"), ("_Side Pane", "win.show-sidebar"), ("E_xtra Pane", "win.split-view"), ("St_atusbar", "win.show-statusbar"), ("_Info Panel", "win.info-panel")]));
        view.append_section(None, &section(&[("Swap Pa_nes", "win.swap-panes"), ("S_witch to Other Pane", "win.switch-pane"), ("Same Location as O_ther Pane", "win.same-location")]));
        view.append_section(None, &section(&[("_Main Toolbar", "win.show-toolbar"), ("Location _Bar", "win.show-locbar")]));
        let sort = gio::Menu::new();
        sort.append_section(None, &sort_section(false));
        sort.append_section(None, &sort_section(true));
        sort.append_section(None, &section(&[("_Reversed Order", "win.sort-desc"), ("_Folders Before Files", "win.folders-first")]));
        view.append_submenu(Some("Arrange _Items"), &sort);
        view.append_section(None, &section(&[("_Visible Columns…", "win.visible-columns")]));
        view.append_section(None, &section(&[("Reset View to _Defaults", "win.reset-view")]));
        view.append_section(None, &section(&[("Zoom _In", "win.zoom-in"), ("Zoom _Out", "win.zoom-out"), ("Normal Si_ze", "win.zoom-normal")]));
        view.append_section(None, &section(&[("Loc_k Toolbar", "win.lock-bar")]));
        bar.append_submenu(Some("_View"), &view);

        let go = gio::Menu::new();
        go.append_section(None, &section(&[("_Back", "win.back"), ("_Forward", "win.forward"), ("_Open Parent", "win.up")]));
        go.append_section(None, &section(&[("_Home Folder", "win.home"), ("_Desktop", "win.go-desktop"), ("File _System", "win.go-root"), ("_Trash", "win.go-trash")]));
        go.append_section(None, &section(&[("_Location…", "win.location"), ("_Filter…", "win.filter")]));
        go.append_section(None, &section(&[("Clea_r History", "win.clear-history")]));
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

    pub(super) fn refresh_bookmarks(&self) {
        self.bookmarks_menu.remove_all();
        for (uri, label) in util::read_bookmarks() {
            let item = gio::MenuItem::new(Some(&label.replace('_', "__")), None);
            item.set_action_and_target_value(Some("win.go-uri"), Some(&uri.to_variant()));
            self.bookmarks_menu.append_item(&item);
        }
        self.sidebar.refresh();
    }

    pub(super) fn refresh_templates(&self) {
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

    pub(super) fn open_with_menu(&self, sel: &[crate::pane::RowInfo]) -> gio::Menu {
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

    pub(super) fn context_menu(self: &Rc<Self>, pane: &Rc<Pane>, event: Option<gdk::Event>, on_item: bool) {
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
                    arrange.append_section(None, &sort_section(false));
                    arrange.append_section(None, &sort_section(true));
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
    pub(super) fn run_custom_action(self: &Rc<Self>, target: &str) {
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
}
