//! The catalog's embed list must match the packaging tree. Walks the source
//! tree via `CARGO_MANIFEST_DIR`, so it runs under Cargo only (the Bazel
//! target is tagged `requires-cargo` and rides the nightly safety net).

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// Every `services/*/pkg` directory carries a `doctor.toml` and appears in
/// the catalog, and nothing else does (docs/services/doctor.md §The derived
/// catalog).
#[test]
fn test_catalog_matches_the_packaging_tree() {
    let services_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut packaged: Vec<String> = std::fs::read_dir(services_dir)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let pkg = entry.path().join("pkg");
            if !pkg.is_dir() {
                return None;
            }
            let name = entry.file_name().into_string().unwrap();
            assert!(
                pkg.join("doctor.toml").is_file(),
                "services/{name}/pkg exists but has no doctor.toml — add one \
                 (docs/services/doctor.md §The derived catalog)"
            );
            Some(name)
        })
        .collect();
    packaged.sort();
    let known: Vec<String> = doctor::catalog::catalog()
        .iter()
        .map(|e| e.name.to_string())
        .collect();
    assert_eq!(
        packaged, known,
        "the embed list in catalog.rs must match services/*/pkg"
    );
}
