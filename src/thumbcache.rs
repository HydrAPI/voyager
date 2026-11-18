//! Thumbnails that go with their files: moved along when Arkha moves or
//! renames a file, removed when it deletes one (also by emptying the
//! trash). The freedesktop.org thumbnail spec names a thumbnail after its
//! file's URI and keeps that URI in it (Thumb::URI), so a moved file's
//! thumbnail would otherwise be made again under the new name, and the old
//! one stay behind for good. Files trashed keep theirs (restored, they're
//! back at their old URI); files changed by other programs aren't known.

use gio::prelude::*;
use std::path::{Path, PathBuf};

/// The sizes the spec keeps thumbnails in.
const SIZES: [&str; 4] = ["normal", "large", "x-large", "xx-large"];

/// Where thumbnails are kept.
pub fn root() -> PathBuf {
    glib::user_cache_dir().join("thumbnails")
}

/// A thumbnail's file name: the MD5 of the file's URI.
fn name(uri: &str) -> String {
    let md5 = glib::compute_checksum_for_data(glib::ChecksumType::Md5, uri.as_bytes()).unwrap_or_default();
    format!("{md5}.png")
}

/// Arkha's note that a file's thumbnail couldn't be made.
fn fail_note(root: &Path, uri: &str) -> PathBuf {
    root.join("fail").join("arkha").join(name(uri))
}

/// Only local files have thumbnails here.
fn local(uri: &str) -> bool {
    uri.starts_with("file:///")
}

/// The thumbnails (each size) and failure note of `uri` removed: the file
/// was deleted.
pub fn forget(root: &Path, uri: &str) {
    if !local(uri) {
        return;
    }
    let n = name(uri);
    for s in SIZES {
        let _ = std::fs::remove_file(root.join(s).join(&n));
    }
    let _ = std::fs::remove_file(fail_note(root, uri));
}

/// The thumbnails of moved or renamed items (URI before, URI now) moved
/// along, also those of everything in a moved folder (found by going
/// through it at its new place). All are read before any is written, so
/// renames like a → b, b → a keep each its own. A thumbnail whose
/// Thumb::URI isn't the item's is left; failure notes go (the file is
/// tried again at its new place), as do the new names' old thumbnails
/// (of items the move replaced).
pub fn moved(root: &Path, items: &[(String, String)]) {
    let mut all = vec![];
    for (old, new) in items {
        if local(old) && local(new) && old != new {
            all.push((old.clone(), new.clone()));
            below(old, new, &mut all);
        }
    }
    let mut keep = vec![];
    for (old, new) in &all {
        for s in SIZES {
            if let Some(png) = std::fs::read(root.join(s).join(name(old))).ok().and_then(|b| with_uri(&b, old, new)) {
                keep.push((root.join(s).join(name(new)), png));
            }
        }
    }
    for (old, new) in &all {
        forget(root, old);
        forget(root, new);
    }
    for (path, png) in keep {
        write(&path, &png);
    }
}

/// (URI before, URI now) of everything in the folder `new` (moved there
/// from `old`), added to `out`. Links aren't followed.
fn below(old: &str, new: &str, out: &mut Vec<(String, String)>) {
    let Some(dir) = gio::File::for_uri(new).path() else { return };
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let old = gio::File::for_uri(old);
    let mut todo = vec![dir.clone()];
    while let Some(d) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(rel) = p.strip_prefix(&dir) else { continue };
            out.push((old.resolve_relative_path(rel).uri().to_string(), gio::File::for_path(&p).uri().to_string()));
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push(p);
            }
        }
    }
}

/// Writes a thumbnail as the spec asks: readable only by its owner, and
/// in one piece (written beside it, then renamed).
fn write(path: &Path, png: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // Its own temporary name: moves and renames run on threads of their own,
    // and with one name for all, two at once wrote into each other's file.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if path.parent().is_none() {
        return;
    }
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("arkha-{}-{n}.tmp", std::process::id()));
    let written = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(png));
    if written.and_then(|_| std::fs::rename(&tmp, path)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The PNG `png` with its Thumb::URI `new` instead of `old`; None if it
/// isn't a whole PNG or its Thumb::URI isn't `old`. Only that text chunk is
/// rewritten: the picture isn't decoded.
fn with_uri(png: &[u8], old: &str, new: &str) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut rest = png.strip_prefix(SIG)?;
    let mut out = Vec::with_capacity(png.len() + new.len());
    out.extend_from_slice(SIG);
    let (mut found, mut ended) = (false, false);
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let chunk = rest.get(..12 + len)?;
        let (kind, data) = (&chunk[4..8], &chunk[8..8 + len]);
        if kind == b"tEXt" && data.strip_prefix(b"Thumb::URI\0") == Some(old.as_bytes()) {
            let mut text = b"Thumb::URI\0".to_vec();
            text.extend_from_slice(new.as_bytes());
            out.extend_from_slice(&(text.len() as u32).to_be_bytes());
            let mut crc = crc32fast::Hasher::new();
            crc.update(b"tEXt");
            crc.update(&text);
            out.extend_from_slice(b"tEXt");
            out.extend_from_slice(&text);
            out.extend_from_slice(&crc.finalize().to_be_bytes());
            found = true;
        } else {
            out.extend_from_slice(chunk);
        }
        rest = &rest[12 + len..];
        if kind == b"IEND" {
            ended = true;
            break;
        }
    }
    (found && ended).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestDir;

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        c.extend_from_slice(&crc.finalize().to_be_bytes());
        c
    }

    /// A thumbnail PNG (as far as the chunks go) for `uri`.
    fn thumb(uri: &str) -> Vec<u8> {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend(chunk(b"IHDR", &[0; 13]));
        p.extend(chunk(b"tEXt", format!("Thumb::URI\0{uri}").as_bytes()));
        p.extend(chunk(b"tEXt", b"Thumb::MTime\x001700000000"));
        p.extend(chunk(b"IDAT", b"picture"));
        p.extend(chunk(b"IEND", b""));
        p
    }

    fn put(root: &Path, size: &str, uri: &str) {
        std::fs::create_dir_all(root.join(size)).unwrap();
        std::fs::write(root.join(size).join(name(uri)), thumb(uri)).unwrap();
    }

    /// Whether the thumbnail kept for `uri` in `size` is one for it.
    fn has(root: &Path, size: &str, uri: &str) -> bool {
        std::fs::read(root.join(size).join(name(uri))).is_ok_and(|b| b == thumb(uri))
    }

    #[test]
    fn names_as_the_spec_has_them() {
        // The spec's example.
        assert_eq!(name("file:///home/jens/photos/me.png"), "c6ee772d9e49320e97ec29a7eb5b1697.png");
    }

    #[test]
    fn uri_rewritten_in_place() {
        let old = thumb("file:///a/x.mp4");
        let new = with_uri(&old, "file:///a/x.mp4", "file:///b/longer name.mp4").unwrap();
        assert_eq!(new, thumb("file:///b/longer name.mp4"));
        // Not this file's thumbnail, or no PNG: left alone.
        assert!(with_uri(&old, "file:///a/y.mp4", "file:///b/y.mp4").is_none());
        assert!(with_uri(b"GIF89a", "file:///a/x.mp4", "file:///b/x.mp4").is_none());
        assert!(with_uri(&old[..old.len() - 20], "file:///a/x.mp4", "file:///b/x.mp4").is_none());
    }

    #[test]
    fn thumbnails_move_with_files_and_folders() {
        let t = TestDir::new("thumbs");
        let root = t.0.join("cache");
        let files = t.0.join("files");
        std::fs::create_dir_all(files.join("new/sub")).unwrap();
        std::fs::write(files.join("new/sub/clip.mp4"), b"").unwrap();
        std::fs::write(files.join("b.mp4"), b"").unwrap();
        let u = |p: &str| gio::File::for_path(files.join(p)).uri().to_string();
        put(&root, "normal", &u("a.mp4"));
        put(&root, "large", &u("a.mp4"));
        put(&root, "large", &u("old/sub/clip.mp4"));
        std::fs::create_dir_all(root.join("fail/arkha")).unwrap();
        std::fs::write(fail_note(&root, &u("a.mp4")), b"note").unwrap();
        moved(&root, &[(u("a.mp4"), u("b.mp4")), (u("old"), u("new"))]);
        assert!(has(&root, "normal", &u("b.mp4")) && has(&root, "large", &u("b.mp4")));
        assert!(!root.join("normal").join(name(&u("a.mp4"))).exists());
        assert!(!fail_note(&root, &u("a.mp4")).exists());
        // In the moved folder.
        assert!(has(&root, "large", &u("new/sub/clip.mp4")));
        assert!(!root.join("large").join(name(&u("old/sub/clip.mp4"))).exists());
        // Swapped names keep each its own: b's picture (the normal size
        // only it has) goes to the clip's name, the clip's (large) to b's.
        std::fs::remove_file(root.join("large").join(name(&u("b.mp4")))).unwrap();
        moved(&root, &[(u("b.mp4"), u("new/sub/clip.mp4")), (u("new/sub/clip.mp4"), u("b.mp4"))]);
        assert!(has(&root, "normal", &u("new/sub/clip.mp4")) && !root.join("large").join(name(&u("new/sub/clip.mp4"))).exists());
        assert!(has(&root, "large", &u("b.mp4")) && !root.join("normal").join(name(&u("b.mp4"))).exists());
        // Deleted: gone, every size and the note.
        std::fs::write(fail_note(&root, &u("b.mp4")), b"note").unwrap();
        forget(&root, &u("b.mp4"));
        assert!(SIZES.iter().all(|s| !root.join(s).join(name(&u("b.mp4"))).exists()));
        assert!(!fail_note(&root, &u("b.mp4")).exists());
        // Not local: nothing done.
        moved(&root, &[("sftp://h/x".into(), u("x"))]);
    }
}
