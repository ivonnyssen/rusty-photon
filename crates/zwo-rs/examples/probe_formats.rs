//! Hardware probe for issue #881: what does a camera *advertise* as its
//! supported output formats, and does it actually honour them?
//!
//! INDI guards RAW16 on ASI120/ASI130 by **device name**, not by checking
//! `SupportedVideoFormat` — which implies the SDK advertises RAW16 on those
//! models even though 16-bit does not work reliably. That would make
//! "enumerate the array and pick" an incomplete fix. This probe measures the
//! answer instead of inheriting the assumption.
//!
//! Run against a physically attached camera, with the real SDK linked:
//!
//! ```sh
//! cargo run -p zwo-rs --no-default-features --features camera --example probe_formats
//! ```
//!
//! It is read-mostly: it opens the camera, sets ROI formats, and takes short
//! dark exposures at a small ROI. It never touches the cooler.

use std::time::{Duration, Instant};

use zwo_rs::{Camera, ControlType, ExposureStatus, ImageType, Sdk};

/// Small ROI so each frame is quick even on a USB2 camera; width % 8 == 0 and
/// height % 2 == 0 per the SDK's alignment rule.
const PROBE_W: u32 = 320;
const PROBE_H: u32 = 240;
const EXPOSURE_US: i64 = 100_000;

fn main() {
    let sdk = Sdk::new().expect("SDK init");
    let cameras = sdk.cameras().expect("enumerate cameras");
    if cameras.is_empty() {
        eprintln!("no ZWO cameras found — check the USB connection and udev rules");
        std::process::exit(1);
    }

    for (index, info) in cameras.iter().enumerate() {
        println!("=== camera {index}: {} ===", info.name);
        println!("  bit_depth (ADC)      : {}", info.bit_depth);
        println!("  is_color             : {}", info.is_color);
        println!("  is_usb3              : {}", info.is_usb3);
        println!(
            "  max frame            : {}x{}",
            info.max_width, info.max_height
        );
        println!("  supported_bins       : {:?}", info.supported_bins);
        println!(
            "  SupportedVideoFormat : {:?}",
            info.supported_video_formats
        );
        println!(
            "  advertises Raw16?    : {}",
            info.supported_video_formats.contains(&ImageType::Raw16)
        );

        let camera = match sdk.open_camera(index) {
            Ok(camera) => camera,
            Err(e) => {
                println!("  !! open failed: {e} — skipping the format trials");
                continue;
            }
        };

        // Small ROI first, then the full frame at bin 1 and bin 2: the field
        // reports of unreliable 16-bit on ASI120-class cameras describe
        // bandwidth-dependent failures, which a 320x240 crop would not reach.
        // Every geometry must satisfy the SDK's width%8 / height%2 rule, and
        // the binned one must satisfy it *after* halving — `full_h / 2` is odd
        // whenever `max_height % 4 != 0`, so align each one independently.
        let align = |w: u32, h: u32| (w - w % 8, h - h % 2);
        let (full_w, full_h) = align(info.max_width, info.max_height);
        let (bin2_w, bin2_h) = align(full_w / 2, full_h / 2);
        let geometries = [
            (PROBE_W, PROBE_H, 1),
            (full_w, full_h, 1),
            (bin2_w, bin2_h, 2),
        ];
        for (w, h, bin) in geometries {
            for image_type in [ImageType::Raw16, ImageType::Raw8] {
                println!("  --- trial: {image_type:?} at {w}x{h} bin {bin} ---");
                trial(&camera, image_type, w, h, bin);
            }
        }
    }
}

fn trial(camera: &Camera, image_type: ImageType, width: u32, height: u32, bin: u32) {
    match camera.set_roi_format(width, height, bin, image_type) {
        Ok(()) => println!("    set_roi_format       : OK"),
        Err(e) => {
            // This is the failure the issue predicts for an unsupported
            // format: ASI_ERROR_INVALID_IMGTYPE surfacing through asi_check.
            println!("    set_roi_format       : REJECTED ({e})");
            return;
        }
    }

    // Everything below is sized and decoded from what the SDK says the ROI IS,
    // never from what we asked for. A silent substitution is precisely what
    // this probe exists to catch, and sizing from the request would hide it
    // behind a `BufferTooSmall` download error or decode the bytes at the
    // wrong element width.
    let roi = match camera.roi_format() {
        Ok(roi) => {
            println!(
                "    read back            : {}x{} bin {} {:?}{}",
                roi.width,
                roi.height,
                roi.bin,
                roi.image_type,
                if roi.image_type == image_type {
                    ""
                } else {
                    "   <-- MISMATCH: the SDK silently kept another format"
                }
            );
            roi
        }
        Err(e) => {
            println!("    read back            : FAILED ({e})");
            return;
        }
    };

    if let Err(e) = camera.set_control_value(ControlType::Exposure, EXPOSURE_US, false) {
        println!("    set exposure         : FAILED ({e})");
        return;
    }

    // `is_dark = true`: no mechanical shutter on these models, so this is just
    // a frame with whatever light is present. Nothing is actuated.
    if let Err(e) = camera.start_exposure(true) {
        println!("    start_exposure       : FAILED ({e})");
        return;
    }

    let started = Instant::now();
    let deadline = started + Duration::from_secs(15);
    loop {
        match camera.exposure_status() {
            Ok(ExposureStatus::Success) => break,
            Ok(ExposureStatus::Failed) => {
                println!("    exposure             : SDK reported FAILED");
                return;
            }
            Ok(_) => {}
            Err(e) => {
                println!("    exposure_status      : FAILED ({e})");
                return;
            }
        }
        if Instant::now() >= deadline {
            println!("    exposure             : TIMED OUT after 15s");
            let _ = camera.stop_exposure();
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let Some(needed) = roi.buffer_len() else {
        println!("    download             : frame too large to address on this target");
        return;
    };
    let mut buf = vec![0u8; needed];
    match camera.download_exposure(&mut buf) {
        Ok(()) => {
            println!(
                "    download             : OK ({needed} bytes in {:?})",
                started.elapsed()
            );
            report_pixels(&buf, roi.image_type);
        }
        Err(e) => println!("    download             : FAILED ({e})"),
    }
}

/// Sanity-check the delivered frame: an all-zero or all-saturated frame, or a
/// 16-bit frame whose high bytes are all zero, is the shape a format that is
/// advertised but not honoured would produce.
fn report_pixels(buf: &[u8], image_type: ImageType) {
    match image_type {
        ImageType::Raw8 => {
            let (min, max, sum) = buf.iter().fold((u8::MAX, 0u8, 0u64), |(lo, hi, sum), &b| {
                (lo.min(b), hi.max(b), sum + u64::from(b))
            });
            println!(
                "    pixels (8-bit)       : min {min} max {max} mean {:.1}",
                sum as f64 / buf.len() as f64
            );
        }
        _ => {
            let pixels: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                .collect();
            let (min, max, sum) = pixels
                .iter()
                .fold((u16::MAX, 0u16, 0u64), |(lo, hi, sum), &p| {
                    (lo.min(p), hi.max(p), sum + u64::from(p))
                });
            let high_bytes_all_zero = buf.chunks_exact(2).all(|c| c[1] == 0);
            println!(
                "    pixels (16-bit)      : min {min} max {max} mean {:.1}{}",
                sum as f64 / pixels.len() as f64,
                if high_bytes_all_zero {
                    "   <-- every high byte is 0: not real 16-bit data"
                } else {
                    ""
                }
            );
            // How the ADC's bits sit in the 16-bit container decides what
            // MaxADU should report: a bare left shift leaves the low bits
            // always zero, a genuine rescale populates them.
            let low_bits_always_zero = |mask: u16| pixels.iter().all(|p| p & mask == 0);
            let shift = (1..=8u16).find(|&s| !low_bits_always_zero((1 << s) - 1));
            match shift {
                Some(1) => println!("    packing              : low bits populated - rescaled, not shifted"),
                Some(s) => println!(
                    "    packing              : low {} bits always zero - left-shifted by {} (ADC value = pixel >> {})",
                    s - 1,
                    s - 1,
                    s - 1
                ),
                None => println!("    packing              : low 8 bits always zero (or frame is flat)"),
            }
        }
    }
}
