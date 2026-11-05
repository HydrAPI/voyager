//! Icon lookup with caching, and a background thumbnailer that writes to the
//! shared freedesktop thumbnail cache (~/.cache/thumbnails), same as Caja.
//! Images are thumbnailed built in; other types by custom thumbnailers.

use crate::thumbnailers::{self, Thumbnailer};

use gdk_pixbuf::Pixbuf;
use gio::prelude::*;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

thread_local! {
    /// Theme icons by their GIcon's text and size.
    static ICONS: RefCell<HashMap<String, HashMap<i32, Option<Pixbuf>>>> = RefCell::new(HashMap::new());
    /// Loaded thumbnails by path and size.
    static THUMBS: RefCell<HashMap<String, HashMap<i32, Option<Pixbuf>>>> = RefCell::new(HashMap::new());
    /// Thumbnails with their shadow (see framed_thumb), and their sizes.
    static FRAMED: RefCell<HashMap<(String, i32), Pixbuf>> = RefCell::new(HashMap::new());
    static DIMS: RefCell<HashMap<String, Option<ThumbHeader>>> = RefCell::new(HashMap::new());
    /// Thumbnails being loaded, and the loaders' queue (see thumb_pixbuf).
    static LOADING: RefCell<HashSet<(String, i32)>> = RefCell::new(HashSet::new());
    static LOAD_QUEUE: RefCell<Option<Arc<LoadQueue>>> = const { RefCell::new(None) };
    /// Views being drawn (see set_drawing).
    static DRAWING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// (uri, mtime) pairs already sent to the worker; an edited file gets a new entry.
    static QUEUED: RefCell<HashSet<(String, i64)>> = RefCell::new(HashSet::new());
    static MAKERS: RefCell<Option<Arc<JobQueue>>> = const { RefCell::new(None) };
    /// The custom thumbnailer for each content type asked about.
    /// Shared, not copied: asked about for every file of a folder.
    static CUSTOM_FOR: RefCell<HashMap<String, Option<Arc<Thumbnailer>>>> = RefCell::new(HashMap::new());
    static LISTENERS: RefCell<Vec<(usize, Listener)>> = RefCell::new(Vec::new());
    static NEXT_LISTENER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The custom thumbnailers whose programs are installed, loaded on
    /// first use (see reload_thumbnailers).
    static CUSTOM: RefCell<Option<Vec<Thumbnailer>>> = const { RefCell::new(None) };
}

fn custom_for(ctype: &str) -> Option<Arc<Thumbnailer>> {
    if let Some(t) = CUSTOM_FOR.with(|c| c.borrow().get(ctype).cloned()) {
        return t;
    }
    let t = CUSTOM.with(|c| {
        let mut c = c.borrow_mut();
        let list = c.get_or_insert_with(|| thumbnailers::load().into_iter().filter(thumbnailers::installed).collect());
        thumbnailers::find(list, ctype).map(Arc::new)
    });
    CUSTOM_FOR.with(|c| c.borrow_mut().insert(ctype.to_string(), t.clone()));
    t
}

/// Re-reads the custom thumbnailers and lets files be tried again.
pub fn reload_thumbnailers() {
    CUSTOM.with(|c| *c.borrow_mut() = None);
    CUSTOM_FOR.with(|c| c.borrow_mut().clear());
    QUEUED.with(|q| q.borrow_mut().clear());
    // Files a thumbnailer failed on may work with the new ones.
    let _ = std::fs::remove_dir_all(fail_dir());
}

#[allow(dead_code)]
pub fn clear_cache() {
    ICONS.with(|c| c.borrow_mut().clear());
}

/// A GIcon's text, which names it in the cache. Making it takes long for
/// what views do for every item each time they measure them, so it's
/// kept on the icon.
fn icon_key(icon: &gio::Icon) -> String {
    let quark = glib::Quark::from_str("arkha-icon-key");
    unsafe {
        if let Some(k) = icon.qdata::<String>(quark) {
            return k.as_ref().clone();
        }
        let k = IconExt::to_string(icon).map(|s| s.to_string()).unwrap_or_else(|| "text-x-generic".into());
        icon.set_qdata(quark, k.clone());
        k
    }
}

pub fn icon_pixbuf(icon: Option<&gio::Icon>, size: i32) -> Option<Pixbuf> {
    let key = icon.map_or_else(|| "text-x-generic".to_string(), icon_key);
    if let Some(p) = ICONS.with(|c| c.borrow().get(&key).and_then(|s| s.get(&size).cloned())) {
        return p;
    }
    let theme = gtk::IconTheme::default()?;
    let flags = gtk::IconLookupFlags::FORCE_SIZE;
    let pb = icon
        .and_then(|i| theme.lookup_by_gicon(i, size, flags))
        .and_then(|info| info.load_icon().ok())
        .or_else(|| {
            theme
                .load_icon("text-x-generic", size, flags)
                .ok()
                .flatten()
        })
        .or_else(|| {
            theme
                .load_icon("application-x-generic", size, flags)
                .ok()
                .flatten()
        });
    ICONS.with(|c| c.borrow_mut().entry(key).or_default().insert(size, pb.clone()));
    pb
}

#[allow(dead_code)]
pub fn named_pixbuf(name: &str, size: i32) -> Option<Pixbuf> {
    let icon: gio::Icon = gio::ThemedIcon::new(name).upcast();
    icon_pixbuf(Some(&icon), size)
}

/// Load a thumbnail file, scaled so its longest side is `size`.
/// A thumbnail file's picture, scaled so its longest side is `size`, if
/// it's loaded. Decoding takes milliseconds per picture (seconds for a big
/// folder), so it's done in the background: while a view is being drawn,
/// a missing one is queued, and the windows are redrawn once it's loaded.
/// Views measure all their items, drawn or not, so that's what keeps
/// pictures nobody sees from being loaded.
pub fn thumb_pixbuf(path: &str, size: i32) -> Option<Pixbuf> {
    if let Some(p) = THUMBS.with(|c| c.borrow().get(path).and_then(|s| s.get(&size).cloned())) {
        return p;
    }
    if DRAWING.with(|d| d.get()) > 0 {
        load_thumb(path, size);
    }
    None
}

/// Views call this before (true) and after (false) drawing their items.
/// Whether a view is being drawn (see set_drawing).
pub fn drawing() -> bool {
    DRAWING.with(|d| d.get()) > 0
}

pub fn set_drawing(on: bool) {
    DRAWING.with(|d| d.set(if on { d.get() + 1 } else { d.get().saturating_sub(1) }));
}

/// Pictures to load, the latest first: those are the ones shown now.
struct LoadQueue {
    jobs: Mutex<Vec<(String, i32)>>,
    ready: Condvar,
}

fn load_thumb(path: &str, size: i32) {
    let key = (path.to_string(), size);
    if !LOADING.with(|l| l.borrow_mut().insert(key.clone())) {
        return;
    }
    let q = LOAD_QUEUE.with(|q| q.borrow_mut().get_or_insert_with(start_loaders).clone());
    q.jobs.lock().unwrap().push(key);
    q.ready.notify_one();
}

fn start_loaders() -> Arc<LoadQueue> {
    let q = Arc::new(LoadQueue { jobs: Mutex::new(vec![]), ready: Condvar::new() });
    // A Pixbuf can't be sent between threads, its pixels can.
    type Pixels = (glib::Bytes, bool, i32, i32, i32);
    let (tx, rx) = async_channel::unbounded::<(String, i32, Option<Pixels>)>();
    for _ in 0..4 {
        let q = q.clone();
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            let (path, size) = {
                let mut jobs = q.jobs.lock().unwrap();
                loop {
                    if let Some(j) = jobs.pop() {
                        break j;
                    }
                    jobs = q.ready.wait(jobs).unwrap();
                }
            };
            let pixels = Pixbuf::from_file_at_scale(&path, size, size, true)
                .ok()
                .map(|p| (p.read_pixel_bytes(), p.has_alpha(), p.width(), p.height(), p.rowstride()));
            if tx.send_blocking((path, size, pixels)).is_err() {
                return;
            }
        });
    }
    glib::MainContext::default().spawn_local(async move {
        while let Ok(first) = rx.recv().await {
            // Everything loaded meanwhile, then one redraw.
            let mut done = vec![first];
            while let Ok(more) = rx.try_recv() {
                done.push(more);
            }
            THUMBS.with(|c| {
                let mut c = c.borrow_mut();
                if c.len() > 4000 {
                    c.clear();
                }
                LOADING.with(|l| {
                    let mut l = l.borrow_mut();
                    for (path, size, pixels) in done {
                        l.remove(&(path.clone(), size));
                        let pb = pixels.map(|(bytes, alpha, w, h, stride)| {
                            Pixbuf::from_bytes(&bytes, gdk_pixbuf::Colorspace::Rgb, alpha, 8, w, h, stride)
                        });
                        c.entry(path).or_default().insert(size, pb);
                    }
                });
            });
            for w in gtk::Window::list_toplevels() {
                w.queue_draw();
            }
        }
    });
    q
}

pub fn forget_thumb(path: &str) {
    THUMBS.with(|c| c.borrow_mut().remove(path));
    FRAMED.with(|c| c.borrow_mut().retain(|k, _| k.0 != path));
    DIMS.with(|c| c.borrow_mut().remove(path));
}

/// What a thumbnail file's PNG header tells before it's decoded: a view
/// can lay out its item before the picture is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbHeader {
    pub width: i32,
    pub height: i32,
    /// Whether it has transparency (an alpha channel, or a tRNS chunk).
    pub alpha: bool,
}

pub fn thumb_header(path: &str) -> Option<ThumbHeader> {
    if let Some(d) = DIMS.with(|c| c.borrow().get(path).copied()) {
        return d;
    }
    let d = (|| {
        use std::io::Read;
        // The chunks before the picture's data are in its first few
        // kilobytes (thumbnails' text chunks are short).
        let mut head = Vec::with_capacity(8192);
        std::fs::File::open(path).ok()?.take(8192).read_to_end(&mut head).ok()?;
        parse_png_header(&head)
    })();
    DIMS.with(|c| c.borrow_mut().insert(path.to_string(), d));
    d
}

fn parse_png_header(b: &[u8]) -> Option<ThumbHeader> {
    if b.len() < 26 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (width, height) = (be(16) as i32, be(20) as i32);
    // Colour types 4 and 6 have an alpha channel; others may have a tRNS
    // chunk, which comes before the picture's data.
    let mut alpha = matches!(b[25], 4 | 6);
    let mut i = 8;
    while !alpha && i + 8 <= b.len() {
        let (len, kind) = (be(i) as usize, &b[i + 4..i + 8]);
        if kind == b"IDAT" {
            break;
        }
        alpha = kind == b"tRNS";
        i += 12 + len;
    }
    (width > 0 && height > 0).then_some(ThumbHeader { width, height, alpha })
}

/// A picture `w`×`h` scaled so its longest side is `size`.
pub fn fit(w: i32, h: i32, size: i32) -> (i32, i32) {
    let scale = size as f64 / w.max(h) as f64;
    (((w as f64 * scale).round() as i32).max(1), ((h as f64 * scale).round() as i32).max(1))
}

/// How far a thumbnail's shadow reaches (see framed_thumb).
pub const SHADOW: i32 = 3;

/// A thumbnail as Caja shows it in the icon view, with a shadow below and
/// to the right: `SHADOW` pixels more at the bottom and on both sides (so
/// the picture stays centered). None while it's still being loaded. Caja
/// leaves it out for pictures with transparency (see pane's
/// icon_view_thumb): it would show through.
pub fn framed_thumb(path: &str, size: i32) -> Option<Pixbuf> {
    let key = (path.to_string(), size);
    if let Some(p) = FRAMED.with(|c| c.borrow().get(&key).cloned()) {
        return Some(p);
    }
    let pb = thumb_pixbuf(path, size)?;
    let (w, h) = (pb.width(), pb.height());
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, w + 2 * SHADOW, h + SHADOW).ok()?;
    {
        use gdk::prelude::GdkContextExt;
        let cr = gtk::cairo::Context::new(&surface).ok()?;
        // Copies offset by 1 to 3 pixels: darker close to the picture.
        for d in 1..=SHADOW {
            cr.rectangle((SHADOW + d) as f64, d as f64, w as f64, h as f64);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
            let _ = cr.fill();
        }
        cr.set_source_pixbuf(&pb, SHADOW as f64, 0.0);
        let _ = cr.paint();
    }
    let framed = gdk::pixbuf_get_from_surface(&surface, 0, 0, w + 2 * SHADOW, h + SHADOW)?;
    FRAMED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 4000 {
            c.clear();
        }
        c.insert(key, framed.clone());
    });
    Some(framed)
}

struct ThumbJob {
    uri: String,
    src: std::path::PathBuf,
    mtime: i64,
    out: std::path::PathBuf,
    /// A custom thumbnailer to make the picture; None for built-in images.
    custom: Option<Arc<Thumbnailer>>,
}

pub fn thumb_path_for(uri: &str) -> std::path::PathBuf {
    let md5 = glib::compute_checksum_for_data(glib::ChecksumType::Md5, uri.as_bytes())
        .map(|s| s.to_string())
        .unwrap_or_default();
    glib::user_cache_dir()
        .join("thumbnails")
        .join("large")
        .join(format!("{md5}.png"))
}

/// Register a callback fired on the main thread when a thumbnail is ready: (uri, thumb_path).
/// Returns an id for remove_thumbnail_listener.
pub fn on_thumbnail_ready(f: impl Fn(&str, &str) + 'static) -> usize {
    let id = NEXT_LISTENER.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    LISTENERS.with(|l| l.borrow_mut().push((id, Box::new(f))));
    id
}

pub fn remove_thumbnail_listener(id: usize) {
    LISTENERS.with(|l| l.borrow_mut().retain(|(i, _)| *i != id));
}

/// Thumbnails to make, the latest asked for first: views ask for what
/// they show, so that's what was just scrolled to.
/// Called with (uri, thumbnail path) when a thumbnail is made.
type Listener = Box<dyn Fn(&str, &str)>;

struct JobQueue {
    jobs: Mutex<Vec<ThumbJob>>,
    ready: Condvar,
}

/// How many thumbnails are made at once (see makers_for); 1 if the number
/// of processors can't be told.
fn maker_count() -> usize {
    std::thread::available_parallelism().map_or(1, |n| makers_for(n.get()))
}

/// Thumbnail makers for `cpus` processors: one fewer than there are, so
/// one is always left for the rest (the window drawing, other programs),
/// at most 8, and at least 1.
fn makers_for(cpus: usize) -> usize {
    if cpus > 8 {
        8
    } else {
        cpus.saturating_sub(1).max(1)
    }
}

fn makers() -> Arc<JobQueue> {
    if let Some(q) = MAKERS.with(|m| m.borrow().clone()) {
        return q;
    }
    let q = Arc::new(JobQueue { jobs: Mutex::new(vec![]), ready: Condvar::new() });
    let (done_tx, done_rx) = async_channel::unbounded::<(String, Option<String>)>();
    for _ in 0..maker_count() {
        let q = q.clone();
        let done_tx = done_tx.clone();
        std::thread::spawn(move || loop {
            let job = {
                let mut jobs = q.jobs.lock().unwrap();
                loop {
                    if let Some(j) = jobs.pop() {
                        break j;
                    }
                    jobs = q.ready.wait(jobs).unwrap();
                }
            };
            let res = make_thumbnail(&job);
            if res.is_none() {
                record_failure(&job);
            }
            if done_tx.send_blocking((job.uri, res)).is_err() {
                return;
            }
        });
    }
    glib::MainContext::default().spawn_local(async move {
        while let Ok((uri, res)) = done_rx.recv().await {
            if let Some(p) = res {
                forget_thumb(&p);
                LISTENERS.with(|l| {
                    for (_, f) in l.borrow().iter() {
                        f(&uri, &p);
                    }
                });
            }
        }
    });
    MAKERS.with(|m| *m.borrow_mut() = Some(q.clone()));
    q
}

/// Where Arkha notes the files it couldn't make a thumbnail of, as the
/// thumbnail spec has it, so they aren't tried every time they're shown.
fn fail_dir() -> std::path::PathBuf {
    glib::user_cache_dir().join("thumbnails").join("fail").join("arkha")
}

fn fail_path_for(uri: &str) -> std::path::PathBuf {
    let large = thumb_path_for(uri);
    fail_dir().join(large.file_name().unwrap_or_default())
}

/// A tiny picture with the file's URI and modification time: a changed
/// file is tried again.
fn record_failure(job: &ThumbJob) {
    let out = fail_path_for(&job.uri);
    let Some(pb) = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 1, 1) else { return };
    pb.fill(0);
    if std::fs::create_dir_all(fail_dir()).is_err() {
        return;
    }
    let mtime = job.mtime.to_string();
    let _ = pb.savev(&out, "png", &[("tEXt::Thumb::URI", job.uri.as_str()), ("tEXt::Thumb::MTime", mtime.as_str()), ("tEXt::Software", "Arkha")]);
}

fn make_thumbnail(job: &ThumbJob) -> Option<String> {
    let pb = match &job.custom {
        // The command writes some image; it's then stored like any other
        // thumbnail (scaled, with the Thumb::URI/MTime the cache needs).
        Some(t) => {
            let raw = job.out.with_extension(format!("arkha-{}.raw.png", std::process::id()));
            let ok = thumbnailers::run(t, &job.src, &raw, 256);
            let pb = if ok { Pixbuf::from_file_at_scale(&raw, 256, 256, true).ok() } else { None };
            let _ = std::fs::remove_file(&raw);
            pb?
        }
        None => {
            let pb = Pixbuf::from_file_at_scale(&job.src, 256, 256, true).ok()?;
            pb.apply_embedded_orientation().unwrap_or(pb)
        }
    };
    if let Some(d) = job.out.parent() {
        std::fs::create_dir_all(d).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = job.out.with_extension(format!("arkha-{}.tmp", std::process::id()));
    let mtime = job.mtime.to_string();
    pb.savev(
        &tmp,
        "png",
        &[
            ("tEXt::Thumb::URI", job.uri.as_str()),
            ("tEXt::Thumb::MTime", mtime.as_str()),
            ("tEXt::Software", "Arkha"),
        ],
    )
    .ok()?;
    std::fs::rename(&tmp, &job.out).ok()?;
    Some(job.out.to_string_lossy().to_string())
}

fn builtin_can(ctype: &str) -> bool {
    ctype.starts_with("image/") && !ctype.contains("djvu") && !ctype.contains("x-xcf")
}

/// Content types we can make thumbnails for: images, plus whatever the
/// custom thumbnailers handle.
pub fn can_thumbnail(ctype: &str) -> bool {
    builtin_can(ctype) || custom_for(ctype).is_some()
}

/// Ask for a thumbnail to be generated (async). Results come via on_thumbnail_ready.
/// A custom thumbnailer for the type is used before the built-in one.
pub fn request_thumbnail(uri: &str, path: &std::path::Path, mtime: i64, size: u64, ctype: &str) {
    let custom = custom_for(ctype);
    // Big images are slow to decode; custom thumbnailers (videos…) decide themselves.
    if custom.is_none() && (!builtin_can(ctype) || size > 60 * 1024 * 1024) {
        return;
    }
    let first = QUEUED.with(|q| q.borrow_mut().insert((uri.to_string(), mtime)));
    if !first {
        return;
    }
    let out = thumb_path_for(uri);
    // Don't thumbnail our own cache.
    if path.starts_with(glib::user_cache_dir().join("thumbnails")) {
        return;
    }
    if png_thumb_mtime(&fail_path_for(uri)) == Some(mtime.to_string()) {
        return;
    }
    let q = makers();
    q.jobs.lock().unwrap().push(ThumbJob {
        uri: uri.to_string(),
        src: path.to_path_buf(),
        mtime,
        out,
        custom,
    });
    q.ready.notify_one();
}

/// A usable existing thumbnail for this file, if the cache has a fresh one.
pub fn existing_thumbnail(info: &gio::FileInfo, uri: &str, mtime: i64) -> Option<String> {
    if let Some(p) = info.attribute_byte_string("thumbnail::path") {
        let valid = if info.has_attribute("thumbnail::is-valid") {
            info.boolean("thumbnail::is-valid")
        } else {
            true
        };
        if valid && !p.is_empty() {
            return Some(p.to_string());
        }
    }
    // Check our own location (older GLib doesn't report thumbnail::path for large).
    let p = thumb_path_for(uri);
    if png_thumb_mtime(&p) == Some(mtime.to_string()) {
        return Some(p.to_string_lossy().to_string());
    }
    None
}

/// The Thumb::MTime text of a thumbnail PNG, read from the chunks before the
/// image data so we don't decode the picture on the main thread.
fn png_thumb_mtime(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut sig = [0u8; 8];
    f.read_exact(&mut sig).ok()?;
    if sig != *b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    loop {
        let mut head = [0u8; 8];
        f.read_exact(&mut head).ok()?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = &head[4..8];
        if kind == b"IDAT" || kind == b"IEND" || len > 1 << 20 {
            return None;
        }
        let mut data = vec![0u8; len + 4]; // chunk data + CRC
        f.read_exact(&mut data).ok()?;
        if kind == b"tEXt" {
            let text = &data[..len];
            if let Some(i) = text.iter().position(|b| *b == 0) {
                if &text[..i] == b"Thumb::MTime" {
                    return Some(String::from_utf8_lossy(&text[i + 1..]).to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    /// A PNG's signature and chunks. The CRCs are left zero: they aren't checked.
    fn png(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in chunks {
            v.extend((data.len() as u32).to_be_bytes());
            v.extend(*kind);
            v.extend(*data);
            v.extend([0u8; 4]);
        }
        v
    }

    #[test]
    fn header_tells_size_and_transparency() {
        // 300×200, 8 bits, the given colour type.
        let ihdr = |colour: u8| -> Vec<u8> { [&300u32.to_be_bytes()[..], &200u32.to_be_bytes()[..], &[8, colour, 0, 0, 0]].concat() };
        let head = |chunks: &[(&[u8; 4], &[u8])]| parse_png_header(&png(chunks));
        let (rgba, rgb) = (ihdr(6), ihdr(2));
        assert_eq!(head(&[(b"IHDR", &rgba), (b"IDAT", b"")]), Some(ThumbHeader { width: 300, height: 200, alpha: true }));
        assert_eq!(head(&[(b"IHDR", &rgb), (b"tEXt", b"Thumb::MTime\x001"), (b"IDAT", b"")]).map(|h| h.alpha), Some(false));
        assert_eq!(head(&[(b"IHDR", &rgb), (b"tRNS", b"\0\0"), (b"IDAT", b"")]).map(|h| h.alpha), Some(true));
        // A tRNS after the picture's data isn't one.
        assert_eq!(head(&[(b"IHDR", &rgb), (b"IDAT", b""), (b"tRNS", b"\0\0")]).map(|h| h.alpha), Some(false));
        assert_eq!(parse_png_header(b"GIF89a"), None);
    }

    #[test]
    fn thumbnail_path() {
        // The example in the freedesktop.org thumbnail spec.
        let p = thumb_path_for("file:///home/jens/photos/me.png");
        assert!(p.ends_with("thumbnails/large/c6ee772d9e49320e97ec29a7eb5b1697.png"));
    }

    #[test]
    fn thumbnail_mtime() {
        let t = TestDir::new("png");
        let write = |name: &str, bytes: &[u8]| {
            let p = t.0.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let ihdr: &[u8] = &[0; 13];
        let good = png(&[(b"IHDR", ihdr), (b"tEXt", b"Software\0GIMP"), (b"tEXt", b"Thumb::MTime\x001700000000"), (b"IDAT", b"")]);
        assert_eq!(png_thumb_mtime(&write("good.png", &good)).as_deref(), Some("1700000000"));
        // Only the chunks before the picture data are read.
        let late = png(&[(b"IHDR", ihdr), (b"IDAT", b""), (b"tEXt", b"Thumb::MTime\x001")]);
        assert_eq!(png_thumb_mtime(&write("late.png", &late)), None);
        let without = png(&[(b"IHDR", ihdr), (b"tEXt", b"Thumb::URI\0file:///x"), (b"IEND", b"")]);
        assert_eq!(png_thumb_mtime(&write("without.png", &without)), None);
        assert_eq!(png_thumb_mtime(&write("cut.png", &good[..good.len() / 2])), None);
        assert_eq!(png_thumb_mtime(&write("jpeg.png", b"\xff\xd8\xff\xe0 not a png")), None);
        assert_eq!(png_thumb_mtime(&t.0.join("missing.png")), None);
    }

    #[test]
    fn existing_thumbnails() {
        let i = gio::FileInfo::new();
        let uri = "file:///no/such/arkha-test-file.png";
        assert_eq!(existing_thumbnail(&i, uri, 5), None);
        i.set_attribute_byte_string("thumbnail::path", "/x/t.png");
        assert_eq!(existing_thumbnail(&i, uri, 5).as_deref(), Some("/x/t.png"));
        // An outdated one isn't used.
        i.set_attribute_boolean("thumbnail::is-valid", false);
        assert_eq!(existing_thumbnail(&i, uri, 5), None);
    }

    #[test]
    fn makers_leave_a_processor_free() {
        let counts: Vec<usize> = [1, 2, 4, 8, 9, 16, 64].iter().map(|&n| makers_for(n)).collect();
        assert_eq!(counts, [1, 1, 3, 7, 8, 8, 8]);
    }

    #[test]
    fn builtin_thumbnail_types() {
        assert!(builtin_can("image/png") && builtin_can("image/jpeg"));
        assert!(!builtin_can("image/vnd.djvu") && !builtin_can("image/x-xcf") && !builtin_can("text/plain"));
    }
}
