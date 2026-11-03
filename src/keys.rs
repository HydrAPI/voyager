//! Keyboard shortcuts: every action that can have one, its defaults, and the
//! user's changes (`Settings::keybindings`), applied as GTK accelerators.

use gtk::prelude::*;

use crate::config;

pub struct Binding {
    /// Detailed action name, e.g. "win.copy" or "win.view-mode::list".
    pub action: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub defaults: &'static [&'static str],
}

const fn b(group: &'static str, action: &'static str, label: &'static str, defaults: &'static [&'static str]) -> Binding {
    Binding { action, label, group, defaults }
}

pub const BINDINGS: &[Binding] = &[
    b("Windows and Tabs", "win.new-tab", "New tab", &["<Ctrl>t"]),
    b("Windows and Tabs", "win.new-window", "New window", &["<Ctrl>n"]),
    b("Windows and Tabs", "win.close-tab", "Close tab", &["<Ctrl>w"]),
    b("Windows and Tabs", "win.reopen-tab", "Reopen closed tab", &["<Ctrl><Shift>t"]),
    b("Windows and Tabs", "win.close-window", "Close window", &["<Ctrl><Shift>w"]),
    b("Windows and Tabs", "app.quit", "Quit", &["<Ctrl>q"]),
    b("Windows and Tabs", "win.next-tab", "Next tab", &["<Ctrl>Page_Down", "<Ctrl>Tab"]),
    b("Windows and Tabs", "win.prev-tab", "Previous tab", &["<Ctrl>Page_Up", "<Ctrl><Shift>Tab"]),
    b("Windows and Tabs", "win.tab-1", "Go to tab 1", &["<Alt>1"]),
    b("Windows and Tabs", "win.tab-2", "Go to tab 2", &["<Alt>2"]),
    b("Windows and Tabs", "win.tab-3", "Go to tab 3", &["<Alt>3"]),
    b("Windows and Tabs", "win.tab-4", "Go to tab 4", &["<Alt>4"]),
    b("Windows and Tabs", "win.tab-5", "Go to tab 5", &["<Alt>5"]),
    b("Windows and Tabs", "win.tab-6", "Go to tab 6", &["<Alt>6"]),
    b("Windows and Tabs", "win.tab-7", "Go to tab 7", &["<Alt>7"]),
    b("Windows and Tabs", "win.tab-8", "Go to tab 8", &["<Alt>8"]),
    b("Windows and Tabs", "win.tab-9", "Go to last tab", &["<Alt>9"]),
    b("Navigation", "win.back", "Back", &["<Alt>Left", "XF86Back"]),
    b("Navigation", "win.forward", "Forward", &["<Alt>Right", "XF86Forward"]),
    b("Navigation", "win.up", "Open parent folder", &["<Alt>Up", "BackSpace"]),
    b("Navigation", "win.home", "Home folder", &["<Alt>Home"]),
    b("Navigation", "win.go-root", "File system", &[]),
    b("Navigation", "win.go-desktop", "Desktop", &[]),
    b("Navigation", "win.go-trash", "Trash", &[]),
    b("Navigation", "win.location", "Type a location", &["<Ctrl>l"]),
    b("Navigation", "win.filter", "Filter the folder", &["<Ctrl>f"]),
    b("Navigation", "win.reload", "Reload", &["F5", "<Ctrl>r"]),
    b("Navigation", "win.escape", "Cancel (close filter or location bar)", &["Escape"]),
    b("Opening", "win.open", "Open", &["<Ctrl>o", "<Alt>Down"]),
    b("Opening", "win.open-new-tab", "Open in new tab", &["<Ctrl>Return"]),
    b("Opening", "win.open-new-window", "Open in new window", &[]),
    b("Opening", "win.open-with-other", "Open with other application", &[]),
    b("Opening", "win.terminal", "Open in terminal", &["F4"]),
    b("Files", "win.new-folder", "New folder", &["<Ctrl><Shift>n"]),
    b("Files", "win.new-file", "New empty document", &[]),
    b("Files", "win.cut", "Cut", &["<Ctrl>x"]),
    b("Files", "win.copy", "Copy", &["<Ctrl>c"]),
    b("Files", "win.paste", "Paste", &["<Ctrl>v"]),
    b("Files", "win.paste-into", "Paste into folder", &[]),
    b("Files", "win.rename", "Rename", &["F2"]),
    b("Files", "win.rename-all", "Rename All… / Advanced Rename…", &[]),
    b("Files", "win.duplicate", "Duplicate", &[]),
    b("Files", "win.make-link", "Make link", &["<Ctrl>m"]),
    b("Files", "win.copy-to", "Copy to…", &[]),
    b("Files", "win.move-to", "Move to…", &[]),
    b("Files", "win.copy-to-pane", "Copy to other pane", &[]),
    b("Files", "win.move-to-pane", "Move to other pane", &[]),
    b("Files", "win.trash", "Move to Trash", &["Delete", "KP_Delete"]),
    b("Files", "win.delete", "Delete permanently", &["<Shift>Delete", "<Shift>KP_Delete"]),
    b("Files", "win.restore", "Restore from Trash", &[]),
    b("Files", "win.empty-trash", "Empty Trash", &[]),
    b("Files", "win.extract", "Extract here", &[]),
    b("Files", "win.compress", "Compress…", &[]),
    b("Files", "win.properties", "Properties", &["<Alt>Return", "<Ctrl>i"]),
    b("Selection", "win.select-all", "Select all", &["<Ctrl>a"]),
    b("Selection", "win.invert-selection", "Invert selection", &["<Ctrl><Shift>i"]),
    b("Selection", "win.select-pattern", "Select by pattern", &["<Ctrl>s"]),
    b("View", "win.view-mode::icon", "Icon view", &["<Ctrl>1"]),
    b("View", "win.view-mode::list", "List view", &["<Ctrl>2"]),
    b("View", "win.view-mode::compact", "Compact view", &["<Ctrl>3"]),
    b("View", "win.view-mode::columns", "Columns view", &["<Ctrl>4"]),
    b("View", "win.show-hidden", "Show hidden files", &["<Ctrl>h"]),
    b("View", "win.show-sidebar", "Side pane", &["F9"]),
    b("View", "win.split-view", "Extra pane (split view)", &["F3"]),
    b("View", "win.focus-follows-mouse", "Focus the pane under the mouse pointer", &["<Shift>F3"]),
    b("View", "win.info-panel", "Info panel (for this folder)", &["F11"]),
    b("View", "win.show-statusbar", "Statusbar", &[]),
    b("View", "win.zoom-in", "Zoom in", &["<Ctrl>plus", "<Ctrl>equal", "<Ctrl>KP_Add"]),
    b("View", "win.zoom-out", "Zoom out", &["<Ctrl>minus", "<Ctrl>KP_Subtract"]),
    b("View", "win.zoom-normal", "Normal size", &["<Ctrl>0", "<Ctrl>KP_0"]),
    b("View", "win.sort-by::name", "Sort by name", &[]),
    b("View", "win.sort-by::size", "Sort by size", &[]),
    b("View", "win.sort-by::type", "Sort by type", &[]),
    b("View", "win.sort-by::mtime", "Sort by modification date", &[]),
    b("View", "win.sort-desc", "Reversed order", &[]),
    b("View", "win.folders-first", "Folders before files", &[]),
    b("Bookmarks and Help", "win.add-bookmark", "Add bookmark", &["<Ctrl>d"]),
    b("Bookmarks and Help", "win.edit-bookmarks", "Edit bookmarks", &["<Ctrl>b"]),
    b("Bookmarks and Help", "win.preferences", "Preferences", &[]),
    b("Bookmarks and Help", "win.shortcuts", "Keyboard shortcuts", &["<Ctrl>question", "<Ctrl>F1"]),
    b("Bookmarks and Help", "win.about", "About", &[]),
];

/// The shortcuts `action` has now: the user's, or the defaults.
pub fn current(action: &str) -> Vec<String> {
    current_in(&config::get(), action)
}

fn current_in(s: &config::Settings, action: &str) -> Vec<String> {
    match s.keybindings.iter().find(|(a, _)| a == action) {
        Some((_, accels)) => accels.clone(),
        None => BINDINGS
            .iter()
            .find(|b| b.action == action)
            .map(|b| b.defaults.iter().map(|d| d.to_string()).collect())
            .unwrap_or_default(),
    }
}

pub fn is_default(action: &str) -> bool {
    !config::get().keybindings.iter().any(|(a, _)| a == action)
}

/// Sets `action`'s shortcuts (None = back to the defaults) and applies them.
pub fn set(app: &gtk::Application, action: &str, accels: Option<Vec<String>>) {
    config::update(|s| {
        s.keybindings.retain(|(a, _)| a != action);
        if let Some(accels) = accels {
            let default = BINDINGS.iter().find(|b| b.action == action).map(|b| b.defaults.to_vec()).unwrap_or_default();
            if accels != default {
                s.keybindings.push((action.to_string(), accels));
            }
        }
    });
    apply(app);
}

pub fn reset_all(app: &gtk::Application) {
    config::update(|s| s.keybindings.clear());
    apply(app);
}

pub fn apply(app: &gtk::Application) {
    let s = config::get();
    for b in BINDINGS {
        let accels = current_in(&s, b.action);
        app.set_accels_for_action(b.action, &accels.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

/// The other action that already uses `accel`, if any.
pub fn used_by(accel: &str, except: &str) -> Option<&'static Binding> {
    let s = config::get();
    let want = gtk::accelerator_parse(accel);
    BINDINGS
        .iter()
        .filter(|b| b.action != except)
        .find(|b| current_in(&s, b.action).iter().any(|a| gtk::accelerator_parse(a) == want))
}

/// "Ctrl+Shift+N, F5" for showing a list of shortcuts.
pub fn labels(accels: &[String]) -> String {
    accels
        .iter()
        .map(|a| {
            let (key, mods) = gtk::accelerator_parse(a);
            gtk::accelerator_get_label(key, mods).map(|l| l.to_string()).unwrap_or_else(|| a.clone())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::BINDINGS;

    #[test]
    fn catalogue_is_consistent() {
        let mut actions: Vec<&str> = BINDINGS.iter().map(|b| b.action).collect();
        actions.sort_unstable();
        let n = actions.len();
        actions.dedup();
        assert_eq!(actions.len(), n, "an action is listed twice");
        let mut accels: Vec<&str> = BINDINGS.iter().flat_map(|b| b.defaults.iter().copied()).collect();
        accels.sort_unstable();
        let n = accels.len();
        accels.dedup();
        assert_eq!(accels.len(), n, "two actions share a default shortcut");
    }
}
