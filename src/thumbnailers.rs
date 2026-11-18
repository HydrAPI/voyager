//! Custom thumbnailers: commands the user sets up to make thumbnails for
//! file types the built-in (image) thumbnailer doesn't handle, e.g. videos
//! with ffmpeg. Stored in ~/.config/arkha/thumbnailers.conf.
//!
//! Also the thumbnailers installed on the system (`system`), as Caja and
//! other file managers use them: PDF with Evince, office documents with
//! libgsf, audio cover art with ffmpegthumbnailer and whatever else is
//! installed later.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thumbnailer {
    pub name: String,
    /// MIME type patterns separated by ';', e.g. "video/*; application/pdf".
    pub types: String,
    /// Run with `sh -c`; see `expand` for the placeholders.
    pub command: String,
    /// One of the system's (`system`): its command is a program and its
    /// arguments, run directly, not by a shell.
    pub system: bool,
}

/// How long a thumbnailer may run before it's stopped.
const TIMEOUT: Duration = Duration::from_secs(30);

fn path() -> PathBuf {
    crate::config::arkha_file("thumbnailers.conf")
}

/// Used until the user changes the list: ffmpegthumbnailer, else ffmpeg,
/// for videos. Ones whose program isn't installed are skipped (`installed`).
pub fn defaults() -> Vec<Thumbnailer> {
    vec![
        Thumbnailer {
            name: "Videos (ffmpegthumbnailer)".into(),
            types: "video/*".into(),
            // -f: a film strip down the sides, as the desktop's own video
            // thumbnailer (and so Caja) makes them, telling videos from pictures.
            command: "ffmpegthumbnailer -i %i -o %o -s %s -f".into(),
            system: false,
        },
        Thumbnailer {
            name: "Videos (ffmpeg)".into(),
            types: "video/*".into(),
            // The thumbnail filter picks a typical frame near the start, so
            // short clips work too (unlike seeking a few seconds in).
            command: "ffmpeg -y -loglevel error -i %i -vf thumbnail,scale=%s:-1 -frames:v 1 %o".into(),
            system: false,
        },
    ]
}

/// Whether the program a thumbnailer runs (its command's first word) is installed.
pub fn installed(t: &Thumbnailer) -> bool {
    glib::shell_parse_argv(&t.command)
        .ok()
        .and_then(|argv| argv.first().cloned())
        .is_some_and(|prog| glib::find_program_in_path(prog).is_some())
}

/// The saved list, or `defaults()` if it was never saved.
pub fn load() -> Vec<Thumbnailer> {
    let kf = glib::KeyFile::new();
    if kf.load_from_file(path(), glib::KeyFileFlags::NONE).is_err() {
        return defaults();
    }
    crate::util::numbered_groups(&kf, "Thumbnailer ")
        .into_iter()
        .filter_map(|g| {
            let s = |k: &str| kf.string(&g, k).map(|v| v.to_string()).unwrap_or_default();
            let t = Thumbnailer { name: s("name"), types: s("types"), command: s("command"), system: false };
            (!t.types.trim().is_empty() && !t.command.trim().is_empty()).then_some(t)
        })
        .collect()
}

pub fn save(list: &[Thumbnailer]) {
    let kf = glib::KeyFile::new();
    for (i, t) in list.iter().enumerate() {
        let g = format!("Thumbnailer {i}");
        kf.set_string(&g, "name", &t.name);
        kf.set_string(&g, "types", &t.types);
        kf.set_string(&g, "command", &t.command);
    }
    crate::util::save_settings(&kf, &path());
}

/// Whether `t` handles content type `ctype` (patterns are case-insensitive globs).
pub fn matches(t: &Thumbnailer, ctype: &str) -> bool {
    let ctype: Vec<char> = ctype.to_lowercase().chars().collect();
    t.types
        .split(';')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .any(|p| crate::util::glob_match(&p.chars().collect::<Vec<_>>(), &ctype))
}

/// The first thumbnailer for `ctype`, if any.
pub fn find(list: &[Thumbnailer], ctype: &str) -> Option<Thumbnailer> {
    list.iter().find(|t| matches(t, ctype)).cloned()
}

/// The thumbnailers installed on the system, as the Thumbnailer
/// specification has file managers find them: the `*.thumbnailer` files in
/// the `thumbnailers` folder of each data folder, the user's first (a file
/// there hides one of the same name further on), those whose program
/// isn't installed left out.
pub fn system() -> Vec<Thumbnailer> {
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("thumbnailers")) else { continue };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "thumbnailer")).collect();
        files.sort();
        for f in files {
            let Some(name) = f.file_name().map(|n| n.to_os_string()) else { continue };
            if seen.insert(name) {
                out.extend(parse_system(&f));
            }
        }
    }
    out
}

/// A `.thumbnailer` file (its [Thumbnailer Entry] group), if its program
/// is installed.
fn parse_system(path: &Path) -> Option<Thumbnailer> {
    let kf = glib::KeyFile::new();
    kf.load_from_file(path, glib::KeyFileFlags::NONE).ok()?;
    let g = "Thumbnailer Entry";
    let command = kf.string(g, "Exec").ok()?.to_string();
    let types: Vec<String> = kf.string_list(g, "MimeType").ok()?.iter().map(|t| t.to_string()).filter(|t| !t.is_empty()).collect();
    if let Ok(try_exec) = kf.string(g, "TryExec") {
        glib::find_program_in_path(try_exec.as_str())?;
    }
    let t = Thumbnailer {
        name: path.file_stem()?.to_string_lossy().to_string(),
        types: types.join(";"),
        command,
        system: true,
    };
    (installed(&t) && !t.types.is_empty()).then_some(t)
}

/// The value of placeholder `c`: %i (input file), %u (its URI), %o
/// (output image), %s (size in pixels).
fn value(c: char, input: &Path, output: &Path, size: u32) -> Option<String> {
    match c {
        'i' => Some(input.to_string_lossy().to_string()),
        'u' => Some(gio::prelude::FileExt::uri(&gio::File::for_path(input)).to_string()),
        'o' => Some(output.to_string_lossy().to_string()),
        's' => Some(size.to_string()),
        _ => None,
    }
}

/// Fills in the placeholders shell-quoted (also within quotes in the
/// command: see util::expand_shell); %% is a literal %.
pub fn expand(command: &str, input: &Path, output: &Path, size: u32) -> String {
    crate::util::expand_shell(command, |c| value(c, input, output, size).map(|v| vec![v]))
}

/// `expand` for one argument of a command run directly (not quoted).
fn expand_arg(arg: &str, input: &Path, output: &Path, size: u32) -> String {
    crate::util::expand_placeholders(arg, |c| value(c, input, output, size))
}

/// Runs `t` for `input`, writing an image to `output`. Called on the
/// thumbnail worker thread; gives up after TIMEOUT. Err says why there's
/// no image (with the end of what the command printed as errors).
pub fn run(t: &Thumbnailer, input: &Path, output: &Path, size: u32) -> Result<(), String> {
    let mut command = if t.system {
        // A program and its arguments (split as GLib splits them).
        let argv = glib::shell_parse_argv(&t.command).map_err(|e| format!("bad command: {e}"))?;
        let mut args = argv.iter().map(|a| expand_arg(&a.to_string_lossy(), input, output, size));
        let Some(program) = args.next() else { return Err("empty command".into()) };
        let mut c = std::process::Command::new(program);
        c.args(args);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(expand(&t.command, input, output, size));
        c
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start it: {e}"))?;
    // Read as it comes, so a chatty command can't fill the pipe and stall.
    let errors = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut text = vec![];
            let _ = std::io::Read::read_to_end(&mut e, &mut text);
            let text = String::from_utf8_lossy(&text);
            let text = text.trim();
            // The end says what went wrong.
            let start = text.char_indices().rev().nth(500).map_or(0, |(i, _)| i);
            text[start..].to_string()
        })
    });
    let errors = move || errors.and_then(|t| t.join().ok()).filter(|t| !t.is_empty()).map(|t| format!(": {t}")).unwrap_or_default();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() && output.exists() => return Ok(()),
            Ok(Some(status)) if status.success() => return Err(format!("it wrote no image{}", errors())),
            Ok(Some(status)) => return Err(format!("it ended with {status}{}", errors())),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            r => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match r {
                    Err(e) => format!("couldn't wait for it: {e}"),
                    _ => format!("stopped after {} s{}", TIMEOUT.as_secs(), errors()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_and_match() {
        let cmd = expand("ffmpeg -i %i -vf scale=%s:-1 %o 100%%", Path::new("/a b/v.mp4"), Path::new("/t/o.png"), 256);
        assert_eq!(cmd, "ffmpeg -i '/a b/v.mp4' -vf scale=256:-1 /t/o.png 100%");
        let t = Thumbnailer { name: "v".into(), types: "video/*; application/PDF".into(), command: "x".into(), system: false };
        assert!(matches(&t, "video/mp4"));
        assert!(installed(&Thumbnailer { command: "sh -c 'true'".into(), ..t.clone() }));
        assert!(!installed(&Thumbnailer { command: "no-such-program-xyz %i %o".into(), ..t.clone() }));
        assert!(defaults().iter().all(|d| matches(d, "video/mp4")));
        assert!(matches(&t, "application/pdf"));
        assert!(!matches(&t, "image/png"));
    }

    #[test]
    fn saved_and_loaded() {
        let _ = std::fs::remove_file(path());
        // Never saved: the defaults; saved empty: none.
        assert_eq!(load(), defaults());
        save(&[]);
        assert!(load().is_empty());
        let t = Thumbnailer { name: "PDF".into(), types: "application/pdf".into(), command: "pdftoppm -png -scale-to %s %i > %o".into(), system: false };
        save(std::slice::from_ref(&t));
        assert_eq!(load(), std::slice::from_ref(&t));
        assert_eq!(find(&load(), "application/pdf"), Some(t));
        assert_eq!(find(&load(), "video/mp4"), None);
        let _ = std::fs::remove_file(path());
    }

    #[test]
    fn system_thumbnailer_files() {
        let dir = crate::testutil::TestDir::new("systhumb");
        let file = dir.0.join("pdf.thumbnailer");
        std::fs::write(&file, "[Thumbnailer Entry]\nTryExec=sh\nExec=sh -c 'cp %i %o' --size %s %u\nMimeType=application/pdf;application/x-ext-pdf;\n").unwrap();
        let t = parse_system(&file).unwrap();
        assert!(t.system && t.name == "pdf");
        assert!(matches(&t, "application/pdf") && matches(&t, "application/x-ext-pdf") && !matches(&t, "image/png"));
        // Not installed: left out.
        let gone = dir.0.join("gone.thumbnailer");
        std::fs::write(&gone, "[Thumbnailer Entry]\nTryExec=no-such-program-xyz\nExec=no-such-program-xyz %i %o\nMimeType=text/plain;\n").unwrap();
        assert!(parse_system(&gone).is_none());
        let bad = dir.0.join("bad.thumbnailer");
        std::fs::write(&bad, "[Something Else]\nExec=sh\n").unwrap();
        assert!(parse_system(&bad).is_none());
        // %u is the input's URI.
        let input = dir.0.join("a b.pdf");
        assert_eq!(expand_arg("%u", &input, Path::new("/o.png"), 128), gio::prelude::FileExt::uri(&gio::File::for_path(&input)).to_string());
    }

    #[test]
    fn system_thumbnailers_run_without_a_shell() {
        let dir = crate::testutil::TestDir::new("systhumbrun");
        let input = dir.0.join("in $HOME file.txt");
        std::fs::write(&input, "data").unwrap();
        let out = dir.0.join("out.png");
        // `cp` gets the name as it is: no shell to split it or expand $HOME.
        let t = Thumbnailer { name: "cp".into(), types: "*".into(), command: "cp %i %o".into(), system: true };
        assert!(run(&t, &input, &out, 128).is_ok());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "data");
    }

    #[test]
    fn run_with_timeout_and_result() {
        let dir = crate::testutil::TestDir::new("thumbnailer");
        let out = dir.0.join("o.png");
        let ok = Thumbnailer { name: "ok".into(), types: "*".into(), command: "printf x > %o".into(), system: false };
        assert!(run(&ok, Path::new("/dev/null"), &out, 128).is_ok());
        let _ = std::fs::remove_file(&out);
        let fails = Thumbnailer { name: "no".into(), types: "*".into(), command: "echo broken >&2; exit 1".into(), system: false };
        assert_eq!(run(&fails, Path::new("/dev/null"), &out, 128), Err("it ended with exit status: 1: broken".into()));
        let silent = Thumbnailer { name: "none".into(), types: "*".into(), command: "true".into(), system: false };
        assert_eq!(run(&silent, Path::new("/dev/null"), &out, 128), Err("it wrote no image".into()));
    }
}
