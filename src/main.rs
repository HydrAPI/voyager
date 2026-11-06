//! Arkha — a Caja-style file manager in Rust + GTK 3.

mod actions;
mod bulkrename;
mod clipboard;
mod columns;
mod config;
mod iconlabel;
mod iconview;
mod info;
mod folders;
mod icons;
mod keys;
mod ops;
mod pane;
mod props;
mod scroll;
mod search;
mod sidebar;
#[cfg(test)]
mod testutil;
mod util;
mod thumbnailers;
mod window;

use gio::prelude::*;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(
        Some("io.github.arkha.FileManager"),
        gio::ApplicationFlags::HANDLES_OPEN,
    );
    app.connect_startup(|app| {
        let quit = gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| {
            if ops::is_busy() {
                // Let running operations finish; just close the windows, and
                // show the operations, whose button goes with them.
                ops::open_window();
                for w in window::all_windows() {
                    w.window.close();
                }
                return;
            }
            a.quit();
        });
        app.add_action(&quit);
        gtk::Window::set_default_icon_name("system-file-manager");
        scroll::init();
        util::set_iec_units(config::with(|s| s.iec_units));
        sidebar::init_automount();
    });
    app.connect_activate(|app| {
        window::AppWindow::new(app, &[]);
    });
    app.connect_open(|app, files, _| {
        let dirs: Vec<gio::File> = files
            .iter()
            .map(|f| {
                let is_dir = f
                    .query_info("standard::type", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
                    .map(|i| i.file_type() == gio::FileType::Directory)
                    .unwrap_or(true);
                if is_dir { f.clone() } else { f.parent().unwrap_or_else(|| f.clone()) }
            })
            .collect();
        window::AppWindow::new(app, &dirs);
    });
    app.run()
}
