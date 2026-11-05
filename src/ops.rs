//! File operations (copy, move, link, trash, delete) running on a worker
//! thread, with a progress window and conflict/error dialogs on the UI side.

use gio::prelude::*;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Link,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub src: String,
    pub dest_dir: String,
    /// Explicit target name (used by restore-from-trash). None = keep source name.
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Job {
    Transfer { kind: Kind, items: Vec<Item> },
    Trash(Vec<String>),
    Delete(Vec<String>),
    EmptyTrash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Skip,
    Replace, // "Merge" for folders
    KeepBoth,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrChoice {
    Skip,
    SkipAll,
    Retry,
    Cancel,
    DeletePermanently,
}

pub struct ConflictReq {
    pub name: String,
    pub dest_dir: String,
    pub src_is_dir: bool,
    pub dest_is_dir: bool,
    pub src_desc: String,
    pub dest_desc: String,
}

enum Msg {
    Progress { detail: String, fraction: f64 },
    Conflict(ConflictReq, mpsc::Sender<(Resolution, bool)>),
    Error {
        title: String,
        detail: String,
        offer_delete: bool,
        reply: mpsc::Sender<ErrChoice>,
    },
    Done(Vec<String>),
}

struct Cancelled;
type Flow = Result<(), Cancelled>;

struct Worker {
    tx: async_channel::Sender<Msg>,
    cancel: gio::Cancellable,
    total_bytes: u64,
    done_bytes: u64,
    total_items: u64,
    done_items: u64,
    by_bytes: bool,
    file_all: Option<Resolution>,
    dir_all: Option<Resolution>,
    skip_errors: bool,
    skipped: u64,
    last: Instant,
    verb: &'static str,
    /// Set from the progress window's pause button.
    paused: Arc<AtomicBool>,
    clock: Clock,
}

/// How long a job has been at work, as Caja's job timer: time paused or
/// waiting for an answer in a dialog is left out (the rate, and the time
/// left from it, would look worse than they are).
#[derive(Clone)]
struct Clock {
    start: Instant,
    /// Milliseconds paused or waiting.
    idle: Arc<std::sync::atomic::AtomicU64>,
}

impl Clock {
    fn new() -> Clock {
        Clock { start: Instant::now(), idle: Arc::default() }
    }

    fn working(&self) -> Duration {
        self.start.elapsed().saturating_sub(Duration::from_millis(self.idle.load(Ordering::Relaxed)))
    }

    /// Runs `f` (a wait) with its time left out.
    fn idle<R>(&self, f: impl FnOnce() -> R) -> R {
        let t = Instant::now();
        let r = f();
        self.idle.fetch_add(t.elapsed().as_millis() as u64, Ordering::Relaxed);
        r
    }
}

/// Blocks while the job is paused (until resumed or cancelled).
fn wait_while_paused(paused: &AtomicBool, cancel: &gio::Cancellable, clock: &Clock) {
    if paused.load(Ordering::Relaxed) {
        clock.idle(|| {
            while paused.load(Ordering::Relaxed) && !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
}

/// The rate is guessed from the job's average so far: before this it's
/// too unsteady to tell the time left, as in Caja.
const RELIABLE_RATE_AFTER: Duration = Duration::from_secs(15);

/// A time left as Caja words it: "40 seconds", "3 minutes", "1 hour, 20
/// minutes", "approximately 5 hours".
fn format_time_left(secs: u64) -> String {
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match secs {
        0..60 => plural(secs, "second", "seconds"),
        60..3600 => plural(secs / 60, "minute", "minutes"),
        3600..14400 => format!("{}, {}", plural(secs / 3600, "hour", "hours"), plural(secs % 3600 / 60, "minute", "minutes")),
        _ => format!("approximately {}", plural(secs / 3600, "hour", "hours")),
    }
}

/// Caja's progress details for a job counted in bytes: "4 MB of 10 MB",
/// then, once the rate can be told, "4 MB of 10 MB — 2 minutes left
/// (1 MB/sec)".
fn bytes_detail(done: u64, total: u64, working: Duration) -> String {
    let total = total.max(done);
    let mut s = format!("{} of {}", util::format_size(done), util::format_size(total));
    if working >= RELIABLE_RATE_AFTER && done > 0 {
        let rate = done as f64 / working.as_secs_f64();
        let left = ((total - done) as f64 / rate) as u64;
        s.push_str(&format!(" \u{2014} {} left ({}/sec)", format_time_left(left), util::format_size(rate as u64)));
    }
    s
}

/// The same for a job counted in items: "40 of 120 items — 1 minute left".
fn items_detail(done: u64, total: u64, working: Duration) -> String {
    let total = total.max(done);
    let mut s = format!("{done} of {total} items");
    if working >= RELIABLE_RATE_AFTER && done > 0 {
        let rate = done as f64 / working.as_secs_f64();
        s.push_str(&format!(" \u{2014} {} left", format_time_left(((total - done) as f64 / rate) as u64)));
    }
    s
}

const NOFOLLOW: gio::FileQueryInfoFlags = gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS;

fn kind_of(f: &gio::File) -> Option<gio::FileType> {
    f.query_info("standard::type", NOFOLLOW, gio::Cancellable::NONE)
        .ok()
        .map(|i| i.file_type())
}

fn describe(f: &gio::File) -> String {
    match f.query_info(
        "standard::size,standard::type,time::modified",
        NOFOLLOW,
        gio::Cancellable::NONE,
    ) {
        Ok(i) => {
            let when = util::format_time(i.attribute_uint64("time::modified") as i64);
            if i.file_type() == gio::FileType::Directory {
                format!("Folder, modified {when}")
            } else {
                format!("{}, modified {when}", util::format_size(i.size() as u64))
            }
        }
        Err(_) => String::new(),
    }
}

fn base_name(f: &gio::File) -> String {
    f.basename()
        .map(|b| b.to_string_lossy().to_string())
        .unwrap_or_default()
}

impl Worker {
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn progress(&mut self, detail: &str, force: bool) {
        // Every step of a job reports progress, so this is where it pauses.
        wait_while_paused(&self.paused, &self.cancel, &self.clock);
        if !force && self.last.elapsed() < Duration::from_millis(80) {
            return;
        }
        self.last = Instant::now();
        let fraction = if self.by_bytes && self.total_bytes > 0 {
            self.done_bytes as f64 / self.total_bytes as f64
        } else if self.total_items > 0 {
            self.done_items as f64 / self.total_items as f64
        } else {
            0.0
        };
        let working = self.clock.working();
        let detail = if self.by_bytes {
            format!("{detail}\n{}", bytes_detail(self.done_bytes, self.total_bytes, working))
        } else {
            format!("{detail}\n{}", items_detail(self.done_items, self.total_items, working))
        };
        let _ = self.tx.send_blocking(Msg::Progress {
            detail,
            fraction: fraction.clamp(0.0, 1.0),
        });
    }

    fn ask_error(&mut self, title: String, err: &glib::Error, offer_delete: bool) -> ErrChoice {
        if err.matches(gio::IOErrorEnum::Cancelled) {
            return ErrChoice::Cancel;
        }
        if self.skip_errors {
            self.skipped += 1;
            return ErrChoice::Skip;
        }
        let (rtx, rrx) = mpsc::channel();
        let _ = self.tx.send_blocking(Msg::Error {
            title,
            detail: err.message().to_string(),
            offer_delete,
            reply: rtx,
        });
        let c = self.clock.idle(|| rrx.recv().unwrap_or(ErrChoice::Cancel));
        match c {
            ErrChoice::SkipAll => {
                self.skip_errors = true;
                self.skipped += 1;
                ErrChoice::Skip
            }
            ErrChoice::Skip => {
                self.skipped += 1;
                c
            }
            ErrChoice::Cancel => {
                self.cancel.cancel();
                c
            }
            _ => c,
        }
    }

    fn ask_conflict(&mut self, src: &gio::File, dst: &gio::File, src_dir: bool, dst_dir: bool) -> Resolution {
        let both_dirs = src_dir && dst_dir;
        let remembered = if both_dirs { self.dir_all } else { self.file_all };
        if let Some(r) = remembered {
            return r;
        }
        let (rtx, rrx) = mpsc::channel();
        let req = ConflictReq {
            name: base_name(dst),
            dest_dir: dst
                .parent()
                .map(|p| util::display_name(&p))
                .unwrap_or_default(),
            src_is_dir: src_dir,
            dest_is_dir: dst_dir,
            src_desc: describe(src),
            dest_desc: describe(dst),
        };
        let _ = self.tx.send_blocking(Msg::Conflict(req, rtx));
        let (r, all) = self.clock.idle(|| rrx.recv().unwrap_or((Resolution::Cancel, false)));
        if all {
            if both_dirs {
                self.dir_all = Some(r);
            } else {
                self.file_all = Some(r);
            }
        }
        if r == Resolution::Cancel {
            self.cancel.cancel();
        }
        r
    }

    /// Entries of folder `f`. Read errors go through the error dialog: Retry reads
    /// the folder again, Skip gives None and counts as skipped, so a move that
    /// couldn't copy everything keeps its source.
    fn list_children(&mut self, f: &gio::File, what: &str) -> Result<Option<Vec<std::path::PathBuf>>, Cancelled> {
        loop {
            if self.cancelled() {
                return Err(Cancelled);
            }
            let err = match f.enumerate_children("standard::name", NOFOLLOW, Some(&self.cancel)) {
                Ok(en) => {
                    let mut v = vec![];
                    loop {
                        match en.next_file(Some(&self.cancel)) {
                            Ok(Some(ci)) => v.push(ci.name()),
                            Ok(None) => return Ok(Some(v)),
                            Err(e) => break e,
                        }
                    }
                }
                Err(e) => e,
            };
            match self.ask_error(format!("Error while {what} “{}”", base_name(f)), &err, false) {
                ErrChoice::Retry => continue,
                ErrChoice::Cancel => return Err(Cancelled),
                _ => return Ok(None),
            }
        }
    }

    /// Count bytes and items below `f` (not following symlinks).
    fn scan(&self, f: &gio::File, bytes: &mut u64, items: &mut u64) {
        if self.cancelled() {
            return;
        }
        let Ok(info) = f.query_info("standard::type,standard::size", NOFOLLOW, Some(&self.cancel)) else {
            *items += 1;
            return;
        };
        *items += 1;
        if info.file_type() == gio::FileType::Directory {
            if let Ok(en) = f.enumerate_children("standard::name,standard::type,standard::size", NOFOLLOW, Some(&self.cancel)) {
                while let Ok(Some(ci)) = en.next_file(Some(&self.cancel)) {
                    let child = f.child(ci.name());
                    if ci.file_type() == gio::FileType::Directory {
                        self.scan(&child, bytes, items);
                    } else {
                        *items += 1;
                        *bytes += ci.size().max(0) as u64;
                    }
                }
            }
        } else {
            *bytes += info.size().max(0) as u64;
        }
    }

    fn delete_rec(&mut self, f: &gio::File) -> Flow {
        loop {
            if self.cancelled() {
                return Err(Cancelled);
            }
            match f.delete(Some(&self.cancel)) {
                Ok(()) => {
                    self.done_items += 1;
                    self.progress(&format!("Deleting “{}”", base_name(f)), false);
                    return Ok(());
                }
                Err(e) if e.matches(gio::IOErrorEnum::NotEmpty) => {
                    let Some(children) = self.list_children(f, "deleting")? else {
                        return Ok(());
                    };
                    let before = self.skipped;
                    for c in children {
                        self.delete_rec(&f.child(c))?;
                    }
                    if self.skipped != before {
                        return Ok(()); // something inside was kept
                    }
                    // try deleting the (now empty) dir again
                    match f.delete(Some(&self.cancel)) {
                        Ok(()) => {
                            self.done_items += 1;
                            return Ok(());
                        }
                        Err(e) => {
                            return match self.ask_error(format!("Error while deleting “{}”", base_name(f)), &e, false) {
                                ErrChoice::Retry => continue,
                                ErrChoice::Cancel => Err(Cancelled),
                                _ => Ok(()),
                            }
                        }
                    }
                }
                Err(e) if e.matches(gio::IOErrorEnum::NotFound) => return Ok(()),
                Err(e) => {
                    match self.ask_error(format!("Error while deleting “{}”", base_name(f)), &e, false) {
                        ErrChoice::Retry => continue,
                        ErrChoice::Cancel => return Err(Cancelled),
                        _ => return Ok(()),
                    }
                }
            }
        }
    }

    /// `overwrite`: the user already chose Replace for `dst` (a move falling
    /// back to copy across filesystems), so don't ask again.
    fn copy_rec(&mut self, src: &gio::File, dst: &gio::File, overwrite: bool, created: &mut Option<String>) -> Flow {
        if self.cancelled() {
            return Err(Cancelled);
        }
        let info = loop {
            match src.query_info("standard::type,standard::size,unix::mode", NOFOLLOW, Some(&self.cancel)) {
                Ok(i) => break i,
                Err(e) => match self.ask_error(format!("Error while copying “{}”", base_name(src)), &e, false) {
                    ErrChoice::Retry => continue,
                    ErrChoice::Cancel => return Err(Cancelled),
                    _ => return Ok(()),
                },
            }
        };
        let mut dst = dst.clone();
        if info.file_type() == gio::FileType::Directory {
            // Create (or merge into) the destination folder.
            loop {
                match dst.make_directory(Some(&self.cancel)) {
                    Ok(()) => break,
                    Err(e) if e.matches(gio::IOErrorEnum::Exists) => {
                        let dst_dir = kind_of(&dst) == Some(gio::FileType::Directory);
                        let r = if overwrite { Resolution::Replace } else { self.ask_conflict(src, &dst, true, dst_dir) };
                        match r {
                            Resolution::Skip => {
                                self.skipped += 1;
                                let (mut b, mut n) = (0, 0);
                                self.scan(src, &mut b, &mut n);
                                self.done_bytes += b;
                                self.done_items += n;
                                return Ok(());
                            }
                            Resolution::Replace => {
                                if dst_dir {
                                    break; // merge
                                }
                                let before = self.skipped;
                                self.delete_rec(&dst)?;
                                if self.skipped != before {
                                    return Ok(()); // couldn't remove it, give up on this item
                                }
                            }
                            Resolution::KeepBoth => {
                                let parent = dst.parent().unwrap();
                                dst = parent.child(util::unique_name(&parent, &base_name(&dst), true));
                            }
                            Resolution::Cancel => return Err(Cancelled),
                        }
                    }
                    Err(e) => match self.ask_error(format!("Error while copying “{}”", base_name(src)), &e, false) {
                        ErrChoice::Retry => continue,
                        ErrChoice::Cancel => return Err(Cancelled),
                        _ => return Ok(()),
                    },
                }
            }
            if created.is_none() {
                *created = Some(dst.uri().to_string());
            }
            self.done_items += 1;
            let Some(children) = self.list_children(src, "reading")? else {
                return Ok(());
            };
            let mut none = None;
            for c in children {
                self.copy_rec(&src.child(&c), &dst.child(&c), false, &mut none)?;
            }
            // Re-read the mode: it may have changed since `info` (e.g. the user
            // fixed an unreadable folder and pressed Retry).
            let mode = src
                .query_info("unix::mode", NOFOLLOW, gio::Cancellable::NONE)
                .ok()
                .filter(|i| i.has_attribute("unix::mode"))
                .map(|i| i.attribute_uint32("unix::mode"))
                .or_else(|| info.has_attribute("unix::mode").then(|| info.attribute_uint32("unix::mode")));
            if let Some(mode) = mode {
                let _ = dst.set_attribute_uint32("unix::mode", mode, NOFOLLOW, gio::Cancellable::NONE);
            }
            return Ok(());
        }

        // Regular file / symlink / special.
        let size = info.size().max(0) as u64;
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        let name = base_name(src);
        loop {
            let base = self.done_bytes;
            let total = self.total_bytes;
            let tx = self.tx.clone();
            let mut last = Instant::now();
            let label = format!("Copying “{name}”");
            let (paused, cancel, clock) = (self.paused.clone(), self.cancel.clone(), self.clock.clone());
            let mut cb = |cur: i64, _tot: i64| {
                // Pausing in the middle of a big file stops the copy here.
                wait_while_paused(&paused, &cancel, &clock);
                if last.elapsed() > Duration::from_millis(100) && total > 0 {
                    last = Instant::now();
                    let done = base + cur.max(0) as u64;
                    let _ = tx.send_blocking(Msg::Progress {
                        detail: format!("{label}\n{}", bytes_detail(done, total, clock.working())),
                        fraction: (done as f64 / total as f64).clamp(0.0, 1.0),
                    });
                }
            };
            // Regular files between local paths (local disks, and network
            // shares mounted by the kernel) are copied here, in pieces.
            let pieces = if info.file_type() == gio::FileType::Regular && src.is_native() && dst.is_native() {
                match (src.path(), dst.path()) {
                    (Some(s), Some(d)) => copy_in_pieces(&s, &d, flags.contains(gio::FileCopyFlags::OVERWRITE), &self.cancel, &mut cb),
                    _ => None,
                }
            } else {
                None
            };
            let res = match pieces.map_or_else(|| src.copy(&dst, flags, Some(&self.cancel), Some(&mut cb)), |r| {
                // GIO copies these along with the data.
                if r.is_ok() {
                    let _ = src.copy_attributes(&dst, gio::FileCopyFlags::ALL_METADATA | gio::FileCopyFlags::NOFOLLOW_SYMLINKS, Some(&self.cancel));
                }
                r
            }) {
                // Phones (MTP) often can't copy a file to another place on
                // themselves ("could not copy object"), and GIO doesn't fall
                // back for that error: copy the data through here instead.
                Err(e) if same_mtp_device(src, &dst) && !e.matches(gio::IOErrorEnum::Exists) && !e.matches(gio::IOErrorEnum::Cancelled) => {
                    stream_copy(src, &dst, flags.contains(gio::FileCopyFlags::OVERWRITE), &self.cancel, &mut cb)
                }
                r => r,
            };
            if let Err(e) = &res {
                // GIO leaves what it wrote of a cancelled or failed copy
                // (and a retry would then find it in the way). Only a file
                // made here: one being replaced may still be the old one.
                if !e.matches(gio::IOErrorEnum::Exists) && !flags.contains(gio::FileCopyFlags::OVERWRITE) {
                    let _ = dst.delete(gio::Cancellable::NONE);
                }
            }
            match res {
                Ok(()) => {
                    // libmtp creates every file it downloads as rwxr-----:
                    // give files from a phone the usual permissions instead.
                    if is_mtp(src) && dst.path().is_some() {
                        let _ = dst.set_attribute_uint32("unix::mode", 0o666 & !umask(), gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, Some(&self.cancel));
                    }
                    self.done_bytes += size;
                    self.done_items += 1;
                    if created.is_none() {
                        *created = Some(dst.uri().to_string());
                    }
                    self.progress(&format!("Copying “{name}”"), false);
                    return Ok(());
                }
                Err(e) if e.matches(gio::IOErrorEnum::Exists) => {
                    let dst_dir = kind_of(&dst) == Some(gio::FileType::Directory);
                    let r = if overwrite { Resolution::Replace } else { self.ask_conflict(src, &dst, false, dst_dir) };
                    match r {
                        Resolution::Skip => {
                            self.skipped += 1;
                            self.done_bytes += size;
                            self.done_items += 1;
                            return Ok(());
                        }
                        Resolution::Replace => {
                            if dst_dir {
                                let before = self.skipped;
                                self.delete_rec(&dst)?;
                                if self.skipped != before {
                                    self.done_bytes += size;
                                    return Ok(()); // couldn't remove it, give up on this item
                                }
                            } else {
                                flags |= gio::FileCopyFlags::OVERWRITE;
                            }
                        }
                        Resolution::KeepBoth => {
                            let parent = dst.parent().unwrap();
                            dst = parent.child(util::unique_name(&parent, &base_name(&dst), false));
                        }
                        Resolution::Cancel => return Err(Cancelled),
                    }
                }
                Err(e) => match self.ask_error(format!("Error while copying “{name}”"), &e, false) {
                    ErrChoice::Retry => continue,
                    ErrChoice::Cancel => return Err(Cancelled),
                    _ => {
                        self.done_bytes += size;
                        return Ok(());
                    }
                },
            }
        }
    }

    fn move_rec(&mut self, src: &gio::File, dst: &gio::File, created: &mut Option<String>) -> Flow {
        if self.cancelled() {
            return Err(Cancelled);
        }
        let mut dst = dst.clone();
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS
            | gio::FileCopyFlags::ALL_METADATA
            | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE;
        let name = base_name(src);
        loop {
            match src.move_(&dst, flags, Some(&self.cancel), None) {
                Ok(()) => {
                    self.done_items += 1;
                    if created.is_none() {
                        *created = Some(dst.uri().to_string());
                    }
                    self.progress(&format!("Moving “{name}”"), false);
                    return Ok(());
                }
                Err(e) if e.matches(gio::IOErrorEnum::Exists) => {
                    let src_dir = kind_of(src) == Some(gio::FileType::Directory);
                    let dst_dir = kind_of(&dst) == Some(gio::FileType::Directory);
                    match self.ask_conflict(src, &dst, src_dir, dst_dir) {
                        Resolution::Skip => {
                            self.skipped += 1;
                            self.done_items += 1;
                            return Ok(());
                        }
                        Resolution::Replace => {
                            if src_dir && dst_dir {
                                // merge: move children one by one, then remove src
                                let Some(children) = self.list_children(src, "reading")? else {
                                    return Ok(());
                                };
                                let before = self.skipped;
                                let mut none = None;
                                for c in children {
                                    self.move_rec(&src.child(&c), &dst.child(&c), &mut none)?;
                                }
                                if self.skipped == before {
                                    let _ = src.delete(gio::Cancellable::NONE);
                                }
                                if created.is_none() {
                                    *created = Some(dst.uri().to_string());
                                }
                                self.done_items += 1;
                                return Ok(());
                            }
                            if dst_dir {
                                let before = self.skipped;
                                self.delete_rec(&dst)?;
                                if self.skipped != before {
                                    return Ok(()); // couldn't remove it, keep the source
                                }
                            } else {
                                flags |= gio::FileCopyFlags::OVERWRITE;
                            }
                        }
                        Resolution::KeepBoth => {
                            let parent = dst.parent().unwrap();
                            dst = parent.child(util::unique_name(&parent, &base_name(&dst), src_dir));
                        }
                        Resolution::Cancel => return Err(Cancelled),
                    }
                }
                Err(e)
                    if e.matches(gio::IOErrorEnum::WouldRecurse)
                        || e.matches(gio::IOErrorEnum::NotSupported)
                        || e.matches(gio::IOErrorEnum::WouldMerge) =>
                {
                    // Different filesystem: copy, then delete the source.
                    let (mut b, mut n) = (0, 0);
                    self.scan(src, &mut b, &mut n);
                    let saved = (self.by_bytes, self.total_bytes, self.done_bytes);
                    self.by_bytes = true;
                    self.total_bytes = b;
                    self.done_bytes = 0;
                    let before = self.skipped;
                    let overwrite = flags.contains(gio::FileCopyFlags::OVERWRITE);
                    let r = self.copy_rec(src, &dst, overwrite, created);
                    (self.by_bytes, self.total_bytes, self.done_bytes) = saved;
                    r?;
                    if self.skipped == before {
                        self.delete_rec(src)?;
                    }
                    return Ok(());
                }
                Err(e) => match self.ask_error(format!("Error while moving “{name}”"), &e, false) {
                    ErrChoice::Retry => continue,
                    ErrChoice::Cancel => return Err(Cancelled),
                    _ => return Ok(()),
                },
            }
        }
    }

    fn run(&mut self, job: Job) -> Vec<String> {
        let mut created = vec![];
        match job {
            Job::Transfer { kind, items } => {
                self.verb = match kind {
                    Kind::Copy => "Copying",
                    Kind::Move => "Moving",
                    Kind::Link => "Linking",
                };
                if kind == Kind::Copy {
                    self.by_bytes = true;
                    for it in &items {
                        let (mut b, mut n) = (0, 0);
                        self.scan(&gio::File::for_uri(&it.src), &mut b, &mut n);
                        self.total_bytes += b;
                        self.total_items += n;
                    }
                } else {
                    self.total_items = items.len() as u64;
                }
                self.progress("Preparing…", true);
                for it in items {
                    let src = gio::File::for_uri(&it.src);
                    let dest_dir = gio::File::for_uri(&it.dest_dir);
                    let src_is_dir = kind_of(&src) == Some(gio::FileType::Directory);
                    if kind != Kind::Link && (dest_dir.has_prefix(&src) || src.equal(&dest_dir)) {
                        let e = glib::Error::new(
                            gio::IOErrorEnum::InvalidArgument,
                            "The destination is inside the source folder.",
                        );
                        if self.ask_error(format!("Cannot {} “{}” into itself", if kind == Kind::Move { "move" } else { "copy" }, base_name(&src)), &e, false) == ErrChoice::Cancel {
                            break;
                        }
                        continue;
                    }
                    let same_dir = src.parent().map(|p| p.equal(&dest_dir)).unwrap_or(false);
                    let name = it.name.clone().unwrap_or_else(|| base_name(&src));
                    // Restoring from the trash: recreate the original folder if needed.
                    if it.name.is_some() && !dest_dir.query_exists(gio::Cancellable::NONE) {
                        let _ = dest_dir.make_directory_with_parents(gio::Cancellable::NONE);
                    }
                    let mut c = None;
                    let r = match kind {
                        Kind::Copy => {
                            let name = if same_dir { util::copy_name(&dest_dir, &name, src_is_dir) } else { name };
                            self.copy_rec(&src, &dest_dir.child(&name), false, &mut c)
                        }
                        Kind::Move => {
                            if same_dir && it.name.is_none() {
                                self.done_items += 1;
                                Ok(())
                            } else {
                                self.move_rec(&src, &dest_dir.child(&name), &mut c)
                            }
                        }
                        Kind::Link => {
                            let name = if same_dir { format!("Link to {name}") } else { name };
                            let name = util::unique_name(&dest_dir, &name, src_is_dir);
                            let dst = dest_dir.child(&name);
                            let target = src
                                .path()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| src.uri().to_string());
                            match dst.make_symbolic_link(target, Some(&self.cancel)) {
                                Ok(()) => {
                                    c = Some(dst.uri().to_string());
                                    self.done_items += 1;
                                    Ok(())
                                }
                                Err(e) => match self.ask_error(format!("Cannot create link to “{name}”"), &e, false) {
                                    ErrChoice::Cancel => Err(Cancelled),
                                    _ => Ok(()),
                                },
                            }
                        }
                    };
                    if let Some(c) = c {
                        created.push(c);
                    }
                    if r.is_err() {
                        break;
                    }
                }
            }
            Job::Trash(uris) => {
                self.verb = "Moving to Trash";
                self.total_items = uris.len() as u64;
                self.progress("Preparing…", true);
                for u in uris {
                    let f = gio::File::for_uri(&u);
                    loop {
                        if self.cancelled() {
                            break;
                        }
                        match f.trash(Some(&self.cancel)) {
                            Ok(()) => {
                                self.done_items += 1;
                                self.progress(&format!("Trashing “{}”", base_name(&f)), false);
                                break;
                            }
                            Err(e) => match self.ask_error(
                                format!("“{}” can't be put in the trash. Delete it permanently?", base_name(&f)),
                                &e,
                                true,
                            ) {
                                ErrChoice::Retry => continue,
                                ErrChoice::DeletePermanently => {
                                    let _ = self.delete_rec(&f);
                                    break;
                                }
                                _ => break,
                            },
                        }
                    }
                    if self.cancelled() {
                        break;
                    }
                }
            }
            Job::Delete(uris) => {
                self.verb = "Deleting";
                for u in &uris {
                    let (mut b, mut n) = (0, 0);
                    self.scan(&gio::File::for_uri(u), &mut b, &mut n);
                    self.total_items += n;
                }
                self.progress("Preparing…", true);
                for u in uris {
                    if self.delete_rec(&gio::File::for_uri(&u)).is_err() {
                        break;
                    }
                }
            }
            Job::EmptyTrash => {
                self.verb = "Emptying Trash";
                let trash = gio::File::for_uri("trash:///");
                let kids: Vec<gio::File> = trash
                    .enumerate_children("standard::name", NOFOLLOW, Some(&self.cancel))
                    .map(|en| {
                        let mut v = vec![];
                        while let Ok(Some(ci)) = en.next_file(Some(&self.cancel)) {
                            v.push(trash.child(ci.name()));
                        }
                        v
                    })
                    .unwrap_or_default();
                self.total_items = kids.len() as u64;
                self.progress("Preparing…", true);
                for k in kids {
                    if self.delete_rec(&k).is_err() {
                        break;
                    }
                }
            }
        }
        created
    }
}

// ---------------------------------------------------------------------------
// UI side
//
// Every running or waiting operation has one shared `Op`. Each place that
// lists operations (a window's operations popover, the File Operations
// window) is a `View` with rows of its own, built from and kept in step
// with the `Op`s, so all of them can be open at once.

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpState {
    /// In the queue: Start Now or Cancel.
    Waiting,
    /// Has a worker: progress, Pause and Cancel.
    Running,
}

struct Op {
    id: u64,
    title: String,
    state: OpState,
    detail: RefCell<String>,
    fraction: std::cell::Cell<f64>,
    paused: Arc<AtomicBool>,
    cancel: gio::Cancellable,
    cancelled: std::cell::Cell<bool>,
}

/// The widgets of one operation's row in one view.
struct Row {
    row: gtk::Box,
    detail: gtk::Label,
    bar: gtk::ProgressBar,
    pause: gtk::ToggleButton,
    pause_img: gtk::Image,
    cancel: gtk::Button,
}

struct View {
    holder: glib::WeakRef<gtk::Box>,
    rows: RefCell<Vec<(u64, Row)>>,
}

/// The progress line under the bar, marked while the job is paused.
fn progress_text(detail: &str, paused: bool) -> String {
    if paused {
        format!("Paused — {}", detail.replace('\n', " — "))
    } else {
        detail.to_string()
    }
}

thread_local! {
    static RUNNING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Whether an operation since the queue last ran empty took a while
    /// (and was listed): only then is its end told, as in Caja.
    static LISTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The operations shown, in order.
    static OPS: RefCell<Vec<Rc<Op>>> = const { RefCell::new(vec![]) };
    static VIEWS: RefCell<Vec<Rc<View>>> = const { RefCell::new(vec![]) };
    static OPS_WINDOW: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = RefCell::new(vec![]);
}

/// A new list of the operations (for a window's operations popover); it
/// stays up to date by itself.
pub fn new_view() -> gtk::Box {
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 12);
    holder.set_border_width(12);
    let view = Rc::new(View { holder: holder.downgrade(), rows: RefCell::new(vec![]) });
    VIEWS.with(|v| v.borrow_mut().push(view.clone()));
    sync_view(&view);
    holder
}

/// Shows the operations in their own window.
pub fn open_window() {
    let win = OPS_WINDOW.with(|w| {
        w.borrow_mut()
            .get_or_insert_with(|| {
                let win = gtk::Window::new(gtk::WindowType::Toplevel);
                win.set_title("File Operations");
                win.set_default_size(440, -1);
                win.set_resizable(false);
                win.set_icon_name(Some("system-file-manager"));
                win.set_type_hint(gdk::WindowTypeHint::Dialog);
                win.connect_delete_event(|w, _| {
                    w.hide();
                    glib::Propagation::Stop
                });
                win.add(&new_view());
                win
            })
            .clone()
    });
    win.show_all();
    win.present();
}

/// Calls `f` whenever operations come, go or make progress.
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

/// Closes the File Operations window once nothing is running or waiting.
/// (Not merely when the list is empty: a new operation only joins it after
/// a moment, and the window may have been opened for it already.)
fn hide_window_if_idle() {
    if !is_busy() && OPS.with(|o| o.borrow().is_empty()) {
        if let Some(w) = OPS_WINDOW.with(|w| w.borrow().clone()) {
            w.hide();
        }
    }
}

fn changed() {
    LISTENERS.with(|l| {
        for f in l.borrow().iter() {
            f();
        }
    });
}

/// (operations shown, overall progress of the running ones).
pub fn summary() -> (usize, f64) {
    OPS.with(|o| {
        let o = o.borrow();
        let running: Vec<f64> = o.iter().filter(|op| op.state == OpState::Running).map(|op| op.fraction.get()).collect();
        let frac = if running.is_empty() { 0.0 } else { running.iter().sum::<f64>() / running.len() as f64 };
        (o.len(), frac)
    })
}

fn add_op(op: Rc<Op>) {
    OPS.with(|o| o.borrow_mut().push(op));
    sync_views();
    changed();
}

fn remove_op(id: u64) {
    OPS.with(|o| o.borrow_mut().retain(|op| op.id != id));
    sync_views();
    hide_window_if_idle();
    changed();
}

/// Brings every view's rows in line with OPS (adds, removes, reorders).
fn sync_views() {
    let views = VIEWS.with(|v| {
        // Views whose window is gone drop out.
        v.borrow_mut().retain(|view| view.holder.upgrade().is_some());
        v.borrow().clone()
    });
    for view in &views {
        sync_view(view);
    }
}

fn sync_view(view: &View) {
    let Some(holder) = view.holder.upgrade() else { return };
    let ops = OPS.with(|o| o.borrow().clone());
    let mut rows = view.rows.borrow_mut();
    rows.retain(|(id, r)| {
        let keep = ops.iter().any(|op| op.id == *id);
        if !keep {
            holder.remove(&r.row);
        }
        keep
    });
    for (pos, op) in ops.iter().enumerate() {
        if !rows.iter().any(|(id, _)| *id == op.id) {
            let r = build_row(op);
            holder.pack_start(&r.row, false, false, 0);
            r.row.show_all();
            update_row(op, &r);
            rows.push((op.id, r));
        }
        if let Some((_, r)) = rows.iter().find(|(id, _)| *id == op.id) {
            holder.reorder_child(&r.row, pos as i32);
        }
    }
}

/// Refreshes `op`'s rows in every view (progress, pause and cancel state).
fn update_op(op: &Op) {
    let views = VIEWS.with(|v| v.borrow().clone());
    for view in views {
        if let Some((_, r)) = view.rows.borrow().iter().find(|(id, _)| *id == op.id) {
            update_row(op, r);
        }
    }
}

fn update_row(op: &Op, r: &Row) {
    if op.state == OpState::Waiting {
        return;
    }
    let paused = op.paused.load(Ordering::Relaxed);
    r.detail.set_text(&progress_text(&op.detail.borrow(), paused));
    r.bar.set_fraction(op.fraction.get());
    // Only when it differs: setting it fires toggled, which calls set_paused.
    if r.pause.is_active() != paused {
        r.pause.set_active(paused);
    }
    let icon = if paused { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" };
    r.pause_img.set_from_icon_name(Some(icon), gtk::IconSize::Button);
    r.pause.set_tooltip_text(Some(if paused { "Resume" } else { "Pause" }));
    // A cancelled job can't be paused or cancelled any more.
    r.pause.set_sensitive(!op.cancelled.get());
    r.cancel.set_sensitive(!op.cancelled.get());
}

fn set_paused(op: &Op, on: bool) {
    if op.paused.load(Ordering::Relaxed) != on {
        op.paused.store(on, Ordering::Relaxed);
        update_op(op);
    }
}

fn build_row(op: &Rc<Op>) -> Row {
    let r = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let detail = gtk::Label::new(None);
    detail.set_xalign(0.0);
    detail.set_ellipsize(pango::EllipsizeMode::Middle);
    detail.style_context().add_class("dim-label");
    let bar = gtk::ProgressBar::new();
    let pause_img = gtk::Image::from_icon_name(Some("media-playback-pause-symbolic"), gtk::IconSize::Button);
    let pause = gtk::ToggleButton::new();
    pause.add(&pause_img);
    let cancel = gtk::Button::from_icon_name(Some("process-stop-symbolic"), gtk::IconSize::Button);
    cancel.set_tooltip_text(Some("Cancel"));
    let id = op.id;
    match op.state {
        OpState::Waiting => {
            detail.set_text("Waiting for other operations…");
            detail.set_hexpand(true);
            let go = gtk::Button::from_icon_name(Some("media-skip-forward-symbolic"), gtk::IconSize::Button);
            go.set_tooltip_text(Some("Start now, alongside the running operations"));
            go.connect_clicked(move |_| {
                if let Some(w) = take_waiting(id) {
                    (w.start)();
                }
            });
            cancel.connect_clicked(move |_| {
                take_waiting(id);
            });
            hb.pack_start(&detail, true, true, 0);
            hb.pack_start(&go, false, false, 0);
            hb.pack_start(&cancel, false, false, 0);
            r.pack_start(&bold_label(&op.title), false, false, 0);
            r.pack_start(&hb, false, false, 0);
        }
        OpState::Running => {
            bar.set_hexpand(true);
            bar.set_valign(gtk::Align::Center);
            {
                let op = Rc::downgrade(op);
                pause.connect_toggled(move |b| {
                    if let Some(op) = op.upgrade() {
                        set_paused(&op, b.is_active());
                    }
                });
            }
            {
                let op = Rc::downgrade(op);
                cancel.connect_clicked(move |_| {
                    if let Some(op) = op.upgrade() {
                        op.cancel.cancel();
                        op.cancelled.set(true);
                        update_op(&op);
                    }
                });
            }
            hb.pack_start(&bar, true, true, 0);
            hb.pack_start(&pause, false, false, 0);
            hb.pack_start(&cancel, false, false, 0);
            r.pack_start(&bold_label(&op.title), false, false, 0);
            r.pack_start(&hb, false, false, 0);
            r.pack_start(&detail, false, false, 0);
        }
    }
    Row { row: r, detail, bar, pause, pause_img, cancel }
}

/// Whether any operation is running or waiting in the queue.
pub fn is_busy() -> bool {
    RUNNING.with(|r| r.get() > 0) || QUEUE.with(|q| !q.borrow().is_empty())
}

pub fn title_for(job: &Job) -> String {
    match job {
        Job::Transfer { kind, items } => {
            let n = items.len();
            let what = if n == 1 {
                format!("“{}”", gio::File::for_uri(&items[0].src).basename().map(|b| b.to_string_lossy().to_string()).unwrap_or_default())
            } else {
                format!("{n} items")
            };
            let to = items
                .first()
                .map(|i| util::display_name(&gio::File::for_uri(&i.dest_dir)))
                .unwrap_or_default();
            match kind {
                Kind::Copy => format!("Copying {what} to “{to}”"),
                Kind::Move => format!("Moving {what} to “{to}”"),
                Kind::Link => format!("Creating links to {what}"),
            }
        }
        Job::Trash(v) => format!("Moving {} item{} to the Trash", v.len(), if v.len() == 1 { "" } else { "s" }),
        Job::Delete(v) => format!("Deleting {} item{}", v.len(), if v.len() == 1 { "" } else { "s" }),
        Job::EmptyTrash => "Emptying the Trash".into(),
    }
}

/// A copy or move waiting for the operations before it to finish.
struct Waiting {
    id: u64,
    start: Box<dyn FnOnce()>,
    /// Keeps the app running for it, like a running job does.
    _hold: Option<gio::ApplicationHoldGuard>,
}

thread_local! {
    static QUEUE: RefCell<std::collections::VecDeque<Waiting>> = RefCell::new(Default::default());
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn next_id() -> u64 {
    NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    })
}

/// Run a job. `on_done` receives the URIs of top-level items created.
/// Copies and moves wait their turn while other operations run or wait
/// (their row has a Start Now button); everything else starts right away.
pub fn run(parent: Option<gtk::Window>, job: Job, on_done: impl Fn(Vec<String>) + 'static) {
    let queueable = matches!(job, Job::Transfer { kind: Kind::Copy | Kind::Move, .. });
    if queueable && is_busy() {
        enqueue(parent, job, Box::new(on_done));
    } else {
        start(parent, job, Box::new(on_done), false);
    }
}

fn enqueue(parent: Option<gtk::Window>, job: Job, on_done: Box<dyn Fn(Vec<String>)>) {
    let id = next_id();
    let op = Rc::new(Op {
        id,
        title: title_for(&job),
        state: OpState::Waiting,
        detail: RefCell::new(String::new()),
        fraction: std::cell::Cell::new(0.0),
        paused: Arc::new(AtomicBool::new(false)),
        cancel: gio::Cancellable::new(),
        cancelled: std::cell::Cell::new(false),
    });
    let hold = gio::Application::default().map(|a| a.hold());
    QUEUE.with(|q| {
        q.borrow_mut().push_back(Waiting {
            id,
            start: Box::new(move || start(parent, job, on_done, true)),
            _hold: hold,
        })
    });
    add_op(op);
}

/// Takes waiting job `id` out of the queue (and off the lists).
fn take_waiting(id: u64) -> Option<Waiting> {
    let w = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let i = q.iter().position(|w| w.id == id)?;
        q.remove(i)
    })?;
    remove_op(id);
    Some(w)
}

/// Starts the oldest waiting job once nothing is running.
fn start_next() {
    if RUNNING.with(|r| r.get()) > 0 {
        return;
    }
    let next = QUEUE.with(|q| q.borrow_mut().pop_front());
    if let Some(w) = next {
        remove_op(w.id);
        (w.start)();
    }
}

fn bold_label(text: &str) -> gtk::Label {
    let t = gtk::Label::new(Some(text));
    t.set_xalign(0.0);
    t.set_ellipsize(pango::EllipsizeMode::Middle);
    t.set_max_width_chars(60);
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
    t.set_attributes(Some(&attrs));
    t
}

/// `show_now`: list the operation at once (a job leaving the queue) instead
/// of only once it has taken a moment.
fn start(parent: Option<gtk::Window>, job: Job, on_done: Box<dyn Fn(Vec<String>)>, show_now: bool) {
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let op = Rc::new(Op {
        id: next_id(),
        title: title_for(&job),
        state: OpState::Running,
        detail: RefCell::new("Preparing…".into()),
        fraction: std::cell::Cell::new(0.0),
        paused: Arc::new(AtomicBool::new(false)),
        cancel: gio::Cancellable::new(),
        cancelled: std::cell::Cell::new(false),
    });
    {
        let cancel = op.cancel.clone();
        let paused = op.paused.clone();
        std::thread::spawn(move || {
            let mut w = Worker {
                tx: tx.clone(),
                cancel,
                total_bytes: 0,
                done_bytes: 0,
                total_items: 0,
                done_items: 0,
                by_bytes: false,
                file_all: None,
                dir_all: None,
                skip_errors: false,
                skipped: 0,
                last: Instant::now() - Duration::from_secs(1),
                verb: "",
                paused,
                clock: Clock::new(),
            };
            let created = w.run(job);
            let _ = tx.send_blocking(Msg::Done(created));
        });
    }
    RUNNING.with(|r| r.set(r.get() + 1));
    // The progress window isn't an application window, so keep the app (and
    // the worker thread) alive ourselves if every main window gets closed.
    let hold = gio::Application::default().map(|a| a.hold());

    // List the operation only if it takes a while.
    let finished = Rc::new(std::cell::Cell::new(false));
    {
        let (op, finished) = (op.clone(), finished.clone());
        let delay = if show_now { 0 } else { 600 };
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if !finished.get() {
                LISTED.with(|l| l.set(true));
                add_op(op);
            }
        });
    }

    glib::MainContext::default().spawn_local(async move {
        let parent = parent;
        let _hold = hold;
        while let Ok(msg) = rx.recv().await {
            match msg {
                Msg::Progress { detail, fraction } => {
                    *op.detail.borrow_mut() = detail;
                    op.fraction.set(fraction);
                    update_op(&op);
                    changed();
                }
                Msg::Conflict(req, reply) => {
                    let p = dialog_parent(&parent);
                    let res = conflict_dialog(p.as_ref(), &req).await;
                    let _ = reply.send(res);
                }
                Msg::Error { title, detail, offer_delete, reply } => {
                    let p = dialog_parent(&parent);
                    let c = error_dialog(p.as_ref(), &title, &detail, offer_delete).await;
                    let _ = reply.send(c);
                }
                Msg::Done(created) => {
                    RUNNING.with(|r| r.set(r.get().saturating_sub(1)));
                    finished.set(true);
                    // Also hides the window if this was the last operation.
                    remove_op(op.id);
                    on_done(created);
                    start_next();
                    if !is_busy() && LISTED.with(|l| l.replace(false)) {
                        notify_done(&parent);
                    }
                    break;
                }
            }
        }
    });
}

/// Tells the desktop that the file operations are done, as Caja does,
/// unless the window they were started from (or the operations' own
/// window) is the active one: one closed, minimized or on another
/// workspace isn't either.
fn notify_done(parent: &Option<gtk::Window>) {
    let watched = |w: &gtk::Window| w.is_visible() && w.is_active();
    if !parent.as_ref().is_some_and(watched) && !OPS_WINDOW.with(|w| w.borrow().as_ref().is_some_and(watched)) {
        util::notify("Arkha", "Process completed");
    }
}

fn dialog_parent(parent: &Option<gtk::Window>) -> Option<gtk::Window> {
    if let Some(w) = OPS_WINDOW.with(|w| w.borrow().clone()) {
        if w.is_visible() {
            return Some(w);
        }
    }
    // The window that started the job may have been closed since.
    parent.clone().filter(|p| p.is_visible())
}

async fn conflict_dialog(parent: Option<&gtk::Window>, req: &ConflictReq) -> (Resolution, bool) {
    let merge = req.src_is_dir && req.dest_is_dir;
    let primary = if merge {
        format!("Merge folder “{}”?", req.name)
    } else if req.dest_is_dir {
        format!("Replace folder “{}”?", req.name)
    } else {
        format!("Replace file “{}”?", req.name)
    };
    let secondary = if merge {
        format!(
            "A folder with the same name already exists in “{}”.\nMerging will ask before replacing any files in it that conflict.",
            req.dest_dir
        )
    } else {
        format!(
            "Another {} with the same name already exists in “{}”.\nReplacing it will overwrite its content.",
            if req.dest_is_dir { "folder" } else { "file" },
            req.dest_dir
        )
    };
    let d = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        &primary,
    );
    d.set_title("File conflict");
    d.set_secondary_text(Some(&format!(
        "{secondary}\n\nOriginal: {}\nReplace with: {}",
        req.dest_desc, req.src_desc
    )));
    let all = gtk::CheckButton::with_label("Apply this action to all conflicts");
    d.message_area()
        .downcast::<gtk::Box>()
        .unwrap()
        .pack_start(&all, false, false, 6);
    all.show();
    d.add_button("_Cancel", gtk::ResponseType::Cancel);
    d.add_button("_Skip", gtk::ResponseType::Other(1));
    d.add_button("_Keep Both", gtk::ResponseType::Other(2));
    d.add_button(if merge { "_Merge" } else { "_Replace" }, gtk::ResponseType::Other(3));
    d.set_default_response(gtk::ResponseType::Other(3));
    let resp = d.run_future().await;
    let apply_all = all.is_active();
    d.close();
    let r = match resp {
        gtk::ResponseType::Other(1) => Resolution::Skip,
        gtk::ResponseType::Other(2) => Resolution::KeepBoth,
        gtk::ResponseType::Other(3) => Resolution::Replace,
        _ => Resolution::Cancel,
    };
    (r, apply_all)
}

async fn error_dialog(parent: Option<&gtk::Window>, title: &str, detail: &str, offer_delete: bool) -> ErrChoice {
    let d = gtk::MessageDialog::new(
        parent,
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Error,
        gtk::ButtonsType::None,
        title,
    );
    d.set_secondary_text(Some(detail));
    d.add_button("_Cancel", gtk::ResponseType::Cancel);
    d.add_button("S_kip All", gtk::ResponseType::Other(2));
    d.add_button("_Skip", gtk::ResponseType::Other(1));
    if offer_delete {
        d.add_button("_Delete Permanently", gtk::ResponseType::Other(4));
    } else {
        d.add_button("_Retry", gtk::ResponseType::Other(3));
    }
    let resp = d.run_future().await;
    d.close();
    match resp {
        gtk::ResponseType::Other(1) => ErrChoice::Skip,
        gtk::ResponseType::Other(2) => ErrChoice::SkipAll,
        gtk::ResponseType::Other(3) => ErrChoice::Retry,
        gtk::ResponseType::Other(4) => ErrChoice::DeletePermanently,
        _ => ErrChoice::Cancel,
    }
}

fn is_mtp(f: &gio::File) -> bool {
    f.uri_scheme().as_deref() == Some("mtp")
}

/// The process's umask, read without changing it (umask(2) can only be
/// read by setting it, which races with other threads creating files).
fn umask() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Umask:").map(|v| v.trim().to_string())))
        .and_then(|v| u32::from_str_radix(&v, 8).ok())
        .unwrap_or(0o022)
}

/// Whether both are on the same phone (or camera) over MTP.
fn same_mtp_device(a: &gio::File, b: &gio::File) -> bool {
    let device = |f: &gio::File| {
        let uri = f.uri();
        uri.strip_prefix("mtp://").and_then(|r| r.split('/').next()).map(str::to_string)
    };
    device(a).is_some() && device(a) == device(b)
}

/// How much `copy_in_pieces` copies at a time: on a network share that
/// copies on the server, a fraction of a second.
const PIECE: usize = 32 * 1024 * 1024;

/// Copies the file `src` to `dst` with copy_file_range, a piece at a time,
/// reporting progress like GFile's copy does; None if that can't be used
/// here (GIO copies then). File systems that copy on the server (SMB, NFS)
/// do all of a file in one call otherwise, as GIO asks: nothing would show
/// its progress, and it couldn't be paused or cancelled until it's done.
/// Pausing happens in `progress`. With `overwrite` an existing `dst` is
/// replaced, otherwise that fails with G_IO_ERROR_EXISTS. A copy that
/// fails or is cancelled is removed.
fn copy_in_pieces(src: &std::path::Path, dst: &std::path::Path, overwrite: bool, cancel: &gio::Cancellable, progress: &mut dyn FnMut(i64, i64)) -> Option<Result<(), glib::Error>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::io::AsRawFd;
    let io_error = |e: &std::io::Error| {
        let code = unsafe { gio::ffi::g_io_error_from_errno(e.raw_os_error().unwrap_or(libc::EIO)) };
        let kind = unsafe { <gio::IOErrorEnum as glib::translate::FromGlib<_>>::from_glib(code) };
        glib::Error::new(kind, &e.to_string())
    };
    let input = std::fs::File::open(src).ok()?;
    let meta = input.metadata().ok()?;
    // The kernel copies only within one kind of file system: don't make
    // (and remove again) a file on a network share just to find that out.
    let parent = std::ffi::CString::new(dst.parent()?.as_os_str().as_encoded_bytes()).ok()?;
    let (mut a, mut b) = unsafe { (std::mem::zeroed::<libc::statfs>(), std::mem::zeroed::<libc::statfs>()) };
    if unsafe { libc::fstatfs(input.as_raw_fd(), &mut a) != 0 || libc::statfs(parent.as_ptr(), &mut b) != 0 } || a.f_type != b.f_type {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).mode(meta.mode() & 0o7777);
    if overwrite {
        // Cut to the new length at the end: it's left as it was if this
        // can't be used here.
        options.create(true);
    } else {
        options.create_new(true);
    }
    let output = match options.open(dst) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Some(Err(glib::Error::new(gio::IOErrorEnum::Exists, &e.to_string())));
        }
        Err(e) => return Some(Err(io_error(&e))),
    };
    let total = meta.len() as i64;
    let (mut off_in, mut off_out) = (0i64, 0i64);
    // Closed first: SMB shares refuse to delete a file that's still open
    // (EIO), and keep its name taken until it's closed.
    let fail = |output: std::fs::File, r: Result<(), glib::Error>| {
        drop(output);
        let _ = std::fs::remove_file(dst);
        Some(r)
    };
    loop {
        let n = unsafe { libc::copy_file_range(input.as_raw_fd(), &mut off_in, output.as_raw_fd(), &mut off_out, PIECE, 0) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            let unusable = matches!(e.raw_os_error(), Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP | libc::EBADF));
            if unusable && off_out == 0 {
                // Not here: GIO copies it another way.
                drop(output);
                if !overwrite {
                    let _ = std::fs::remove_file(dst);
                }
                return None;
            }
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return fail(output, Err(io_error(&e)));
        }
        if n == 0 {
            break;
        }
        progress(off_out, total);
        if cancel.is_cancelled() {
            return fail(output, Err(glib::Error::new(gio::IOErrorEnum::Cancelled, "Cancelled")));
        }
    }
    if let Err(e) = output.set_len(off_out as u64) {
        return fail(output, Err(io_error(&e)));
    }
    Some(Ok(()))
}

/// Copies `src`'s contents to `dst` by reading and writing them, reporting
/// progress like GFile's copy does. With `overwrite` an existing `dst` is
/// replaced, otherwise that fails with G_IO_ERROR_EXISTS.
fn stream_copy(src: &gio::File, dst: &gio::File, overwrite: bool, cancel: &gio::Cancellable, progress: &mut dyn FnMut(i64, i64)) -> Result<(), glib::Error> {
    let total = src
        .query_info("standard::size", gio::FileQueryInfoFlags::NONE, Some(cancel))
        .map(|i| i.size())
        .unwrap_or(0);
    let input = src.read(Some(cancel))?;
    let output: gio::OutputStream = if overwrite {
        dst.replace(None, false, gio::FileCreateFlags::NONE, Some(cancel))?.upcast()
    } else {
        dst.create(gio::FileCreateFlags::NONE, Some(cancel))?.upcast()
    };
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0i64;
    loop {
        let n = input.read(&mut buf[..], Some(cancel))?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n], Some(cancel))?;
        done += n as i64;
        progress(done, total);
    }
    output.close(Some(cancel))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Runs `job` on a worker whose conflict dialogs are answered from
    /// `answers` in order (with "apply to all" off) and whose error dialogs
    /// all answer Skip. Returns the created URIs and the questions asked.
    fn run_job(job: Job, answers: &[Resolution]) -> (Vec<String>, Vec<String>) {
        run_job_paused(job, answers, Arc::new(AtomicBool::new(false)))
    }

    fn run_job_paused(job: Job, answers: &[Resolution], paused: Arc<AtomicBool>) -> (Vec<String>, Vec<String>) {
        run_job_with(job, answers, paused, gio::Cancellable::new())
    }

    fn run_job_with(job: Job, answers: &[Resolution], paused: Arc<AtomicBool>, cancel: gio::Cancellable) -> (Vec<String>, Vec<String>) {
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let mut w = Worker {
            tx,
            cancel,
            total_bytes: 0,
            done_bytes: 0,
            total_items: 0,
            done_items: 0,
            by_bytes: false,
            file_all: None,
            dir_all: None,
            skip_errors: false,
            skipped: 0,
            last: Instant::now(),
            verb: "",
            paused,
            clock: Clock::new(),
        };
        let answers = answers.to_vec();
        let ui = std::thread::spawn(move || {
            let mut asked = vec![];
            let mut answers = answers.into_iter();
            while let Ok(m) = rx.recv_blocking() {
                match m {
                    Msg::Conflict(req, reply) => {
                        asked.push(format!("conflict:{}", req.name));
                        let _ = reply.send((answers.next().expect("unexpected conflict"), false));
                    }
                    Msg::Error { title, reply, .. } => {
                        asked.push(format!("error:{title}"));
                        let _ = reply.send(ErrChoice::Skip);
                    }
                    _ => {}
                }
            }
            asked
        });
        let created = w.run(job);
        drop(w);
        (created, ui.join().unwrap())
    }

    fn home_scratch(name: &str) -> TestDir {
        TestDir::new(name)
    }
    /// On another file system than target/, for moves across them.
    fn scratch(base: &str, name: &str) -> TestDir {
        TestDir::in_dir(Path::new(base), name)
    }
    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }
    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }
    fn uri(p: &Path) -> String {
        gio::File::for_path(p).uri().to_string()
    }
    fn transfer(kind: Kind, src: &Path, dest_dir: &Path) -> Job {
        Job::Transfer {
            kind,
            items: vec![Item { src: uri(src), dest_dir: uri(dest_dir), name: None }],
        }
    }

    /// src/d/{a,b,sub/c} merged into dst/d/{a,x}: a is replaced, the rest moves in.
    fn merge_setup(root: &Path, dst_root: &Path) -> (PathBuf, PathBuf) {
        let src = root.join("src/d");
        write(&src.join("a"), "new a");
        write(&src.join("b"), "b");
        write(&src.join("sub/c"), "c");
        let dst = dst_root.join("dst");
        write(&dst.join("d/a"), "old a");
        write(&dst.join("d/x"), "x");
        (src, dst)
    }

    fn check_merged(dst: &Path) {
        assert_eq!(read(&dst.join("d/a")), "new a");
        assert_eq!(read(&dst.join("d/b")), "b");
        assert_eq!(read(&dst.join("d/sub/c")), "c");
        assert_eq!(read(&dst.join("d/x")), "x");
    }

    #[test]
    fn move_merge_same_fs() {
        let s = home_scratch("merge");
        let (src, dst) = merge_setup(&s.0, &s.0);
        let (created, asked) = run_job(transfer(Kind::Move, &src, &dst), &[Resolution::Replace, Resolution::Replace]);
        assert_eq!(asked, ["conflict:d", "conflict:a"]);
        check_merged(&dst);
        assert!(!src.exists(), "source folder should be gone after a full merge");
        assert_eq!(created, [uri(&dst.join("d"))]);
    }

    #[test]
    fn move_merge_keeps_skipped() {
        let s = home_scratch("mergeskip");
        let (src, dst) = merge_setup(&s.0, &s.0);
        let (_, asked) = run_job(transfer(Kind::Move, &src, &dst), &[Resolution::Replace, Resolution::Skip]);
        assert_eq!(asked, ["conflict:d", "conflict:a"]);
        assert_eq!(read(&dst.join("d/a")), "old a");
        assert_eq!(read(&dst.join("d/b")), "b");
        assert_eq!(read(&src.join("a")), "new a", "skipped file stays in the source");
        assert!(!src.join("b").exists());
    }

    #[test]
    fn move_merge_cross_fs() {
        let s = home_scratch("xmerge");
        let t = scratch("/dev/shm", "xmerge");
        let (src, dst) = merge_setup(&t.0, &s.0);
        let (_, asked) = run_job(transfer(Kind::Move, &src, &dst), &[Resolution::Replace, Resolution::Replace]);
        assert_eq!(asked, ["conflict:d", "conflict:a"]);
        check_merged(&dst);
        assert!(!src.exists());
    }

    #[test]
    fn move_cross_fs_plain() {
        let s = home_scratch("xmove");
        let t = scratch("/dev/shm", "xmove");
        let src = t.0.join("d");
        write(&src.join("sub/c"), "c");
        let (created, asked) = run_job(transfer(Kind::Move, &src, &s.0), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(read(&s.0.join("d/sub/c")), "c");
        assert!(!src.exists());
        assert_eq!(created, [uri(&s.0.join("d"))]);
    }

    #[test]
    fn move_folder_over_file_cross_fs() {
        let s = home_scratch("xdiroverfile");
        let t = scratch("/dev/shm", "xdiroverfile");
        write(&t.0.join("n/inner"), "i");
        write(&s.0.join("n"), "file");
        let (_, asked) = run_job(transfer(Kind::Move, &t.0.join("n"), &s.0), &[Resolution::Replace]);
        assert_eq!(asked, ["conflict:n"]);
        assert_eq!(read(&s.0.join("n/inner")), "i");
        assert!(!t.0.join("n").exists());
    }

    #[test]
    fn move_file_over_folder() {
        let s = home_scratch("fileoverdir");
        write(&s.0.join("src/n"), "file");
        write(&s.0.join("dst/n/inner"), "i");
        let (_, asked) = run_job(transfer(Kind::Move, &s.0.join("src/n"), &s.0.join("dst")), &[Resolution::Replace]);
        assert_eq!(asked, ["conflict:n"]);
        assert_eq!(read(&s.0.join("dst/n")), "file");
        assert!(!s.0.join("src/n").exists());
    }

    #[test]
    fn move_folder_over_file() {
        let s = home_scratch("diroverfile");
        write(&s.0.join("src/n/inner"), "i");
        write(&s.0.join("dst/n"), "file");
        let (_, asked) = run_job(transfer(Kind::Move, &s.0.join("src/n"), &s.0.join("dst")), &[Resolution::Replace]);
        assert_eq!(asked, ["conflict:n"]);
        assert_eq!(read(&s.0.join("dst/n/inner")), "i");
        assert!(!s.0.join("src/n").exists());
    }

    #[test]
    fn move_keep_both() {
        let s = home_scratch("keepboth");
        write(&s.0.join("src/a.txt"), "new");
        write(&s.0.join("dst/a.txt"), "old");
        let (created, _) = run_job(transfer(Kind::Move, &s.0.join("src/a.txt"), &s.0.join("dst")), &[Resolution::KeepBoth]);
        assert_eq!(read(&s.0.join("dst/a.txt")), "old");
        assert_eq!(read(&s.0.join("dst/a (2).txt")), "new");
        assert_eq!(created, [uri(&s.0.join("dst/a (2).txt"))]);
    }

    #[test]
    fn copy_merge() {
        let s = home_scratch("copymerge");
        let (src, dst) = merge_setup(&s.0, &s.0);
        let (_, asked) = run_job(transfer(Kind::Copy, &src, &dst), &[Resolution::Replace, Resolution::Replace]);
        assert_eq!(asked, ["conflict:d", "conflict:a"]);
        check_merged(&dst);
        assert_eq!(read(&src.join("a")), "new a", "copy leaves the source alone");
    }

    #[test]
    fn pause_holds_the_job_until_resumed() {
        let s = home_scratch("pause");
        write(&s.0.join("src/a.txt"), "a");
        write(&s.0.join("src/b.txt"), "b");
        fs::create_dir_all(s.0.join("dst")).unwrap();
        let paused = Arc::new(AtomicBool::new(true));
        let job = Job::Transfer {
            kind: Kind::Copy,
            items: ["a.txt", "b.txt"].iter().map(|n| Item { src: uri(&s.0.join("src").join(n)), dest_dir: uri(&s.0.join("dst")), name: None }).collect(),
        };
        let p = paused.clone();
        let worker = std::thread::spawn(move || run_job_paused(job, &[], p));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!worker.is_finished(), "a paused job must not finish");
        assert!(!s.0.join("dst/b.txt").exists(), "nothing past the pause point may be copied");
        paused.store(false, Ordering::Relaxed);
        let (_, asked) = worker.join().unwrap();
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(read(&s.0.join("dst/a.txt")), "a");
        assert_eq!(read(&s.0.join("dst/b.txt")), "b");
    }

    #[test]
    fn move_into_itself_refused() {
        let s = home_scratch("intoself");
        write(&s.0.join("d/f"), "f");
        let (_, asked) = run_job(transfer(Kind::Move, &s.0.join("d"), &s.0.join("d")), &[]);
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("error:Cannot move"), "{asked:?}");
        assert_eq!(read(&s.0.join("d/f")), "f");
    }

    #[test]
    fn delete_leaves_what_links_point_to() {
        let s = home_scratch("deletelinks");
        write(&s.0.join("d/sub/f"), "f");
        write(&s.0.join("outside/keep.txt"), "keep");
        std::os::unix::fs::symlink(s.0.join("outside"), s.0.join("d/sub/dir-link")).unwrap();
        std::os::unix::fs::symlink(s.0.join("outside/keep.txt"), s.0.join("d/file-link")).unwrap();
        let (_, asked) = run_job(Job::Delete(vec![uri(&s.0.join("d"))]), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert!(!s.0.join("d").exists());
        assert_eq!(read(&s.0.join("outside/keep.txt")), "keep");
    }

    #[test]
    fn copies_into_the_same_folder_get_new_names() {
        let s = home_scratch("samefolder");
        write(&s.0.join("a.txt"), "a");
        write(&s.0.join("d/f"), "f");
        for _ in 0..2 {
            let (_, asked) = run_job(transfer(Kind::Copy, &s.0.join("a.txt"), &s.0), &[]);
            assert!(asked.is_empty(), "{asked:?}");
        }
        let (created, _) = run_job(transfer(Kind::Copy, &s.0.join("d"), &s.0), &[]);
        assert_eq!(read(&s.0.join("a (copy).txt")), "a");
        assert_eq!(read(&s.0.join("a (copy 2).txt")), "a");
        assert_eq!(read(&s.0.join("d (copy)/f")), "f");
        assert_eq!(created, [uri(&s.0.join("d (copy)"))]);
        // Moving something to where it is changes nothing.
        let (_, asked) = run_job(transfer(Kind::Move, &s.0.join("a.txt"), &s.0), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(read(&s.0.join("a.txt")), "a");
    }

    #[test]
    fn copied_links_stay_links() {
        let s = home_scratch("copylinks");
        write(&s.0.join("src/target.txt"), "t");
        std::os::unix::fs::symlink("target.txt", s.0.join("src/link")).unwrap();
        std::os::unix::fs::symlink("/no/such/place", s.0.join("src/dangling")).unwrap();
        fs::create_dir_all(s.0.join("dst")).unwrap();
        let (_, asked) = run_job(transfer(Kind::Copy, &s.0.join("src"), &s.0.join("dst")), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(fs::read_link(s.0.join("dst/src/link")).unwrap(), Path::new("target.txt"));
        assert_eq!(fs::read_link(s.0.join("dst/src/dangling")).unwrap(), Path::new("/no/such/place"));
        assert_eq!(read(&s.0.join("dst/src/link")), "t");
    }

    #[test]
    fn unreadable_files_are_skipped_without_a_partial_copy() {
        use std::os::unix::fs::PermissionsExt;
        // Root reads everything: nothing to test then.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let s = home_scratch("unreadable");
        write(&s.0.join("src/d/ok"), "ok");
        write(&s.0.join("src/d/secret"), "secret");
        fs::set_permissions(s.0.join("src/d/secret"), fs::Permissions::from_mode(0o000)).unwrap();
        fs::create_dir_all(s.0.join("dst")).unwrap();
        let (_, asked) = run_job(transfer(Kind::Copy, &s.0.join("src/d"), &s.0.join("dst")), &[]);
        assert_eq!(asked, ["error:Error while copying “secret”"]);
        assert_eq!(read(&s.0.join("dst/d/ok")), "ok");
        assert!(!s.0.join("dst/d/secret").exists(), "a skipped file leaves nothing behind");
        fs::set_permissions(s.0.join("src/d/secret"), fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn links_point_to_the_original() {
        let s = home_scratch("link");
        write(&s.0.join("a.txt"), "a");
        fs::create_dir_all(s.0.join("dst")).unwrap();
        run_job(transfer(Kind::Link, &s.0.join("a.txt"), &s.0.join("dst")), &[]);
        assert_eq!(fs::read_link(s.0.join("dst/a.txt")).unwrap(), s.0.join("a.txt"));
        // In the same folder, as Caja names them.
        let (created, _) = run_job(transfer(Kind::Link, &s.0.join("a.txt"), &s.0), &[]);
        assert_eq!(fs::read_link(s.0.join("Link to a.txt")).unwrap(), s.0.join("a.txt"));
        assert_eq!(created, [uri(&s.0.join("Link to a.txt"))]);
    }

    #[test]
    fn keep_both_for_a_folder() {
        let s = home_scratch("keepbothdir");
        write(&s.0.join("src/d/new"), "new");
        write(&s.0.join("dst/d/old"), "old");
        let (created, asked) = run_job(transfer(Kind::Copy, &s.0.join("src/d"), &s.0.join("dst")), &[Resolution::KeepBoth]);
        assert_eq!(asked, ["conflict:d"]);
        assert_eq!(read(&s.0.join("dst/d/old")), "old");
        assert!(!s.0.join("dst/d/new").exists());
        assert_eq!(read(&s.0.join("dst/d (2)/new")), "new");
        assert_eq!(created, [uri(&s.0.join("dst/d (2)"))]);
    }

    #[test]
    fn cancel_stops_the_remaining_items() {
        let s = home_scratch("cancel");
        for n in ["a", "b"] {
            write(&s.0.join("src").join(n), "new");
        }
        write(&s.0.join("dst/a"), "old");
        let job = Job::Transfer {
            kind: Kind::Move,
            items: ["a", "b"].iter().map(|n| Item { src: uri(&s.0.join("src").join(n)), dest_dir: uri(&s.0.join("dst")), name: None }).collect(),
        };
        let (_, asked) = run_job(job, &[Resolution::Cancel]);
        assert_eq!(asked, ["conflict:a"]);
        assert_eq!(read(&s.0.join("dst/a")), "old");
        assert!(!s.0.join("dst/b").exists());
        assert_eq!(read(&s.0.join("src/b")), "new");
    }

    /// A tree of `files` files (sizes up to 64 KiB, some empty) in
    /// folders up to four deep, with links to files and folders in it.
    fn make_tree(root: &Path, files: usize, seed: u64) {
        let mut r = crate::testutil::Rng::new(seed);
        fs::create_dir_all(root).unwrap();
        let mut dirs = vec![root.to_path_buf()];
        for i in 0..files {
            if r.chance(12) && dirs.len() < files / 10 + 2 {
                let parent = r.pick(&dirs).clone();
                if parent.components().count() - root.components().count() < 4 {
                    let d = parent.join(format!("dir {i}"));
                    fs::create_dir_all(&d).unwrap();
                    dirs.push(d);
                }
            }
            let d = r.pick(&dirs).clone();
            let size = if r.chance(10) { 0 } else { r.below(64 * 1024) };
            let data: Vec<u8> = (0..size).map(|j| ((i * 31 + j) % 251) as u8).collect();
            fs::write(d.join(format!("file {i}.bin")), data).unwrap();
        }
        std::os::unix::fs::symlink("file 0.bin", root.join("link to a file")).unwrap();
        std::os::unix::fs::symlink(".", root.join("link to here")).unwrap();
    }

    /// Every entry under `root`, relative, with its content (a link's target).
    fn tree_contents(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(root).unwrap().to_path_buf();
                let meta = fs::symlink_metadata(&p).unwrap();
                if meta.file_type().is_symlink() {
                    out.insert(rel, fs::read_link(&p).unwrap().into_os_string().into_encoded_bytes());
                } else if meta.is_dir() {
                    out.insert(rel, b"<dir>".to_vec());
                    stack.push(p);
                } else {
                    out.insert(rel, fs::read(&p).unwrap());
                }
            }
        }
        out
    }

    fn files_count() -> usize {
        std::env::var("ARKHA_STRESS_FILES").ok().and_then(|v| v.parse().ok()).unwrap_or(3000)
    }

    /// cargo test -- --ignored (ARKHA_STRESS_FILES=n for another size).
    #[test]
    #[ignore = "stress test: run with --ignored"]
    fn stress_big_tree_copy_move_delete() {
        let s = home_scratch("stress-tree");
        let src = s.0.join("src/tree");
        make_tree(&src, files_count(), 1);
        let want = tree_contents(&src);
        // Copy, here.
        fs::create_dir_all(s.0.join("copy")).unwrap();
        let (_, asked) = run_job(transfer(Kind::Copy, &src, &s.0.join("copy")), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert!(tree_contents(&s.0.join("copy/tree")) == want, "the copy differs");
        // Move to another file system (RAM: a small tree only) and back.
        let shm = scratch("/dev/shm", "stress-tree");
        let small = s.0.join("small/tree");
        make_tree(&small, 300, 2);
        let small_want = tree_contents(&small);
        run_job(transfer(Kind::Move, &small, &shm.0), &[]);
        assert!(!small.exists() && tree_contents(&shm.0.join("tree")) == small_want, "moving away");
        run_job(transfer(Kind::Move, &shm.0.join("tree"), &s.0.join("small")), &[]);
        assert!(!shm.0.join("tree").exists() && tree_contents(&small) == small_want, "moving back");
        // Move here (a rename), then delete it all.
        run_job(transfer(Kind::Move, &s.0.join("copy/tree"), &s.0), &[]);
        assert!(tree_contents(&s.0.join("tree")) == want);
        let (_, asked) = run_job(Job::Delete(vec![uri(&s.0.join("tree")), uri(&src)]), &[]);
        assert!(asked.is_empty(), "{asked:?}");
        assert!(!s.0.join("tree").exists() && !src.exists());
    }

    /// Copies and moves cancelled at random moments: what got copied is
    /// whole, nothing is lost, and nothing half-written is left behind.
    #[test]
    #[ignore = "stress test: run with --ignored"]
    fn stress_cancel_anywhere() {
        let s = home_scratch("stress-cancel");
        let src = s.0.join("src/tree");
        make_tree(&src, files_count() / 3, 3);
        let want = tree_contents(&src);
        let mut r = crate::testutil::Rng::new(4);
        for round in 0..12 {
            let dst = s.0.join(format!("dst{round}"));
            fs::create_dir_all(&dst).unwrap();
            let moving = round % 3 == 2;
            // Moving within a file system is a rename: across one it copies.
            let shm = scratch("/dev/shm", &format!("stress-cancel-{round}"));
            let (from, to) = if moving {
                let from = shm.0.join("tree");
                make_tree(&from, 400, 10 + round as u64);
                (from, dst.clone())
            } else {
                (src.clone(), dst.clone())
            };
            let before = tree_contents(&from);
            let cancel = gio::Cancellable::new();
            let c = cancel.clone();
            let delay = r.below(400) as u64;
            let canceller = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(delay));
                c.cancel();
            });
            let job = transfer(if moving { Kind::Move } else { Kind::Copy }, &from, &to);
            let (_, asked) = run_job_with(job, &[], Arc::new(AtomicBool::new(false)), cancel);
            canceller.join().unwrap();
            assert!(asked.is_empty(), "round {round}: {asked:?}");
            let got = if to.join("tree").exists() { tree_contents(&to.join("tree")) } else { Default::default() };
            let left = if from.exists() { tree_contents(&from) } else { Default::default() };
            for (rel, data) in &got {
                // A folder may be there half full; a file only whole.
                let orig = before.get(rel).unwrap_or_else(|| panic!("round {round}: {rel:?} came from nowhere"));
                assert!(data == orig || data == b"<dir>", "round {round}: {rel:?} is cut off ({} of {} bytes)", data.len(), orig.len());
            }
            for (rel, data) in &before {
                if data != b"<dir>" {
                    assert!(got.contains_key(rel) || left.get(rel) == Some(data), "round {round}: {rel:?} was lost");
                }
            }
            if !moving {
                assert!(tree_contents(&src) == want, "round {round}: the source changed");
            }
        }
    }

    #[test]
    fn time_left_as_caja_words_it() {
        assert_eq!(format_time_left(1), "1 second");
        assert_eq!(format_time_left(59), "59 seconds");
        assert_eq!(format_time_left(60), "1 minute");
        assert_eq!(format_time_left(3599), "59 minutes");
        assert_eq!(format_time_left(3600 + 60), "1 hour, 1 minute");
        assert_eq!(format_time_left(3 * 3600 + 20 * 60), "3 hours, 20 minutes");
        assert_eq!(format_time_left(5 * 3600 + 59), "approximately 5 hours");
    }

    #[test]
    fn time_left_once_the_rate_is_steady() {
        let (s, f) = (Duration::from_secs, util::format_size);
        // Not before 15 seconds.
        assert_eq!(bytes_detail(1_000_000, 4_000_000, s(14)), format!("{} of {}", f(1_000_000), f(4_000_000)));
        // 1 MB in 20 s: the other 3 MB take a minute.
        assert_eq!(bytes_detail(1_000_000, 4_000_000, s(20)), format!("{} of {} \u{2014} 1 minute left ({}/sec)", f(1_000_000), f(4_000_000), f(50_000)));
        assert_eq!(items_detail(10, 40, s(30)), "10 of 40 items \u{2014} 1 minute left");
        assert_eq!(items_detail(0, 40, s(30)), "0 of 40 items");
    }

    #[test]
    fn copy_in_pieces_like_gio() {
        let s = home_scratch("pieces");
        let src = s.0.join("src.bin");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let none = gio::Cancellable::new();
        let mut seen = vec![];
        let dst = s.0.join("dst.bin");
        assert!(copy_in_pieces(&src, &dst, false, &none, &mut |done, total| seen.push((done, total))).unwrap().is_ok());
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert_eq!(seen.last(), Some(&(300_000, 300_000)));
        // An existing file isn't replaced unless asked.
        let r = copy_in_pieces(&src, &dst, false, &none, &mut |_, _| {}).unwrap();
        assert!(r.unwrap_err().matches(gio::IOErrorEnum::Exists));
        // Replacing a longer file leaves just the copy.
        fs::write(&dst, vec![7u8; 500_000]).unwrap();
        assert!(copy_in_pieces(&src, &dst, true, &none, &mut |_, _| {}).unwrap().is_ok());
        assert_eq!(fs::read(&dst).unwrap(), data);
        // A cancelled copy leaves nothing behind.
        let cancelled = gio::Cancellable::new();
        cancelled.cancel();
        let gone = s.0.join("gone.bin");
        let r = copy_in_pieces(&src, &gone, false, &cancelled, &mut |_, _| {}).unwrap();
        assert!(r.unwrap_err().matches(gio::IOErrorEnum::Cancelled));
        assert!(!gone.exists());
    }
}
