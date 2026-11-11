//! GVfs metadata (emblems, notes, custom icons, as Caja keeps them),
//! read and written by a thread of its own, one request after another.
//!
//! GVfs's metadata service can be slow or stuck (busy, or its store moved
//! away), and then reading or writing metadata waits up to 25 s. So it's
//! never done on the main thread, nor on GIO's shared threads (with a few
//! of those waiting, folder loading would wait too): the views show items
//! without it and add it when it comes (pane.rs), Properties fills its
//! Emblems and Notes pages when it comes (props.rs).

use gio::prelude::*;
use std::cell::RefCell;

/// The metadata the views and Properties use.
pub const ATTRS: &str = "metadata::emblems,metadata::annotation,metadata::custom-icon,metadata::custom-icon-name,metadata::folder-background-color,metadata::folder-background-image";

/// An item's metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    /// Emblem keywords ("important" for emblem-important).
    pub emblems: Vec<String>,
    pub note: String,
    /// A picture's URI.
    pub icon: String,
    /// A theme icon's name.
    pub icon_name: String,
    /// A folder's background (Caja's keys): "#rrggbb", a picture's URI.
    pub bg_color: String,
    pub bg_image: String,
}

impl Meta {
    pub fn of(info: &gio::FileInfo) -> Meta {
        let s = |a: &str| info.attribute_string(a).map(|v| v.to_string()).unwrap_or_default();
        Meta {
            emblems: if info.has_attribute("metadata::emblems") {
                info.attribute_stringv("metadata::emblems").iter().map(|k| k.to_string()).filter(|k| !k.is_empty()).collect()
            } else {
                vec![]
            },
            note: s("metadata::annotation"),
            icon: s("metadata::custom-icon"),
            icon_name: s("metadata::custom-icon-name"),
            bg_color: s("metadata::folder-background-color"),
            bg_image: s("metadata::folder-background-image"),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Meta::default()
    }

    /// `info` with this metadata in it (for icons::item_icon).
    pub fn applied_to(&self, info: &gio::FileInfo) -> gio::FileInfo {
        let i = info.dup();
        if !self.emblems.is_empty() {
            i.set_attribute_stringv("metadata::emblems", self.emblems.as_slice());
        }
        for (a, v) in [("metadata::annotation", &self.note), ("metadata::custom-icon", &self.icon), ("metadata::custom-icon-name", &self.icon_name)] {
            if !v.is_empty() {
                i.set_attribute_string(a, v);
            }
        }
        i
    }
}

/// A value to write (see write).
#[derive(Clone, Debug)]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Unset,
}

enum Job {
    /// The items in a folder that have metadata.
    Folder(String, async_channel::Sender<Vec<(String, Meta)>>),
    /// These items' metadata (empty for those without).
    Items(Vec<String>, async_channel::Sender<Vec<(String, Meta)>>),
    /// Writes (item, key, value), in order; the errors are answered.
    Write(Vec<(String, String, Value)>, async_channel::Sender<Vec<String>>),
}

thread_local! {
    static WORKER: RefCell<Option<std::sync::mpsc::Sender<Job>>> = const { RefCell::new(None) };
}

fn send(job: Job) {
    let tx = WORKER.with(|w| {
        w.borrow_mut()
            .get_or_insert_with(|| {
                let (tx, rx) = std::sync::mpsc::channel::<Job>();
                std::thread::spawn(move || {
                    while let Ok(job) = rx.recv() {
                        work(job);
                    }
                });
                tx
            })
            .clone()
    });
    let _ = tx.send(job);
}

fn work(job: Job) {
    match job {
        Job::Folder(uri, reply) => {
            let folder = gio::File::for_uri(&uri);
            let mut found = vec![];
            if let Ok(en) = folder.enumerate_children(&format!("standard::name,{ATTRS}"), gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                while let Ok(Some(info)) = en.next_file(gio::Cancellable::NONE) {
                    let m = Meta::of(&info);
                    if !m.is_empty() {
                        found.push((folder.child(info.name()).uri().to_string(), m));
                    }
                }
            }
            let _ = reply.send_blocking(found);
        }
        Job::Items(uris, reply) => {
            let found = uris
                .into_iter()
                .map(|u| {
                    let m = gio::File::for_uri(&u).query_info(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).map(|i| Meta::of(&i)).unwrap_or_default();
                    (u, m)
                })
                .collect();
            let _ = reply.send_blocking(found);
        }
        Job::Write(writes, reply) => {
            let mut errors = vec![];
            for (uri, key, value) in writes {
                let f = gio::File::for_uri(&uri);
                let v = match &value {
                    Value::Text(t) => crate::util::Meta::Text(t),
                    Value::List(l) => crate::util::Meta::List(l),
                    Value::Unset => crate::util::Meta::Unset,
                };
                if let Err(e) = crate::util::set_metadata(&f, &key, v) {
                    errors.push(format!("{}: {}", crate::util::display_name(&f), e.message()));
                }
            }
            let _ = reply.send_blocking(errors);
        }
    }
}

/// The items of `folder` that have metadata, given to `done` on the main
/// thread once read.
pub fn read_folder(folder: &gio::File, done: impl FnOnce(Vec<(String, Meta)>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    send(Job::Folder(folder.uri().to_string(), tx));
    glib::MainContext::default().spawn_local(async move {
        if let Ok(found) = rx.recv().await {
            done(found);
        }
    });
}

/// These items' metadata, given to `done` on the main thread once read.
pub fn read_items(uris: Vec<String>, done: impl FnOnce(Vec<(String, Meta)>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    send(Job::Items(uris, tx));
    glib::MainContext::default().spawn_local(async move {
        if let Ok(found) = rx.recv().await {
            done(found);
        }
    });
}

/// Writes metadata (item URI, key without "metadata::", value), in order
/// after anything asked before; `done` gets the errors (empty if none).
pub fn write(writes: Vec<(String, String, Value)>, done: impl FnOnce(Vec<String>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    send(Job::Write(writes, tx));
    glib::MainContext::default().spawn_local(async move {
        if let Ok(errors) = rx.recv().await {
            done(errors);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_from_and_into_infos() {
        let info = gio::FileInfo::new();
        assert!(Meta::of(&info).is_empty());
        info.set_attribute_stringv("metadata::emblems", ["important", ""].as_slice());
        info.set_attribute_string("metadata::annotation", "a note");
        let m = Meta::of(&info);
        assert_eq!(m, Meta { emblems: vec!["important".into()], note: "a note".into(), ..Meta::default() });
        let plain = gio::FileInfo::new();
        plain.set_name("x");
        let merged = m.applied_to(&plain);
        assert_eq!(Meta::of(&merged), m);
        assert_eq!(merged.name().to_str(), Some("x"));
        assert!(!plain.has_attribute("metadata::annotation"), "the original stays as it was");
    }
}
