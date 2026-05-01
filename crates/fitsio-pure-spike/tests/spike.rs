//! fitsio-pure 0.11 spike — issue #107.
//!
//! Eight tests exercising the rp / sky-survey-camera / phd2-guider call-site
//! profiles against `fitsio-pure = 0.11`. See
//! `docs/plans/fitsio-pure-spike.md` for the open questions and the
//! decision-gate matrix.
//!
//! Note: `build_image_hdu_with_scaling` is in fitsio-pure's master (PR #51,
//! 2026-02-18) but did NOT make it into the published 0.11.0. We use the
//! lower-level API (`build_primary_header` + `serialize_header` +
//! `serialize_image_*`) which is what a real wrapper crate would use anyway.

use std::io::Write;
use std::time::Instant;

use fitsio_pure::hdu::parse_fits;
use fitsio_pure::header::{serialize_header, Card};
use fitsio_pure::image::{
    extract_bscale_bzero, read_image_data, read_image_physical, serialize_image,
    serialize_image_i16, serialize_image_i32, ImageData,
};
use fitsio_pure::primary::build_primary_header;
use fitsio_pure::value::Value;

// ---------- helpers ----------

fn keyword(name: &str) -> [u8; 8] {
    let mut k = [b' '; 8];
    let bytes = name.as_bytes();
    let len = bytes.len().min(8);
    k[..len].copy_from_slice(&bytes[..len]);
    k
}

fn card(name: &str, value: Value) -> Card {
    Card {
        keyword: keyword(name),
        value: Some(value),
        comment: None,
    }
}

/// Build full primary-HDU bytes from cards + raw data, padded by fitsio-pure.
fn build_hdu_bytes(cards: &[Card], data_bytes: &[u8]) -> Vec<u8> {
    let header_bytes = serialize_header(cards);
    let mut buf = Vec::with_capacity(header_bytes.len() + data_bytes.len());
    buf.extend_from_slice(&header_bytes);
    buf.extend_from_slice(data_bytes);
    // serialize_image_* already pads the data block to a 2880 boundary.
    buf
}

// ===========================================================================
// A. rp profile (BITPIX=32 + custom DOC_ID keyword)
// ===========================================================================

#[test]
fn a1_rp_round_trip_i32_image_with_doc_id() {
    let pixels: Vec<i32> = vec![100, 200, 300, 400];
    let doc_id = "550e8400-e29b-41d4-a716-446655440000";

    // Build cards: SIMPLE/BITPIX/NAXIS via build_primary_header, then append DOC_ID.
    let mut cards = build_primary_header(32, &[2, 2]).unwrap();
    cards.push(card("DOC_ID", Value::String(doc_id.to_string())));

    let data_bytes = serialize_image_i32(&pixels);
    let bytes = build_hdu_bytes(&cards, &data_bytes);

    // Write to tempfile (rp's atomic write goes through std::fs separately).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rp_capture.fits");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&bytes).unwrap();
    drop(f);

    // Reopen.
    let read_bytes = std::fs::read(&path).unwrap();
    let fits = parse_fits(&read_bytes).unwrap();
    let primary = fits.primary();

    // Pixels round-trip.
    let read_back = read_image_data(&read_bytes, primary).unwrap();
    assert_eq!(read_back, ImageData::I32(pixels));

    // DOC_ID round-trips through the card list.
    let doc_id_back = primary
        .cards
        .iter()
        .find(|c| std::str::from_utf8(&c.keyword).unwrap().trim() == "DOC_ID")
        .and_then(|c| match &c.value {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(doc_id_back, doc_id);
}

// ===========================================================================
// B. sky-survey-camera profile (the load-bearing case)
// ===========================================================================

#[test]
fn b1_sky_survey_bitpix16_bzero_unsigned_range() {
    // Construct a SkyView-shaped fixture: BITPIX=16 + BZERO=32768 + BSCALE=1.0
    // encoding unsigned-16 values [0, 32768, 65535, 12345].
    let unsigned: [u16; 4] = [0, 32768, 65535, 12345];
    let signed: Vec<i16> = unsigned
        .iter()
        .map(|u| (*u as i32 - 32768) as i16)
        .collect();

    let mut cards = build_primary_header(16, &[2, 2]).unwrap();
    cards.push(card("BSCALE", Value::Float(1.0)));
    cards.push(card("BZERO", Value::Float(32768.0)));

    let data_bytes = serialize_image_i16(&signed);
    let bytes = build_hdu_bytes(&cards, &data_bytes);

    // Parse from &[u8] — sky-survey-camera's hot path.
    let fits = parse_fits(&bytes).unwrap();
    let primary = fits.primary();

    // *** LOAD-BEARING: read_image_physical applies BSCALE/BZERO. ***
    let physical = read_image_physical(&bytes, primary).unwrap();
    assert_eq!(physical, vec![0.0, 32768.0, 65535.0, 12345.0]);

    // Confirm the unsigned-16 range is recovered (the entire reason fitrs was rejected).
    let recovered: Vec<u16> = physical.iter().map(|f| *f as u16).collect();
    assert_eq!(recovered, vec![0u16, 32768, 65535, 12345]);
}

#[test]
fn b2_skyview_float_with_bscale_bzero_applied() {
    // BITPIX=-32 with non-trivial BSCALE/BZERO — verify both are honoured.
    let raw: [f32; 4] = [0.0, 1.0, -1.0, 100.5];
    let mut cards = build_primary_header(-32, &[2, 2]).unwrap();
    cards.push(card("BSCALE", Value::Float(2.0)));
    cards.push(card("BZERO", Value::Float(5.0)));

    let data_bytes = serialize_image(&ImageData::F32(raw.to_vec()));
    let bytes = build_hdu_bytes(&cards, &data_bytes);

    let fits = parse_fits(&bytes).unwrap();
    let primary = fits.primary();

    let physical = read_image_physical(&bytes, primary).unwrap();
    // Stored: [0.0, 1.0, -1.0, 100.5]; physical = stored*2 + 5
    assert_eq!(physical, vec![5.0, 7.0, 3.0, 206.0]);

    // Sanity: extract_bscale_bzero returns the values we put in.
    let (bscale, bzero) = extract_bscale_bzero(&primary.cards);
    assert_eq!((bscale, bzero), (2.0, 5.0));
}

#[test]
fn b3_parse_from_byte_buffer_no_filesystem() {
    // Sanity check: the read path is `&[u8]` -> data, no tempfile required.
    let mut cards = build_primary_header(16, &[2, 2]).unwrap();
    cards.push(card("BSCALE", Value::Float(1.0)));
    cards.push(card("BZERO", Value::Float(32768.0)));
    let data = serialize_image_i16(&[0i16, 1, 2, 3]);
    let bytes = build_hdu_bytes(&cards, &data);

    let fits = parse_fits(&bytes).unwrap();
    assert_eq!(fits.len(), 1);
    let _primary = fits.primary();
    // No filesystem touched anywhere in this test by construction.
}

// ===========================================================================
// C. phd2-guider profile (u16 native via BZERO=32768 encoding)
// ===========================================================================

#[test]
fn c1_phd2_u16_via_bzero_writes() {
    // Restore the native u16 storage that ADR-001's first supersession
    // gave up on. fitsio-pure has no U16 ImageData variant -- u16 lands
    // on disk as BITPIX=16 + BZERO=32768 + signed-16 bytes, exactly how
    // the FITS standard mandates.
    let unsigned: Vec<u16> = vec![0, 1, 100, 65535, 32768, 32767];
    let signed: Vec<i16> = unsigned
        .iter()
        .map(|u| (*u as i32 - 32768) as i16)
        .collect();

    let mut cards = build_primary_header(16, &[3, 2]).unwrap();
    cards.push(card("BSCALE", Value::Float(1.0)));
    cards.push(card("BZERO", Value::Float(32768.0)));
    cards.push(card("OBJECT", Value::String("Guide Star".to_string())));
    cards.push(card("ORIGIN", Value::String("PHD2".to_string())));

    let data_bytes = serialize_image_i16(&signed);
    let bytes = build_hdu_bytes(&cards, &data_bytes);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phd2_thumbnail.fits");
    std::fs::write(&path, &bytes).unwrap();

    let read_bytes = std::fs::read(&path).unwrap();
    let fits = parse_fits(&read_bytes).unwrap();
    let primary = fits.primary();

    // Read back as physical, convert to u16 -> assert.
    let physical = read_image_physical(&read_bytes, primary).unwrap();
    let recovered: Vec<u16> = physical.iter().map(|f| *f as u16).collect();
    assert_eq!(recovered, unsigned);

    // Custom string headers come back.
    let object = primary
        .cards
        .iter()
        .find(|c| std::str::from_utf8(&c.keyword).unwrap().trim() == "OBJECT")
        .and_then(|c| match &c.value {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(object.trim(), "Guide Star");
}

// ===========================================================================
// D. multi-HDU
// ===========================================================================

#[test]
fn d1_multi_hdu_iteration() {
    // Primary (NAXIS=0) + one image extension. Verify the iterator
    // surfaces both HDUs and primary() / get(1) work.
    use fitsio_pure::extension::{build_extension_header, ExtensionType};

    let primary_cards = build_primary_header(8, &[]).unwrap();
    let primary_bytes = serialize_header(&primary_cards);

    let ext_cards = build_extension_header(ExtensionType::Image, 32, &[2, 2], 0, 1).unwrap();
    let ext_header_bytes = serialize_header(&ext_cards);
    let ext_data_bytes = serialize_image_i32(&[10i32, 20, 30, 40]);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&primary_bytes);
    bytes.extend_from_slice(&ext_header_bytes);
    bytes.extend_from_slice(&ext_data_bytes);

    let fits = parse_fits(&bytes).unwrap();
    assert_eq!(fits.len(), 2);
    let _primary = fits.primary();
    let ext = fits.get(1).unwrap();

    let read_back = read_image_data(&bytes, ext).unwrap();
    assert_eq!(read_back, ImageData::I32(vec![10, 20, 30, 40]));

    let count = fits.iter().count();
    assert_eq!(count, 2);
}

// ===========================================================================
// E. QHY600 scale (the throughput question)
// ===========================================================================

#[test]
fn e1_qhy600_scale_write_smoke() {
    // 9576 x 6388 u16 = ~122 MB raw, ~131 MB FITS-padded. Time
    // build + serialise + write + read + parse + read_image_physical.
    // Loose assertion: under 60 seconds total. We care about
    // "fast enough", not micro-perf.
    let w: usize = 9576;
    let h: usize = 6388;
    let pixel_count = w * h;

    let t_alloc = Instant::now();
    let signed: Vec<i16> = (0..pixel_count)
        .map(|i| ((i % 65536) as i32 - 32768) as i16)
        .collect();
    let alloc_elapsed = t_alloc.elapsed();

    let t_build = Instant::now();
    let mut cards = build_primary_header(16, &[w, h]).unwrap();
    cards.push(card("BSCALE", Value::Float(1.0)));
    cards.push(card("BZERO", Value::Float(32768.0)));
    let data_bytes = serialize_image_i16(&signed);
    let bytes = build_hdu_bytes(&cards, &data_bytes);
    let build_elapsed = t_build.elapsed();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("qhy600.fits");

    let t_write = Instant::now();
    std::fs::write(&path, &bytes).unwrap();
    let write_elapsed = t_write.elapsed();

    let t_read = Instant::now();
    let read_bytes = std::fs::read(&path).unwrap();
    let read_elapsed = t_read.elapsed();

    let t_parse = Instant::now();
    let fits = parse_fits(&read_bytes).unwrap();
    let primary = fits.primary();
    let parse_elapsed = t_parse.elapsed();

    let t_decode = Instant::now();
    let raw = read_image_data(&read_bytes, primary).unwrap();
    let decode_elapsed = t_decode.elapsed();

    // Sanity: pixel count survived.
    if let ImageData::I16(v) = &raw {
        assert_eq!(v.len(), pixel_count);
    } else {
        panic!("expected I16");
    }

    let total = alloc_elapsed
        + build_elapsed
        + write_elapsed
        + read_elapsed
        + parse_elapsed
        + decode_elapsed;
    println!("e1: 9576x6388 BITPIX=16 ({} bytes on disk):", bytes.len());
    println!("    alloc:  {:?}", alloc_elapsed);
    println!("    build:  {:?}", build_elapsed);
    println!("    write:  {:?}", write_elapsed);
    println!("    read:   {:?}", read_elapsed);
    println!("    parse:  {:?}", parse_elapsed);
    println!("    decode: {:?}", decode_elapsed);
    println!("    TOTAL:  {:?}", total);

    assert!(
        total < std::time::Duration::from_secs(60),
        "QHY600-scale round-trip took {:?}, expected <60s",
        total
    );
}

// ===========================================================================
// F. build smoke
// ===========================================================================

#[test]
fn f1_compiles_on_target_smoke() {
    // Forces fitsio-pure to be compiled+linked on whichever target
    // CI is running. Pure Rust so trivial on every platform; here
    // mostly to give the Windows-MSVC job a job.
    let cards = build_primary_header(8, &[]).unwrap();
    let bytes = serialize_header(&cards);
    let fits = parse_fits(&bytes).unwrap();
    assert_eq!(fits.len(), 1);
}
