//! Helpers for the unit tests.

use std::path::{Path, PathBuf};

/// A folder for a test's files, removed when dropped. It's in target/, not
/// the system's temp folder, which may be a small RAM disk.
pub struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new(name: &str) -> TestDir {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("arkha-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TestDir(p)
    }

    /// Writes `rel` (creating its folders) and returns its path.
    pub fn file(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
