//! cfitsio-via-vcpkg spike — issue #107.
//!
//! End-to-end smoke tests against stock `fitsio = "0.21"` from
//! crates.io. The crate's whole point is "no source patches anywhere":
//! these tests exercise the full link path (Rust → fitsio →
//! fitsio-sys → CFITSIO C library) and only succeed when CFITSIO is
//! findable via pkg-config on the host.
//!
//! See `docs/plans/fits-cfitsio-vcpkg-spike.md` for the platform-by-
//! platform setup expectations.

use fitsio::headers::HeaderValue;
use fitsio::images::{ImageDescription, ImageType};
use fitsio::FitsFile;
use tempfile::Builder;

#[test]
fn creates_and_reads_back_i32_image() {
    let tmp = Builder::new()
        .prefix("fits-cfitsio-spike-")
        .tempdir()
        .unwrap();
    let path = tmp.path().join("round_trip.fits");

    let pixels: Vec<i32> = vec![100, 200, 300, 400];
    let desc = ImageDescription {
        data_type: ImageType::Long,
        dimensions: &[2, 2],
    };

    {
        let mut f = FitsFile::create(&path)
            .with_custom_primary(&desc)
            .overwrite()
            .open()
            .unwrap();
        let hdu = f.primary_hdu().unwrap();
        hdu.write_image(&mut f, &pixels).unwrap();
    }

    let mut f = FitsFile::open(&path).unwrap();
    let hdu = f.primary_hdu().unwrap();
    let read_back: Vec<i32> = hdu.read_image(&mut f).unwrap();
    assert_eq!(read_back, pixels);
}

#[test]
fn writes_and_reads_custom_string_keyword() {
    let tmp = Builder::new()
        .prefix("fits-cfitsio-spike-")
        .tempdir()
        .unwrap();
    let path = tmp.path().join("doc_id.fits");

    let pixels: Vec<i32> = vec![1, 2, 3, 4];
    let desc = ImageDescription {
        data_type: ImageType::Long,
        dimensions: &[2, 2],
    };
    let doc_id = "550e8400-e29b-41d4-a716-446655440000";

    {
        let mut f = FitsFile::create(&path)
            .with_custom_primary(&desc)
            .overwrite()
            .open()
            .unwrap();
        let hdu = f.primary_hdu().unwrap();
        hdu.write_key(&mut f, "DOC_ID", doc_id).unwrap();
        hdu.write_image(&mut f, &pixels).unwrap();
    }

    let mut f = FitsFile::open(&path).unwrap();
    let hdu = f.primary_hdu().unwrap();
    let value: HeaderValue<String> = hdu.read_key(&mut f, "DOC_ID").unwrap();
    assert_eq!(value.value, doc_id);
}

#[test]
fn fitsio_links_smoke() {
    // If the C library isn't linked correctly, this fails before the
    // actual round-trip tests get a chance to run — useful for
    // diagnosing "did we link CFITSIO at all" vs "did we link CFITSIO
    // and then break on the API surface".
    let tmp = Builder::new()
        .prefix("fits-cfitsio-spike-")
        .tempdir()
        .unwrap();
    let path = tmp.path().join("smoke.fits");
    let desc = ImageDescription {
        data_type: ImageType::Byte,
        dimensions: &[1, 1],
    };
    let _ = FitsFile::create(&path)
        .with_custom_primary(&desc)
        .overwrite()
        .open()
        .unwrap();
    assert!(path.exists());
}
