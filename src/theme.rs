//! Voyager's look: Windows Explorer's. Flat toolbar buttons with a light
//! hover, the address as a bordered field with chevrons between the
//! folders, a navigation pane with rounded, light-blue selected rows and
//! Windows-like tabs and selections. Only shapes and tints: colors come
//! from the GTK theme, so dark themes stay dark.

use gtk::prelude::*;

const CSS: &str = r#"
/* ---- bars */
.voyager-locbar, .voyager-commandbar { background-image: none; background-color: @theme_bg_color; border: none; box-shadow: none; }
.voyager-commandbar { border-bottom: 1px solid @borders; }
.voyager-locbar { padding-top: 2px; padding-bottom: 2px; }

.voyager-locbar button, .voyager-commandbar button, .voyager-commandbar menubutton > button {
    background-image: none; background-color: transparent; border: 1px solid transparent;
    box-shadow: none; border-radius: 4px; padding: 3px 8px; min-height: 22px; min-width: 0;
}
.voyager-locbar button:hover, .voyager-commandbar button:hover, .voyager-commandbar menubutton > button:hover {
    background-color: alpha(@theme_fg_color, 0.08);
}
.voyager-locbar button:active, .voyager-locbar button:checked, .voyager-commandbar button:active, .voyager-commandbar button:checked {
    background-color: alpha(@theme_selected_bg_color, 0.22);
}
.voyager-locbar button:disabled, .voyager-commandbar button:disabled { opacity: 0.4; }
.voyager-commandbar separator { min-width: 1px; margin: 4px 2px; background-color: @borders; }

/* ---- address bar */
scrolledwindow.voyager-address {
    background-color: @theme_base_color; border: 1px solid @borders; border-radius: 4px;
    padding: 0 2px; min-height: 28px;
}
scrolledwindow.voyager-address:hover { border-color: alpha(@theme_fg_color, 0.45); }
scrolledwindow.voyager-address button { padding: 1px 6px; min-height: 20px; margin: 2px 0; }
scrolledwindow.voyager-address button:checked { background-color: transparent; }
scrolledwindow.voyager-address button:checked:hover { background-color: alpha(@theme_fg_color, 0.08); }
.voyager-address .voyager-chevron { margin: 0 -2px; }
.voyager-locbar entry { border-radius: 4px; min-height: 26px; }

/* ---- navigation pane */
list.voyager-sidebar { background-color: @theme_bg_color; }
list.voyager-sidebar row, list.voyager-sidebar row.sidebar-row {
    background-color: transparent; background-image: none; border: none; box-shadow: none; outline: none;
}
list.voyager-sidebar row.sidebar-row {
    border-radius: 4px; margin: 1px 6px; padding: 3px 4px;
}
list.voyager-sidebar row.sidebar-row:hover { background-color: alpha(@theme_fg_color, 0.07); }
list.voyager-sidebar row.sidebar-row:selected, list.voyager-sidebar row.sidebar-row:selected:focus {
    background-color: alpha(@theme_selected_bg_color, 0.24); color: @theme_fg_color;
    box-shadow: inset 3px 0 0 0 @theme_selected_bg_color;
}
list.voyager-sidebar row.sidebar-row:selected label, list.voyager-sidebar row.sidebar-row:selected image { color: @theme_fg_color; }
list.voyager-sidebar row { padding: 0; }
list.voyager-sidebar row:not(.sidebar-row) { margin: 0 6px; }
list.voyager-sidebar label.sidebar-heading { font-weight: normal; }

/* ---- file views */
iconview.view:selected, iconview.view:selected:focus {
    background-color: alpha(@theme_selected_bg_color, 0.24); color: @theme_fg_color;
    border: 1px solid alpha(@theme_selected_bg_color, 0.6); border-radius: 4px;
}
treeview.view:selected, treeview.view:selected:focus {
    background-color: alpha(@theme_selected_bg_color, 0.24); color: @theme_fg_color;
}
treeview.view:hover, iconview.view:hover { background-color: alpha(@theme_fg_color, 0.06); }
treeview.view header button {
    background-image: none; background-color: @theme_base_color; border: none; box-shadow: none;
    border-right: 1px solid @borders; border-bottom: 1px solid @borders; border-radius: 0; padding: 4px 8px;
}
treeview.view header button:hover { background-color: alpha(@theme_selected_bg_color, 0.14); }

/* ---- This PC */
scrolledwindow.thispc { background-color: @theme_base_color; }
label.thispc-heading { font-size: 1.05em; font-weight: 600; }
expander.thispc-section title { padding: 2px 0; }
flowbox.thispc flowboxchild { border-radius: 4px; padding: 6px 8px; border: 1px solid transparent; }
flowbox.thispc flowboxchild:hover { background-color: alpha(@theme_selected_bg_color, 0.12); border-color: alpha(@theme_selected_bg_color, 0.25); }
flowbox.thispc flowboxchild:selected, flowbox.thispc flowboxchild:selected:focus {
    background-color: alpha(@theme_selected_bg_color, 0.24); border-color: alpha(@theme_selected_bg_color, 0.6); color: @theme_fg_color;
}
progressbar.thispc-bar trough { min-height: 14px; border-radius: 0; background-color: alpha(@theme_fg_color, 0.1); border: 1px solid alpha(@theme_fg_color, 0.3); }
progressbar.thispc-bar progress { min-height: 12px; border-radius: 0; border: none; background-image: none; background-color: #26a0da; }
progressbar.thispc-bar.critical progress { background-color: #da2a1f; }

/* ---- preferences search */
.voyager-search-hit { background-color: alpha(#ffc800, 0.35); border-radius: 3px; }

/* ---- tabs */
notebook > header { background-color: @theme_bg_color; border-width: 0 0 1px 0; }
notebook > header tabs tab {
    border-radius: 6px 6px 0 0; padding: 4px 8px; background-image: none; background-color: transparent;
    border-width: 0; box-shadow: none; min-height: 24px;
}
notebook > header tabs tab:hover { background-color: alpha(@theme_fg_color, 0.07); }
notebook > header tabs tab:checked { background-color: @theme_base_color; box-shadow: inset 0 2px 0 0 @theme_selected_bg_color; }

/* ---- status bar */
statusbar, .voyager-statusbar { border-top: 1px solid @borders; background-color: @theme_bg_color; }
.voyager-statusbar label { font-size: 0.92em; }

/* ---- menus */
menubar { background-color: @theme_bg_color; }
menubar > menuitem { padding: 3px 8px; }
menuitem:hover { background-color: alpha(@theme_selected_bg_color, 0.24); color: @theme_fg_color; }
"#;

/// Puts the look on every window (once, at startup).
pub fn apply() {
    let provider = gtk::CssProvider::new();
    if provider.load_from_data(CSS.as_bytes()).is_err() {
        return;
    }
    if let Some(screen) = gdk::Screen::default() {
        // Below the application level of the settings (backgrounds, row
        // height), so what the user chose still wins.
        gtk::StyleContext::add_provider_for_screen(&screen, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION - 1);
    }
}

thread_local! {
    static SYSTEM_FONT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Applies the font chosen in the preferences ("" for the system's, which
/// GTK takes from the desktop).
pub fn apply_font() {
    let Some(settings) = gtk::Settings::default() else { return };
    SYSTEM_FONT.with(|f| {
        let mut f = f.borrow_mut();
        if f.is_none() {
            *f = settings.gtk_font_name().map(|n| n.to_string());
        }
        let chosen = crate::config::with(|s| s.font.clone());
        let name = if chosen.is_empty() { f.clone() } else { Some(chosen) };
        settings.set_gtk_font_name(name.as_deref());
    });
}
