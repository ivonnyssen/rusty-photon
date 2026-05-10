//! Asserts every shipped locale parses, has the same key set as the
//! fallback (`en`), and uses matching placeholder names per key.
//!
//! Runs under `cargo test`, `cargo nextest`, the workspace `cargo rail run
//! --profile commit -q` pre-push gate, GitHub Actions, and Bazel's
//! `:translations` test target — so any of those will fail if a
//! translator's PR introduces a parse error, drops a key, adds an unknown
//! key, or renames a `{ $var }` placeholder.
//!
//! Reads the `i18n/` tree at runtime via [`rp_i18n::verify_translations_in_dir`],
//! which walks a directory through its own `FsAssets` impl, rather than going
//! through `RustEmbed` (whose compile-time directory walk would bake in a
//! sandbox path that won't exist when the test action runs).
//!
//! Resolving the runtime path differs by build system, so [`locate_i18n_dir`]
//! falls back through several candidates instead of trusting a single source:
//!
//! - **Cargo:** `env!("CARGO_MANIFEST_DIR")` expands to the package source
//!   directory at compile time and stays valid at test runtime — the tree
//!   committed at `services/ppba-driver/i18n/` is right there.
//! - **Bazel:** `rules_rust` sets `CARGO_MANIFEST_DIR` to a compile-time
//!   sandbox path that no longer exists when the test runs from the runfiles
//!   tree. Instead, the `data = glob(["i18n/**/*.ftl"])` on the `:translations`
//!   target stages the same files under `$TEST_SRCDIR/_main/services/ppba-driver/i18n/`,
//!   which `locate_i18n_dir` finds via `TEST_SRCDIR`.

use std::path::{Path, PathBuf};

/// Find the `i18n/` directory at runtime by trying, in order:
/// `CARGO_MANIFEST_DIR/i18n` (Cargo), `$TEST_SRCDIR/_main/services/ppba-driver/i18n`
/// (Bazel runfiles), and finally `<cwd>/services/ppba-driver/i18n`. See the
/// module docs for why each path is or isn't reliable per build system.
fn locate_i18n_dir() -> PathBuf {
    let mut tried = Vec::new();

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
    if manifest.is_dir() {
        return manifest;
    }
    tried.push(manifest);

    if let Ok(srcdir) = std::env::var("TEST_SRCDIR") {
        // Bazel's standard runfiles layout for an external repo with name "_main"
        let bazel = Path::new(&srcdir).join("_main/services/ppba-driver/i18n");
        if bazel.is_dir() {
            return bazel;
        }
        tried.push(bazel);
    }

    // Last resort: the test's CWD often is the package dir under cargo nextest
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join("services/ppba-driver/i18n");
        if cwd_path.is_dir() {
            return cwd_path;
        }
        tried.push(cwd_path);
    }

    panic!(
        "could not locate ppba-driver i18n/ tree at runtime; candidates tried:\n{}",
        tried
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn all_locales_are_parseable_and_consistent_with_en() {
    let i18n_dir = locate_i18n_dir();
    let report = rp_i18n::verify_translations_in_dir(&i18n_dir, "en");
    assert!(
        report.is_clean(),
        "translation issues against fallback `{}` (locales: {:?}):\n{:#?}",
        report.fallback,
        report.locales,
        report.issues
    );
}
