//! Custom thumbnailers: commands the user sets up to make thumbnails for
//! file types the built-in (image) thumbnailer doesn't handle, e.g. videos
//! with ffmpeg. Stored in ~/.config/arkha/thumbnailers.conf.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thumbnailer {
    pub name: String,
    /// MIME type patterns separated by ';', e.g. "video/*; application/pdf".
    pub types: String,
    /// Run with `sh -c`; see `expand` for the placeholders.
    pub command: String,
}

/// How long a thumbnailer may run before it's stopped.
const TIMEOUT: Duration = Duration::from_secs(30);

fn path() -> PathBuf {
    glib::user_config_dir().join("arkha").join("thumbnailers.conf")
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
        },
        Thumbnailer {
            name: "Videos (ffmpeg)".into(),
            types: "video/*".into(),
            // The thumbnail filter picks a typical frame near the start, so
            // short clips work too (unlike seeking a few seconds in).
            command: "ffmpeg -y -loglevel error -i %i -vf thumbnail,scale=%s:-1 -frames:v 1 %o".into(),
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
    let mut groups: Vec<(u32, String)> = kf
        .groups()
        .iter()
        .filter_map(|g| Some((g.strip_prefix("Thumbnailer ")?.parse().ok()?, g.to_string())))
        .collect();
    groups.sort();
    groups
        .into_iter()
        .filter_map(|(_, g)| {
            let s = |k: &str| kf.string(&g, k).map(|v| v.to_string()).unwrap_or_default();
            let t = Thumbnailer { name: s("name"), types: s("types"), command: s("command") };
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
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = kf.save_to_file(p);
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

/// Fills in %i (input file), %o (output image) and %s (size in pixels),
/// shell-quoted; %% is a literal %.
pub fn expand(command: &str, input: &Path, output: &Path, size: u32) -> String {
    let q = |p: &Path| glib::shell_quote(p).to_string_lossy().to_string();
    let mut out = String::new();
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('i') => out.push_str(&q(input)),
            Some('o') => out.push_str(&q(output)),
            Some('s') => out.push_str(&size.to_string()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Runs `t` for `input`, writing an image to `output`. Called on the
/// thumbnail worker thread; gives up after TIMEOUT.
pub fn run(t: &Thumbnailer, input: &Path, output: &Path, size: u32) -> bool {
    let cmd = expand(&t.command, input, output, size);
    let Ok(mut child) = std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success() && output.exists(),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
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
        assert_eq!(cmd, "ffmpeg -i '/a b/v.mp4' -vf scale=256:-1 '/t/o.png' 100%");
        let t = Thumbnailer { name: "v".into(), types: "video/*; application/PDF".into(), command: "x".into() };
        assert!(matches(&t, "video/mp4"));
        assert!(installed(&Thumbnailer { command: "sh -c 'true'".into(), ..t.clone() }));
        assert!(!installed(&Thumbnailer { command: "no-such-program-xyz %i %o".into(), ..t.clone() }));
        assert!(defaults().iter().all(|d| matches(d, "video/mp4")));
        assert!(matches(&t, "application/pdf"));
        assert!(!matches(&t, "image/png"));
    }

    #[test]
    fn run_with_timeout_and_result() {
        let dir = std::env::temp_dir().join(format!("arkha-thumbtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("o.png");
        let ok = Thumbnailer { name: "ok".into(), types: "*".into(), command: "printf x > %o".into() };
        assert!(run(&ok, Path::new("/dev/null"), &out, 128));
        let _ = std::fs::remove_file(&out);
        let fails = Thumbnailer { name: "no".into(), types: "*".into(), command: "exit 1".into() };
        assert!(!run(&fails, Path::new("/dev/null"), &out, 128));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
