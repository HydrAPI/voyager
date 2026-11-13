//! Notes the build for the About window (ARKHA_BUILD: "r142, 6a7d287", the
//! number of commits and the commit), and builds the translations: each
//! po/<language>.po into
//! target/<profile>/locale/<language>/LC_MESSAGES/arkha.mo (with gettext's
//! msgfmt), where Arkha finds them when run from the build folder and the
//! PKGBUILD and install.sh take them from, and lists them for Preferences
//! (src/i18n.rs) by the name in their X-Language-Name header. Without
//! msgfmt, Arkha is built in English only.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rustc-env=ARKHA_BUILD={}", build_id().unwrap_or_default());
    println!("cargo:rerun-if-changed=po");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR is target/<profile>/build/arkha-<hash>/out.
    let profile = out.ancestors().nth(3).unwrap().to_path_buf();
    let mut po: Vec<PathBuf> = std::fs::read_dir("po")
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "po")).collect())
        .unwrap_or_default();
    po.sort();
    let mut languages = vec![];
    for p in &po {
        let code = p.file_stem().unwrap().to_string_lossy().to_string();
        let Some(name) = header(p, "X-Language-Name") else {
            println!("cargo:warning={}: no X-Language-Name header, left out", p.display());
            continue;
        };
        let dir = profile.join("locale").join(&code).join("LC_MESSAGES");
        std::fs::create_dir_all(&dir).unwrap();
        match Command::new("msgfmt").arg("--check").arg("-o").arg(dir.join("arkha.mo")).arg(p).status() {
            Ok(s) if s.success() => languages.push((code, name)),
            Ok(_) => panic!("msgfmt found mistakes in {}", p.display()),
            Err(e) => {
                println!("cargo:warning=msgfmt (gettext) not found ({e}): built without translations");
                break;
            }
        }
    }
    let list: String = languages.iter().map(|(c, n)| format!("({c:?}, {n:?}), ")).collect();
    std::fs::write(
        out.join("languages.rs"),
        format!("/// The translations there are: (code, name in its own language).\npub const LANGUAGES: &[(&str, &str)] = &[{list}];\n"),
    )
    .unwrap();
}

/// The build: from ARKHA_BUILD if a packager sets it, else from git in a
/// checkout (the PKGBUILD builds in one), else from the VERSION file that
/// packaging/remote-build.sh writes into the sources it copies
/// ("1.0.0-dev+r142.g6a7d287"). None for sources with neither.
fn build_id() -> Option<String> {
    println!("cargo:rerun-if-env-changed=ARKHA_BUILD");
    if let Ok(b) = std::env::var("ARKHA_BUILD") {
        return Some(b);
    }
    // Rebuilt after a commit (or a checkout of another one).
    for p in [".git/HEAD", ".git/refs/heads", ".git/packed-refs", "VERSION"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    let git = |args: &[&str]| {
        let out = Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    if Path::new(".git").exists() {
        if let (Some(n), Some(commit)) = (git(&["rev-list", "--count", "HEAD"]), git(&["rev-parse", "--short", "HEAD"])) {
            return Some(format!("r{n}, {commit}"));
        }
    }
    let version = std::fs::read_to_string("VERSION").ok()?;
    let (count, commit) = version.trim().split_once('+')?.1.split_once(".g")?;
    Some(format!("{count}, {}", commit.trim_end_matches(".dirty")))
}

/// A header field of a .po file (in its first, empty msgid's msgstr).
fn header(po: &Path, field: &str) -> Option<String> {
    let text = std::fs::read_to_string(po).ok()?;
    let prefix = format!("\"{field}: ");
    let line = text.lines().take(40).find(|l| l.starts_with(&prefix))?;
    Some(line[prefix.len()..].trim_end_matches('"').trim_end_matches("\\n").trim().to_string())
}
