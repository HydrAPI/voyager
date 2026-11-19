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
    let Some(kf) = crate::util::load_settings(&path()) else { return defaults() };
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
/// (output image), %s (size in pixels). As bytes: a name needn't be UTF-8.
fn value(c: char, input: &Path, output: &Path, size: u32) -> Option<std::ffi::OsString> {
    match c {
        'i' => Some(input.as_os_str().to_os_string()),
        'u' => Some(gio::prelude::FileExt::uri(&gio::File::for_path(input)).to_string().into()),
        'o' => Some(output.as_os_str().to_os_string()),
        's' => Some(size.to_string().into()),
        _ => None,
    }
}

/// Fills in the placeholders shell-quoted (also within quotes in the
/// command: see util::expand_shell); %% is a literal %.
pub fn expand(command: &str, input: &Path, output: &Path, size: u32) -> std::ffi::OsString {
    crate::util::expand_shell(command, |c| value(c, input, output, size).map(|v| vec![v]))
}

/// `expand` for one argument of a command run directly (not quoted).
fn expand_arg(arg: &str, input: &Path, output: &Path, size: u32) -> std::ffi::OsString {
    crate::util::expand_placeholders(arg, |c| value(c, input, output, size))
}

/// Runs `t` for `input`, writing an image to `output`. Called on the
/// thumbnail worker thread; gives up after TIMEOUT. Err says why there's
/// no image (with the end of what the command printed as errors).
pub fn run(t: &Thumbnailer, input: &Path, output: &Path, size: u32) -> Result<(), String> {
    // In the sandbox the image is written into a folder of its own, the
    // only one it can write to, and moved to `output` from there.
    let jail = if sandboxed(t) { sandbox::bwrap() } else { None };
    let cell = match jail {
        Some(_) => Some(sandbox::Cell::new().map_err(|e| format!("no folder for the sandbox: {e}"))?),
        None => None,
    };
    let written = cell.as_ref().map_or_else(|| output.to_path_buf(), |c| c.dir.join(output.file_name().unwrap_or_default()));
    let argv: Vec<std::ffi::OsString> = if t.system {
        // A program and its arguments (split as GLib splits them).
        let argv = glib::shell_parse_argv(&t.command).map_err(|e| format!("bad command: {e}"))?;
        argv.iter().map(|a| expand_arg(&a.to_string_lossy(), input, &written, size)).collect()
    } else {
        vec!["sh".into(), "-c".into(), expand(&t.command, input, &written, size)]
    };
    let Some(program) = argv.first() else { return Err("empty command".into()) };
    let mut command = match (jail, &cell) {
        (Some(bwrap), Some(cell)) => {
            glib::g_debug!("arkha", "thumbnail: “{}” runs in a sandbox", t.name);
            let mut c = std::process::Command::new(bwrap);
            c.args(sandbox::args(input, &cell.dir)).arg("--").args(&argv);
            c
        }
        _ => {
            let mut c = std::process::Command::new(program);
            c.args(&argv[1..]);
            c
        }
    };
    let result = wait_for(&mut command, &written);
    if result.is_ok() && written != output {
        std::fs::copy(&written, output).map_err(|e| format!("couldn't take its image: {e}"))?;
    }
    result
}

/// Whether `t` runs in the sandbox: the system's thumbnailers and Arkha's
/// own (as long as they're unchanged). The user's own commands run as
/// written: they may well use scripts in the home folder, which the
/// sandbox doesn't show.
fn sandboxed(t: &Thumbnailer) -> bool {
    t.system || defaults().iter().any(|d| d.command == t.command)
}

/// Runs `command` until it ends (TIMEOUT at most); Ok if it succeeded and
/// wrote `written`.
fn wait_for(command: &mut std::process::Command, written: &Path) -> Result<(), String> {
    let output = written;
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

/// Thumbnailers in a bubblewrap sandbox, as GNOME runs them: they read
/// files from anywhere (Downloads, USB drives) as soon as a folder is
/// shown, and a file made to exploit a bug in one (Ghostscript through
/// PostScript thumbnails, a media library) would otherwise run with all
/// the user's rights. In the sandbox the system is there read-only (the
/// programs, their libraries, fonts), the home folder isn't at all, the
/// file to read is, read-only, and a folder of its own to write the image
/// to; no network, no other processes, and it ends with Arkha. Without
/// bwrap, or where the system doesn't allow sandboxes (some kernels,
/// containers), thumbnailers run as before, which is noted once.
mod sandbox {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// bwrap, if it's installed and can make a sandbox here (tried once);
    /// ARKHA_THUMBNAIL_SANDBOX=0 runs thumbnailers without one.
    pub fn bwrap() -> Option<&'static Path> {
        static BWRAP: OnceLock<Option<PathBuf>> = OnceLock::new();
        BWRAP
            .get_or_init(|| {
                if std::env::var("ARKHA_THUMBNAIL_SANDBOX").is_ok_and(|v| v == "0") {
                    return None;
                }
                let Some(b) = glib::find_program_in_path("bwrap") else {
                    glib::g_message!("arkha", "thumbnails: no bwrap (bubblewrap) installed: thumbnailers run without a sandbox");
                    return None;
                };
                let tried = Cell::new().map_err(|e| format!("no folder for it: {e}")).and_then(|cell| {
                    std::process::Command::new(&b)
                        .args(args(Path::new("/dev/null"), &cell.dir))
                        .args(["--", "true"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::piped())
                        .output()
                        .map_err(|e| e.to_string())
                });
                match tried {
                    Ok(o) if o.status.success() => Some(b),
                    r => {
                        let why = r.map_or_else(|e| e, |o| String::from_utf8_lossy(&o.stderr).trim().to_string());
                        glib::g_message!("arkha", "thumbnails: bwrap can't make a sandbox here ({why}): thumbnailers run without one");
                        None
                    }
                }
            })
            .as_deref()
    }

    /// bwrap's arguments (before "--" and the command) for a thumbnailer
    /// reading `input` and writing into `out_dir`.
    pub fn args(input: &Path, out_dir: &Path) -> Vec<std::ffi::OsString> {
        let mut a: Vec<std::ffi::OsString> = vec![];
        let mut add = |words: &[&std::ffi::OsStr]| a.extend(words.iter().map(|w| w.to_os_string()));
        let s = |x: &'static str| std::ffi::OsStr::new(x);
        add(&[s("--unshare-all"), s("--die-with-parent"), s("--new-session"), s("--clearenv")]);
        let lang = std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into());
        for (k, v) in [("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/tmp"), ("TMPDIR", "/tmp"), ("GIO_USE_VFS", "local"), ("LANG", lang.as_str())] {
            add(&[s("--setenv"), std::ffi::OsStr::new(k), std::ffi::OsStr::new(v)]);
        }
        add(&[s("--ro-bind"), s("/usr"), s("/usr")]);
        // /bin, /lib… are links into /usr on most systems now, folders on others.
        for d in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
            match std::fs::read_link(d) {
                Ok(target) => add(&[s("--symlink"), target.as_os_str(), std::ffi::OsStr::new(d)]),
                Err(_) if Path::new(d).is_dir() => add(&[s("--ro-bind"), std::ffi::OsStr::new(d), std::ffi::OsStr::new(d)]),
                Err(_) => {}
            }
        }
        add(&[s("--ro-bind-try"), s("/etc"), s("/etc"), s("--ro-bind-try"), s("/var/cache/fontconfig"), s("/var/cache/fontconfig")]);
        add(&[s("--proc"), s("/proc"), s("--dev"), s("/dev"), s("--tmpfs"), s("/tmp")]);
        add(&[s("--ro-bind"), input.as_os_str(), input.as_os_str(), s("--bind"), out_dir.as_os_str(), out_dir.as_os_str()]);
        a
    }

    /// A folder of its own for one thumbnailer to write into (removed again
    /// when dropped), only the user's: in the runtime folder, or in the
    /// cache folder where that can't be written to (seen where
    /// XDG_RUNTIME_DIR was another user's).
    pub struct Cell {
        pub dir: PathBuf,
    }

    impl Cell {
        pub fn new() -> std::io::Result<Cell> {
            use std::os::unix::fs::DirBuilderExt;
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = format!("arkha-thumbnail-{}-{n}", std::process::id());
            let in_runtime = glib::user_runtime_dir().join(&name);
            if std::fs::DirBuilder::new().mode(0o700).create(&in_runtime).is_ok() {
                return Ok(Cell { dir: in_runtime });
            }
            let cache = glib::user_cache_dir().join("arkha");
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&cache)?;
            let dir = cache.join(&name);
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
            Ok(Cell { dir })
        }
    }

    impl Drop for Cell {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
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
        assert_eq!(expand_arg("%u", &input, Path::new("/o.png"), 128), std::ffi::OsString::from(gio::prelude::FileExt::uri(&gio::File::for_path(&input)).to_string()));
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
    fn sandboxed_thumbnailers_see_only_their_file() {
        let Some(_) = sandbox::bwrap() else {
            eprintln!("no sandbox possible here (bwrap missing or not allowed): not tested");
            return;
        };
        let dir = crate::testutil::TestDir::new("sandbox");
        let (input, out, secret, planted) = (dir.0.join("clip.mp4"), dir.0.join("o.png"), dir.0.join("secret.txt"), dir.0.join("planted"));
        std::fs::write(&input, "picture").unwrap();
        std::fs::write(&secret, "key").unwrap();
        let sys = |cmd: String| Thumbnailer { name: "t".into(), types: "*".into(), command: cmd, system: true };
        // Its own file it reads, its image it writes.
        assert_eq!(run(&sys("cp %i %o".into()), &input, &out, 128), Ok(()));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "picture");
        std::fs::remove_file(&out).unwrap();
        // Another file there it can't read, nothing it can write there:
        // each command works as the user's own (no sandbox), not in it.
        let p = |x: &Path| {
            let x = x.to_string_lossy().to_string();
            assert!(!x.contains(['"', '\'', ' ', '$', '\\']), "{x}");
            x
        };
        let read = format!("cat {} > %o", p(&secret));
        let own = |cmd: &str| Thumbnailer { system: false, ..sys(cmd.into()) };
        assert_eq!(run(&own(&read), &input, &out, 128), Ok(()));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "key");
        std::fs::remove_file(&out).unwrap();
        assert!(run(&sys(format!("sh -c \"{read}\"")), &input, &out, 128).is_err());
        assert!(!out.exists());
        let plant = format!("touch {}; cp %i %o", p(&planted));
        assert_eq!(run(&own(&plant), &input, &out, 128), Ok(()));
        assert!(planted.exists());
        std::fs::remove_file(&planted).unwrap();
        assert_eq!(run(&sys(format!("sh -c \"{plant}\"")), &input, &out, 128), Ok(()));
        assert!(!planted.exists());
        // Arkha's own video thumbnailers run in it too; the user's own don't.
        assert!(defaults().iter().all(sandboxed));
        assert!(!sandboxed(&Thumbnailer { system: false, ..sys("my-script %i %o".into()) }));
    }

    #[test]
    fn thumbnailers_get_names_that_arent_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let dir = crate::testutil::TestDir::new("latin1thumb");
        let input = dir.0.join(std::ffi::OsStr::from_bytes(b"caf\xe9.png"));
        std::fs::write(&input, "picture").unwrap();
        let out = dir.0.join("o.png");
        for system in [true, false] {
            let t = Thumbnailer { name: "cp".into(), types: "*".into(), command: "cp %i %o".into(), system };
            assert_eq!(run(&t, &input, &out, 128), Ok(()), "system: {system}");
            assert_eq!(std::fs::read_to_string(&out).unwrap(), "picture");
            std::fs::remove_file(&out).unwrap();
        }
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
