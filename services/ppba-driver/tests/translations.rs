//! Asserts every shipped locale parses, has the same key set as the
//! fallback (`en`), and uses matching placeholder names per key.
//!
//! Runs under `cargo test`, `cargo nextest`, the workspace `cargo rail run
//! --profile commit -q` pre-push gate, GitHub Actions, and Bazel's
//! `:translations` test target — so any of those will fail if a
//! translator's PR introduces a parse error, drops a key, adds an unknown
//! key, or renames a `{ $var }` placeholder.
//!
//! Reads the `i18n/` tree at runtime via [`i18n_embed::FileSystemAssets`]
//! rather than `RustEmbed` because RustEmbed's compile-time directory walk
//! doesn't pick up Bazel `compile_data` symlinks for proc-macro contexts.
//! Filesystem read keys off `CARGO_MANIFEST_DIR`, which both Cargo and
//! `rules_rust` populate consistently, and the `i18n/` directory is shipped
//! to the test action via Bazel's `data` attribute.

use std::path::{Path, PathBuf};

/// Find the `i18n/` directory at runtime.
///
/// Cargo: `env!("CARGO_MANIFEST_DIR")` is the package source dir, and the
/// tree is committed there directly.
///
/// Bazel: `rust_test` runs out of the runfiles tree. Its `data` attribute
/// places listed files at workspace-relative paths (e.g.
/// `services/ppba-driver/i18n/en/ppba-driver.ftl` ends up there too), and
/// `TEST_SRCDIR` points at the workspace root inside the runfiles tree.
/// `env!("CARGO_MANIFEST_DIR")` under Bazel is set to a fixed compile-time
/// sandbox path that does *not* exist at test execution time, so we fall
/// back through several candidates rather than asserting on the first.
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
