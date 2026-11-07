//! The org.freedesktop.FileManager1 D-Bus service, as Caja has it: other
//! programs ask the file manager to show folders, show files in their
//! folder ("Show in folder" in browsers, download lists, …) or show their
//! properties.
//!
//! Arkha takes the name when it starts, if no one has it; otherwise it
//! waits its turn (a running Caja keeps it). So that programs can also start
//! Arkha for it, Preferences can put a D-Bus activation file in the user's
//! own folder, which goes before the system's (Caja's, Nemo's).

use gio::prelude::*;
use gtk::prelude::*;

const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";
const INTERFACE: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItems">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
    <method name="ShowItemProperties">
      <arg type="as" name="URIs" direction="in"/>
      <arg type="s" name="StartupId" direction="in"/>
    </method>
  </interface>
</node>"#;

/// Takes the name (or queues for it) and serves the methods.
pub fn init() {
    gio::bus_own_name(
        gio::BusType::Session,
        NAME,
        gio::BusNameOwnerFlags::NONE,
        |conn, _| {
            let Ok(node) = gio::DBusNodeInfo::for_xml(INTERFACE) else { return };
            let Some(iface) = node.lookup_interface(NAME) else { return };
            // GLib calls these on the main thread (where this registers).
            let _ = conn.register_object(
                PATH,
                &iface,
                |_, _, _, _, method, params, invocation| {
                    let Some((uris, startup_id)) = params.get::<(Vec<String>, String)>() else {
                        invocation.return_error(gio::IOErrorEnum::InvalidArgument, "Expected a list of URIs and a startup ID");
                        return;
                    };
                    match method {
                        "ShowFolders" => show_folders(&uris, &startup_id),
                        "ShowItems" => show_items(&uris, &startup_id),
                        "ShowItemProperties" => show_properties(&uris, &startup_id),
                        _ => {
                            invocation.return_error(gio::IOErrorEnum::NotSupported, "No such method");
                            return;
                        }
                    }
                    invocation.return_value(None);
                },
                |_, _, _, _, _| glib::Variant::from(()),
                |_, _, _, _, _, _| false,
            );
        },
        |_, _| {},
        |_, _| {},
    );
}

fn app() -> Option<gtk::Application> {
    gio::Application::default().and_downcast::<gtk::Application>()
}

/// Lets the window come to the front for the program that asked.
fn raise(w: &crate::window::AppWindow, startup_id: &str) {
    if !startup_id.is_empty() {
        w.window.set_startup_id(startup_id);
    }
    w.window.present();
}

fn show_folders(uris: &[String], startup_id: &str) {
    let Some(app) = app() else { return };
    let folders: Vec<gio::File> = uris.iter().map(|u| gio::File::for_uri(u)).collect();
    if folders.is_empty() {
        return;
    }
    // One window, a tab for each.
    let w = crate::window::AppWindow::new(&app, &folders);
    raise(&w, startup_id);
}

/// The items grouped by their folder, in the order given.
fn by_folder(uris: &[String]) -> Vec<(gio::File, Vec<String>)> {
    let mut groups: Vec<(gio::File, Vec<String>)> = vec![];
    for u in uris {
        let f = gio::File::for_uri(u);
        let Some(parent) = f.parent() else {
            // The root itself: just show it.
            groups.push((f, vec![]));
            continue;
        };
        match groups.iter_mut().find(|(p, _)| p.equal(&parent)) {
            Some((_, items)) => items.push(u.clone()),
            None => groups.push((parent, vec![u.clone()])),
        }
    }
    groups
}

/// A window for each folder, its items selected (as Caja does).
fn show_items(uris: &[String], startup_id: &str) {
    let Some(app) = app() else { return };
    for (folder, items) in by_folder(uris) {
        let w = crate::window::AppWindow::new(&app, &[folder]);
        if let Some(p) = w.current_pane() {
            p.select_uris_later(items);
        }
        raise(&w, startup_id);
    }
}

fn show_properties(uris: &[String], startup_id: &str) {
    let Some(app) = app() else { return };
    let files: Vec<gio::File> = uris.iter().map(|u| gio::File::for_uri(u)).collect();
    let Some(first) = files.first() else { return };
    // On a window of Arkha's: the last one used, or one at the items' folder.
    let w = match crate::window::all_windows().into_iter().last() {
        Some(w) => w,
        None => crate::window::AppWindow::new(&app, &[first.parent().unwrap_or_else(|| first.clone())]),
    };
    raise(&w, startup_id);
    crate::props::show(w.window.upcast_ref(), files);
}

/// The activation file in the user's own D-Bus services folder.
fn service_file() -> std::path::PathBuf {
    glib::user_data_dir().join("dbus-1").join("services").join(format!("{NAME}.service"))
}

/// Whether programs start Arkha (through the user's activation file).
pub fn activation_on() -> bool {
    std::fs::read_to_string(service_file()).is_ok_and(|t| t.contains("arkha"))
}

/// Writes or removes the user's activation file (only one of Arkha's).
pub fn set_activation(on: bool) -> std::io::Result<()> {
    let path = service_file();
    if on {
        let exe = std::env::current_exe()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // As a service: no window of its own, just what's asked for.
        std::fs::write(&path, format!("[D-BUS Service]\nName={NAME}\nExec={} --gapplication-service\n", exe.display()))
    } else if activation_on() {
        std::fs::remove_file(&path)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gio::prelude::*;

    #[test]
    fn items_grouped_by_folder() {
        let uris: Vec<String> = ["file:///a/x", "file:///b/y", "file:///a/z", "file:///"].iter().map(|s| s.to_string()).collect();
        let g = super::by_folder(&uris);
        let shown: Vec<(String, Vec<String>)> = g.iter().map(|(f, items)| (f.uri().to_string(), items.clone())).collect();
        assert_eq!(
            shown,
            [
                ("file:///a".to_string(), vec!["file:///a/x".to_string(), "file:///a/z".to_string()]),
                ("file:///b".to_string(), vec!["file:///b/y".to_string()]),
                ("file:///".to_string(), vec![]),
            ]
        );
    }

    #[test]
    fn interface_parses() {
        let node = gio::DBusNodeInfo::for_xml(super::INTERFACE).unwrap();
        let iface = node.lookup_interface(super::NAME).unwrap();
        assert!(iface.lookup_method("ShowItems").is_some());
    }
}
