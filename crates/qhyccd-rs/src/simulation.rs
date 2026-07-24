//! Simulation support for QHYCCD cameras
//!
//! This module provides simulated camera and filter wheel support, allowing
//! library users to develop and test applications without physical QHYCCD hardware.
//!
//! Folded into a single file (was a `simulation/` subtree) as part of the
//! zwo/svbony convention alignment: the simulated device state is held directly
//! by a [`Camera`](crate::Camera) under `#[cfg(feature = "simulation")]`, not via
//! a runtime backend enum.
//!
//! # Example
//!
//! ```no_run
//! use qhyccd_rs::simulation::{SimulatedCameraConfig, ImagePattern, ImageGenerator};
//!
//! // Create a custom camera configuration
//! let config = SimulatedCameraConfig::default()
//!     .with_id("TEST-001")
//!     .with_filter_wheel(5)
//!     .with_cooler();
//!
//! // Create an image generator for testing
//! let generator = ImageGenerator::new(ImagePattern::StarField)
//!     .with_noise_level(0.02);
//! ```

use crate::{BayerMode, CCDChipArea, CCDChipInfo, ControlType, StreamMode};
use rand::{Rng, RngExt};
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::Instant;

// ===== Simulated camera configuration =====

/// Configuration for a simulated camera
///
/// # Example
/// ```no_run
/// use qhyccd_rs::simulation::SimulatedCameraConfig;
///
/// let config = SimulatedCameraConfig::default()
///     .with_filter_wheel(5)
///     .with_cooler();
/// ```
#[derive(Debug, Clone)]
pub struct SimulatedCameraConfig {
    /// Camera identifier (e.g., "SIM-001")
    pub id: String,
    /// Model name (e.g., "QHY178M-SIM")
    pub model: String,
    /// CCD/CMOS chip information
    pub chip_info: CCDChipInfo,
    /// Effective imaging area
    pub effective_area: CCDChipArea,
    /// Overscan area (if any)
    pub overscan_area: CCDChipArea,
    /// Supported controls with their (min, max, step) values
    pub supported_controls: HashMap<ControlType, (f64, f64, f64)>,
    /// Number of filter wheel slots (0 = no filter wheel)
    pub filter_wheel_slots: u32,
    /// Whether the camera has a cooler
    pub has_cooler: bool,
    /// Bayer mode for color cameras (None = mono)
    pub bayer_mode: Option<BayerMode>,
    /// Available readout modes (name, (width, height))
    pub readout_modes: Vec<(String, (u32, u32))>,
    /// Camera type code
    pub camera_type: u32,
    /// Firmware version string
    pub firmware_version: String,
}

impl Default for SimulatedCameraConfig {
    /// Creates a default configuration similar to a QHY178M
    fn default() -> Self {
        let mut supported_controls = HashMap::new();

        // Basic controls
        supported_controls.insert(ControlType::Gain, (0.0, 100.0, 1.0));
        supported_controls.insert(ControlType::Offset, (0.0, 255.0, 1.0));
        supported_controls.insert(ControlType::Exposure, (1.0, 3_600_000_000.0, 1.0)); // 1us to 1hr
        supported_controls.insert(ControlType::Speed, (0.0, 2.0, 1.0));
        supported_controls.insert(ControlType::UsbTraffic, (0.0, 255.0, 1.0));
        supported_controls.insert(ControlType::TransferBit, (8.0, 16.0, 8.0));

        // Binning modes
        supported_controls.insert(ControlType::CamBin1x1mode, (1.0, 1.0, 1.0));
        supported_controls.insert(ControlType::CamBin2x2mode, (1.0, 1.0, 1.0));

        // Frame modes
        supported_controls.insert(ControlType::CamSingleFrameMode, (1.0, 1.0, 1.0));
        supported_controls.insert(ControlType::CamLiveVideoMode, (1.0, 1.0, 1.0));

        // Bit modes
        supported_controls.insert(ControlType::Cam8bits, (1.0, 1.0, 1.0));
        supported_controls.insert(ControlType::Cam16bits, (1.0, 1.0, 1.0));

        Self {
            id: "SIM-001".to_string(),
            model: "QHY-SIMULATED".to_string(),
            chip_info: CCDChipInfo {
                // um, like the real SDK (verified on a QHY178M: GetQHYCCDChipInfo
                // returns chip dims in micrometers ≈ image_dim × pixel_size, not mm).
                chip_width: 7372.8,  // um (3072 × 2.4)
                chip_height: 4915.2, // um (2048 × 2.4)
                image_width: 3072,
                image_height: 2048,
                pixel_width: 2.4,  // um
                pixel_height: 2.4, // um
                bits_per_pixel: 16,
            },
            effective_area: CCDChipArea {
                start_x: 0,
                start_y: 0,
                width: 3072,
                height: 2048,
            },
            overscan_area: CCDChipArea {
                start_x: 0,
                start_y: 0,
                width: 3072,
                height: 2048,
            },
            supported_controls,
            filter_wheel_slots: 0,
            has_cooler: false,
            bayer_mode: None,
            readout_modes: vec![("Standard".to_string(), (3072, 2048))],
            camera_type: 4010,
            firmware_version: "Firmware version: 2024_1_1".to_string(),
        }
    }
}

impl SimulatedCameraConfig {
    /// Creates a new configuration with a custom ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Sets the camera model name
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Adds filter wheel support with the specified number of slots
    pub fn with_filter_wheel(mut self, slots: u32) -> Self {
        self.filter_wheel_slots = slots;
        if slots > 0 {
            self.supported_controls
                .insert(ControlType::CfwPort, (0.0, (slots - 1) as f64, 1.0));
            self.supported_controls
                .insert(ControlType::CfwSlotsNum, (slots as f64, slots as f64, 0.0));
        }
        self
    }

    /// Makes this a color camera with the specified Bayer pattern
    pub fn with_color(mut self, bayer_mode: BayerMode) -> Self {
        self.bayer_mode = Some(bayer_mode);
        self.supported_controls.insert(
            ControlType::CamColor,
            (bayer_mode as u32 as f64, bayer_mode as u32 as f64, 0.0),
        );
        self.supported_controls
            .insert(ControlType::Wbr, (0.0, 255.0, 1.0));
        self.supported_controls
            .insert(ControlType::Wbb, (0.0, 255.0, 1.0));
        self.supported_controls
            .insert(ControlType::Wbg, (0.0, 255.0, 1.0));
        self
    }

    /// Adds cooler support
    pub fn with_cooler(mut self) -> Self {
        self.has_cooler = true;
        self.supported_controls
            .insert(ControlType::Cooler, (-40.0, 30.0, 0.1));
        self.supported_controls
            .insert(ControlType::CurTemp, (-40.0, 50.0, 0.1));
        self.supported_controls
            .insert(ControlType::CurPWM, (0.0, 255.0, 1.0));
        self.supported_controls
            .insert(ControlType::ManualPWM, (0.0, 255.0, 1.0));
        self
    }

    /// Sets custom chip information
    pub fn with_chip_info(mut self, chip_info: CCDChipInfo) -> Self {
        self.effective_area = CCDChipArea {
            start_x: 0,
            start_y: 0,
            width: chip_info.image_width,
            height: chip_info.image_height,
        };
        self.overscan_area = self.effective_area;
        self.chip_info = chip_info;
        self
    }

    /// Adds a readout mode
    pub fn with_readout_mode(mut self, name: impl Into<String>, width: u32, height: u32) -> Self {
        self.readout_modes.push((name.into(), (width, height)));
        self
    }

    /// Sets the firmware version string
    pub fn with_firmware_version(mut self, version: impl Into<String>) -> Self {
        self.firmware_version = version.into();
        self
    }

    /// Adds support for a control with the specified min, max, step values
    pub fn with_control(mut self, control: ControlType, min: f64, max: f64, step: f64) -> Self {
        self.supported_controls.insert(control, (min, max, step));
        self
    }
}

// ===== Runtime state for a simulated camera =====

/// Metadata for a captured image
#[derive(Debug, Clone)]
pub(crate) struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    pub channels: u32,
}

/// Runtime state for a simulated camera
#[derive(Debug)]
pub(crate) struct SimulatedCameraState {
    /// Camera configuration (immutable reference data)
    pub config: SimulatedCameraConfig,
    /// Whether the camera is currently open
    pub is_open: bool,
    /// Whether the camera has been initialized
    pub is_initialized: bool,
    /// Current stream mode
    pub stream_mode: Option<StreamMode>,
    /// Current parameter values
    pub parameters: HashMap<ControlType, f64>,
    /// Current ROI settings
    pub roi: CCDChipArea,
    /// Current binning (x, y)
    pub binning: (u32, u32),
    /// Current bit depth for transfers
    pub bit_depth: u32,
    /// Current readout mode index
    pub readout_mode: u32,
    /// Whether live mode is active
    pub live_mode_active: bool,
    /// Exposure start time (for single frame mode)
    pub exposure_start: Option<Instant>,
    /// Exposure duration in microseconds (for single frame mode)
    pub exposure_duration_us: u64,
    /// Pre-generated image data (available after exposure completes)
    pub captured_image: Option<Vec<u8>>,
    /// Dimensions and metadata for the captured image
    pub captured_image_metadata: Option<ImageMetadata>,
    /// Current filter wheel position (0-indexed)
    pub filter_wheel_position: u32,
    /// Current target temperature for cooler
    pub target_temperature: f64,
    /// Current actual temperature (simulated)
    pub current_temperature: f64,
    /// Current cooler PWM
    pub cooler_pwm: f64,
    /// Debayer enabled
    pub debayer_enabled: bool,
}

impl SimulatedCameraState {
    /// Creates a new state from a configuration
    pub fn new(config: SimulatedCameraConfig) -> Self {
        let roi = config.effective_area;
        let bit_depth = config.chip_info.bits_per_pixel;

        // Initialize parameters with default values (middle of range)
        let mut parameters = HashMap::new();
        for (control, (min, max, _step)) in &config.supported_controls {
            let default = match control {
                ControlType::Gain => 0.0,
                ControlType::Offset => 10.0,
                ControlType::Exposure => 1000.0, // 1ms default
                ControlType::Speed => 0.0,
                ControlType::UsbTraffic => 50.0,
                ControlType::TransferBit => 16.0,
                ControlType::CfwPort => 0.0,
                ControlType::CfwSlotsNum => config.filter_wheel_slots as f64,
                ControlType::CurTemp => 20.0, // Room temperature
                ControlType::CurPWM => 0.0,
                ControlType::Cooler => 20.0,
                ControlType::ManualPWM => 0.0,
                _ => (*min + *max) / 2.0,
            };
            parameters.insert(*control, default);
        }

        Self {
            config,
            is_open: false,
            is_initialized: false,
            stream_mode: None,
            parameters,
            roi,
            binning: (1, 1),
            bit_depth,
            readout_mode: 0,
            live_mode_active: false,
            exposure_start: None,
            exposure_duration_us: 1000,
            captured_image: None,
            captured_image_metadata: None,
            filter_wheel_position: 0,
            target_temperature: 20.0,
            current_temperature: 20.0,
            cooler_pwm: 0.0,
            debayer_enabled: false,
        }
    }

    /// Gets the current image dimensions accounting for ROI
    /// Note: ROI dimensions are already in binned coordinates when set via ASCOM Alpaca,
    /// so we don't apply binning division here
    pub fn get_current_image_dimensions(&self) -> (u32, u32) {
        (self.roi.width, self.roi.height)
    }

    /// Gets the number of bytes per pixel based on current bit depth
    pub fn get_bytes_per_pixel(&self) -> u32 {
        if self.bit_depth <= 8 {
            1
        } else {
            2
        }
    }

    /// Gets the number of channels (1 for mono, 3 for color with debayer)
    pub fn get_channels(&self) -> u32 {
        if self.config.bayer_mode.is_some() && self.debayer_enabled {
            3
        } else {
            1
        }
    }

    /// Calculates the required buffer size for the current settings
    pub fn calculate_buffer_size(&self) -> usize {
        let (width, height) = self.get_current_image_dimensions();
        let bytes_per_pixel = self.get_bytes_per_pixel();
        let channels = self.get_channels();
        (width * height * bytes_per_pixel * channels) as usize
    }

    /// Returns the remaining exposure time in microseconds
    pub fn get_remaining_exposure_us(&self) -> u32 {
        match self.exposure_start {
            Some(start) => {
                let elapsed_us = start.elapsed().as_micros() as u64;
                if elapsed_us >= self.exposure_duration_us {
                    0
                } else {
                    (self.exposure_duration_us - elapsed_us) as u32
                }
            }
            None => 0,
        }
    }

    /// Checks if the current exposure is complete
    pub fn is_exposure_complete(&self) -> bool {
        match self.exposure_start {
            Some(start) => {
                let elapsed_us = start.elapsed().as_micros() as u64;
                elapsed_us >= self.exposure_duration_us
            }
            None => true,
        }
    }

    /// Starts an exposure
    pub fn start_exposure(&mut self) {
        // Get exposure time from parameters
        if let Some(&exposure_us) = self.parameters.get(&ControlType::Exposure) {
            self.exposure_duration_us = exposure_us as u64;
        }

        // Pre-generate the image when the exposure starts
        let (width, height) = self.get_current_image_dimensions();
        let bits_per_pixel = self.bit_depth;
        let channels = self.get_channels();

        let generator = ImageGenerator::default();
        let data = if bits_per_pixel <= 8 {
            generator.generate_8bit(width, height, channels)
        } else {
            generator.generate_16bit(width, height, channels)
        };

        // Store the generated image and metadata
        self.captured_image = Some(data);
        self.captured_image_metadata = Some(ImageMetadata {
            width,
            height,
            bits_per_pixel,
            channels,
        });

        // Capture the timestamp last: the exposure clock must not include
        // the frame-generation work above, which can take visible wall time
        // in unoptimized builds on loaded machines and would otherwise eat
        // into (or instantly complete) the simulated exposure.
        self.exposure_start = Some(Instant::now());
    }

    /// Stops the current exposure but keeps the image data
    /// (for CancelQHYCCDExposing - image stays in camera)
    pub fn stop_exposure(&mut self) {
        // Mark exposure as complete while preserving the captured image for later retrieval.
        self.exposure_start = None;
    }

    /// Aborts the current exposure and discards the image data
    /// (for CancelQHYCCDExposingAndReadout - image is discarded)
    pub fn abort_exposure(&mut self) {
        self.exposure_start = None;
        self.captured_image = None;
        self.captured_image_metadata = None;
    }

    /// Updates the simulated temperature (call periodically for realistic behavior)
    #[allow(dead_code)]
    pub fn update_temperature(&mut self) {
        if self.config.has_cooler && self.cooler_pwm > 0.0 {
            // Simple simulation: temperature approaches target based on PWM
            let cooling_rate = self.cooler_pwm / 255.0 * 0.1; // Max 0.1C per update
            if self.current_temperature > self.target_temperature {
                self.current_temperature =
                    (self.current_temperature - cooling_rate).max(self.target_temperature);
            }
        } else {
            // Warm up towards ambient (20C)
            if self.current_temperature < 20.0 {
                self.current_temperature = (self.current_temperature + 0.05).min(20.0);
            }
        }
        // Update the parameter
        self.parameters
            .insert(ControlType::CurTemp, self.current_temperature);
    }
}

// ===== Image generation for simulated frames =====

/// Pattern type for generated images
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ImagePattern {
    /// Gradient from dark to light with noise
    #[default]
    Gradient,
    /// Simulated star field
    StarField,
    /// Flat field with noise
    Flat,
    /// Test pattern with geometric shapes
    TestPattern,
}

/// Per-pixel noise source (xorshift32). Simulated frames need noise that
/// looks plausible, not statistical rigor, and a full `rand` uniform-range
/// sample per pixel dominates frame-generation cost in unoptimized builds
/// — a multi-megapixel frame is millions of samples.
struct PixelNoise {
    state: u32,
}

impl PixelNoise {
    fn new(seed: u32) -> Self {
        // xorshift is stuck at zero; force a nonzero start.
        Self { state: seed | 1 }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Roughly uniform value in `[-range, +range]`. The modulo bias is
    /// negligible at the spans used here (a few thousand out of 2^32).
    fn next_signed(&mut self, range: i32) -> i32 {
        if range <= 0 {
            return 0;
        }
        let span = range as u32 * 2 + 1;
        (self.next_u32() % span) as i32 - range
    }
}

/// Row-distinct seed so rayon rows (and serial frames reusing a seed)
/// don't repeat the same noise sequence.
fn row_seed(frame_seed: u32, row: usize) -> u32 {
    frame_seed ^ (row as u32).wrapping_mul(0x9E37_79B9)
}

/// Generates test images for simulated camera capture
#[derive(Debug, Clone)]
pub struct ImageGenerator {
    pattern: ImagePattern,
    noise_level: f64,
    base_level: u16,
}

impl Default for ImageGenerator {
    fn default() -> Self {
        Self {
            pattern: ImagePattern::Gradient,
            noise_level: 0.05, // 5% noise
            base_level: 1000,  // Base ADU level
        }
    }
}

impl ImageGenerator {
    /// Creates a new generator with the specified pattern
    pub fn new(pattern: ImagePattern) -> Self {
        Self {
            pattern,
            ..Default::default()
        }
    }

    /// Sets the noise level (0.0 to 1.0)
    pub fn with_noise_level(mut self, level: f64) -> Self {
        self.noise_level = level.clamp(0.0, 1.0);
        self
    }

    /// Sets the base signal level
    pub fn with_base_level(mut self, level: u16) -> Self {
        self.base_level = level;
        self
    }

    /// Generates an 8-bit image
    pub fn generate_8bit(&self, width: u32, height: u32, channels: u32) -> Vec<u8> {
        let pixel_count = (width * height) as usize;
        let total_size = pixel_count * channels as usize;
        let mut data = vec![0u8; total_size];
        // One `rand` sample per frame keeps frames distinct; the per-pixel
        // noise itself comes from the cheap `PixelNoise` stream.
        let mut rng = rand::rng();
        let frame_seed: u32 = rng.random();

        match self.pattern {
            ImagePattern::Gradient => {
                self.generate_gradient_8bit(&mut data, width, height, channels, frame_seed)
            }
            ImagePattern::StarField => self
                .generate_starfield_8bit(&mut data, width, height, channels, frame_seed, &mut rng),
            ImagePattern::Flat => {
                self.generate_flat_8bit(&mut data, width, height, channels, frame_seed)
            }
            ImagePattern::TestPattern => {
                self.generate_test_pattern_8bit(&mut data, width, height, channels, frame_seed)
            }
        }

        data
    }

    /// Generates a 16-bit image
    pub fn generate_16bit(&self, width: u32, height: u32, channels: u32) -> Vec<u8> {
        let pixel_count = (width * height) as usize;
        let total_size = pixel_count * channels as usize * 2; // 2 bytes per sample
        let mut data = vec![0u8; total_size];
        // One `rand` sample per frame keeps frames distinct; the per-pixel
        // noise itself comes from the cheap `PixelNoise` stream.
        let mut rng = rand::rng();
        let frame_seed: u32 = rng.random();

        match self.pattern {
            ImagePattern::Gradient => {
                self.generate_gradient_16bit(&mut data, width, channels, frame_seed)
            }
            ImagePattern::StarField => self
                .generate_starfield_16bit(&mut data, width, height, channels, frame_seed, &mut rng),
            ImagePattern::Flat => {
                self.generate_flat_16bit(&mut data, width, height, channels, frame_seed)
            }
            ImagePattern::TestPattern => {
                self.generate_test_pattern_16bit(&mut data, width, height, channels, frame_seed)
            }
        }

        data
    }

    fn generate_gradient_8bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
    ) {
        let base = (self.base_level >> 8) as u8;
        let noise_range = (255.0 * self.noise_level) as i16;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                let gradient = ((x as f64 / width as f64) * 200.0) as u8;
                let noise = noise_source.next_signed(noise_range as i32) as i16;
                let value = (base as i16 + gradient as i16 + noise).clamp(0, 255) as u8;

                let idx = ((y * width + x) * channels) as usize;
                for c in 0..channels as usize {
                    data[idx + c] = value;
                }
            }
        }
    }

    fn generate_gradient_16bit(&self, data: &mut [u8], width: u32, channels: u32, frame_seed: u32) {
        let noise_range = (65535.0 * self.noise_level) as i32;
        let base_level = self.base_level;
        let row_size = (width * channels) as usize * 2;

        // Process rows in parallel; each row gets its own noise stream.
        data.par_chunks_mut(row_size)
            .enumerate()
            .for_each(|(y, row)| {
                let mut noise_source = PixelNoise::new(row_seed(frame_seed, y));

                for x in 0..width {
                    let gradient = ((x as f64 / width as f64) * 50000.0) as u16;
                    let noise = noise_source.next_signed(noise_range);
                    let value =
                        (base_level as i32 + gradient as i32 + noise).clamp(0, 65535) as u16;

                    let idx = (x * channels) as usize * 2;
                    let bytes = value.to_le_bytes();
                    for c in 0..channels as usize {
                        row[idx + c * 2] = bytes[0];
                        row[idx + c * 2 + 1] = bytes[1];
                    }
                }
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_starfield_8bit<R: Rng>(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
        rng: &mut R,
    ) {
        // Fill with background noise
        let base = (self.base_level >> 8) as u8;
        let noise_range = (255.0 * self.noise_level * 0.5) as i16; // Less noise for starfield
        let mut noise_source = PixelNoise::new(frame_seed);

        for pixel in data.iter_mut() {
            let noise = noise_source.next_signed(noise_range as i32) as i16;
            *pixel = (base as i16 + noise).clamp(0, 255) as u8;
        }

        // Add stars
        let num_stars = ((width * height) as f64 * 0.001) as usize; // ~0.1% coverage
        for _ in 0..num_stars {
            let x = rng.random_range(1..width - 1);
            let y = rng.random_range(1..height - 1);
            let brightness = rng.random_range(150..255) as u8;
            let size = rng.random_range(1..=3);

            self.draw_star_8bit(data, width, height, channels, x, y, brightness, size);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn generate_starfield_16bit<R: Rng>(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
        rng: &mut R,
    ) {
        // Fill with background noise
        let noise_range = (65535.0 * self.noise_level * 0.3) as i32;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                let noise = noise_source.next_signed(noise_range);
                let value = (self.base_level as i32 + noise).clamp(0, 65535) as u16;

                let idx = ((y * width + x) * channels) as usize * 2;
                let bytes = value.to_le_bytes();
                for c in 0..channels as usize {
                    data[idx + c * 2] = bytes[0];
                    data[idx + c * 2 + 1] = bytes[1];
                }
            }
        }

        // Add stars
        let num_stars = ((width * height) as f64 * 0.001) as usize;
        for _ in 0..num_stars {
            let x = rng.random_range(2..width - 2);
            let y = rng.random_range(2..height - 2);
            let brightness = rng.random_range(40000..65535) as u16;
            let size = rng.random_range(1..=3);

            self.draw_star_16bit(data, width, height, channels, x, y, brightness, size);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_star_8bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        cx: u32,
        cy: u32,
        brightness: u8,
        size: u32,
    ) {
        for dy in 0..=size * 2 {
            for dx in 0..=size * 2 {
                let x = cx as i32 + dx as i32 - size as i32;
                let y = cy as i32 + dy as i32 - size as i32;

                if x < 0 || x >= width as i32 || y < 0 || y >= height as i32 {
                    continue;
                }

                let dist = (((dx as i32 - size as i32).pow(2) + (dy as i32 - size as i32).pow(2))
                    as f64)
                    .sqrt();
                if dist <= size as f64 {
                    let falloff = 1.0 - (dist / (size as f64 + 1.0));
                    let value = (brightness as f64 * falloff) as u8;

                    let idx = ((y as u32 * width + x as u32) * channels) as usize;
                    for c in 0..channels as usize {
                        data[idx + c] = data[idx + c].saturating_add(value);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_star_16bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        cx: u32,
        cy: u32,
        brightness: u16,
        size: u32,
    ) {
        for dy in 0..=size * 2 {
            for dx in 0..=size * 2 {
                let x = cx as i32 + dx as i32 - size as i32;
                let y = cy as i32 + dy as i32 - size as i32;

                if x < 0 || x >= width as i32 || y < 0 || y >= height as i32 {
                    continue;
                }

                let dist = (((dx as i32 - size as i32).pow(2) + (dy as i32 - size as i32).pow(2))
                    as f64)
                    .sqrt();
                if dist <= size as f64 {
                    let falloff = 1.0 - (dist / (size as f64 + 1.0));
                    let value = (brightness as f64 * falloff) as u16;

                    let idx = ((y as u32 * width + x as u32) * channels) as usize * 2;
                    for c in 0..channels as usize {
                        let current =
                            u16::from_le_bytes([data[idx + c * 2], data[idx + c * 2 + 1]]);
                        let new_value = current.saturating_add(value);
                        let bytes = new_value.to_le_bytes();
                        data[idx + c * 2] = bytes[0];
                        data[idx + c * 2 + 1] = bytes[1];
                    }
                }
            }
        }
    }

    fn generate_flat_8bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
    ) {
        let base = (self.base_level >> 8) as u8;
        let noise_range = (255.0 * self.noise_level) as i16;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                let noise = noise_source.next_signed(noise_range as i32) as i16;
                let value = (base as i16 + noise).clamp(0, 255) as u8;

                let idx = ((y * width + x) * channels) as usize;
                for c in 0..channels as usize {
                    data[idx + c] = value;
                }
            }
        }
    }

    fn generate_flat_16bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
    ) {
        let noise_range = (65535.0 * self.noise_level) as i32;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                let noise = noise_source.next_signed(noise_range);
                let value = (self.base_level as i32 + noise).clamp(0, 65535) as u16;

                let idx = ((y * width + x) * channels) as usize * 2;
                let bytes = value.to_le_bytes();
                for c in 0..channels as usize {
                    data[idx + c * 2] = bytes[0];
                    data[idx + c * 2 + 1] = bytes[1];
                }
            }
        }
    }

    fn generate_test_pattern_8bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
    ) {
        let noise_range = (255.0 * self.noise_level * 0.5) as i16;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                // Create a checkerboard with varying intensities
                let block_size = 64;
                let block_x = x / block_size;
                let block_y = y / block_size;
                let is_light = (block_x + block_y) % 2 == 0;

                let base = if is_light { 200u8 } else { 50u8 };

                // Add concentric circles in center
                let cx = width / 2;
                let cy = height / 2;
                let dist =
                    (((x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2)) as f64).sqrt();
                let ring = ((dist / 50.0) as u32) % 2;
                let ring_mod = if ring == 0 { 20i16 } else { -20i16 };

                let noise = noise_source.next_signed(noise_range as i32) as i16;
                let value = (base as i16 + ring_mod + noise).clamp(0, 255) as u8;

                let idx = ((y * width + x) * channels) as usize;
                for c in 0..channels as usize {
                    data[idx + c] = value;
                }
            }
        }
    }

    fn generate_test_pattern_16bit(
        &self,
        data: &mut [u8],
        width: u32,
        height: u32,
        channels: u32,
        frame_seed: u32,
    ) {
        let noise_range = (65535.0 * self.noise_level * 0.5) as i32;
        let mut noise_source = PixelNoise::new(frame_seed);

        for y in 0..height {
            for x in 0..width {
                // Create a checkerboard with varying intensities
                let block_size = 64;
                let block_x = x / block_size;
                let block_y = y / block_size;
                let is_light = (block_x + block_y) % 2 == 0;

                let base: u16 = if is_light { 50000 } else { 10000 };

                // Add concentric circles in center
                let cx = width / 2;
                let cy = height / 2;
                let dist =
                    (((x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2)) as f64).sqrt();
                let ring = ((dist / 50.0) as u32) % 2;
                let ring_mod: i32 = if ring == 0 { 5000 } else { -5000 };

                let noise = noise_source.next_signed(noise_range);
                let value = (base as i32 + ring_mod + noise).clamp(0, 65535) as u16;

                let idx = ((y * width + x) * channels) as usize * 2;
                let bytes = value.to_le_bytes();
                for c in 0..channels as usize {
                    data[idx + c * 2] = bytes[0];
                    data[idx + c * 2 + 1] = bytes[1];
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod image_generator_tests {
    use super::*;

    // Frame-envelope literals below derive from `ImageGenerator::default()`:
    // base_level 1000 (8-bit base 1000 >> 8 = 3), noise_level 0.05
    // (8-bit ±12, 16-bit ±3276; starfield/test-pattern scale these down).

    const W: u32 = 64;
    const H: u32 = 64;

    fn px16(data: &[u8], width: u32, x: u32, y: u32) -> u16 {
        let idx = ((y * width + x) * 2) as usize;
        u16::from_le_bytes([data[idx], data[idx + 1]])
    }

    #[test]
    fn pixel_noise_zero_or_negative_range_is_zero() {
        let mut noise = PixelNoise::new(42);
        assert_eq!(noise.next_signed(0), 0);
        assert_eq!(noise.next_signed(-5), 0);
    }

    #[test]
    fn pixel_noise_stays_within_range_and_varies() {
        let mut noise = PixelNoise::new(7);
        let samples: Vec<i32> = (0..1000).map(|_| noise.next_signed(100)).collect();
        assert!(samples.iter().all(|v| (-100..=100).contains(v)));
        assert!(
            samples.iter().any(|v| *v != samples[0]),
            "noise stream must not be constant"
        );
    }

    #[test]
    fn row_seed_differs_between_rows() {
        assert_ne!(row_seed(1234, 0), row_seed(1234, 1));
    }

    #[test]
    fn gradient_8bit_brightens_left_to_right() {
        let data = ImageGenerator::new(ImagePattern::Gradient).generate_8bit(W, H, 1);
        assert_eq!(data.len(), (W * H) as usize);
        let col_mean = |x: u32| {
            (0..H)
                .map(|y| data[(y * W + x) as usize] as f64)
                .sum::<f64>()
                / f64::from(H)
        };
        // Left column ≈ base 3, right ≈ 3 + 196; ±12 noise averages out
        // over a column, so a 100-count margin cannot flake.
        assert!(
            col_mean(W - 1) > col_mean(0) + 100.0,
            "gradient must rise left to right: left {} right {}",
            col_mean(0),
            col_mean(W - 1)
        );
    }

    #[test]
    fn gradient_16bit_brightens_left_to_right() {
        let data = ImageGenerator::new(ImagePattern::Gradient).generate_16bit(W, H, 1);
        assert_eq!(data.len(), (W * H * 2) as usize);
        let col_mean =
            |x: u32| (0..H).map(|y| f64::from(px16(&data, W, x, y))).sum::<f64>() / f64::from(H);
        // Left column ≈ base 1000, right ≈ 1000 + 49218; ±3276 noise
        // averages out over a column.
        assert!(
            col_mean(W - 1) > col_mean(0) + 20_000.0,
            "gradient must rise left to right: left {} right {}",
            col_mean(0),
            col_mean(W - 1)
        );
    }

    #[test]
    fn flat_frames_stay_inside_the_noise_envelope() {
        let generator = ImageGenerator::new(ImagePattern::Flat);
        let data8 = generator.generate_8bit(W, H, 1);
        // 8-bit flat: base 3 ± 12, clamped at 0 → every sample ≤ 15.
        assert!(data8.iter().all(|&v| v <= 15));
        let data16 = generator.generate_16bit(W, H, 1);
        // 16-bit flat: base 1000 ± 3276 → every sample ≤ 4276.
        let max = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .map(|(x, y)| px16(&data16, W, x, y))
            .max()
            .unwrap();
        assert!(max <= 4276, "flat sample above the noise envelope: {max}");
        // Noise must actually be present.
        let first = px16(&data16, W, 0, 0);
        assert!((0..W).any(|x| px16(&data16, W, x, 0) != first));
    }

    #[test]
    fn starfield_adds_stars_above_the_background() {
        // 64×64 places 4 stars (0.1% coverage); each star's centre pixel
        // carries its full brightness (≥150 in 8-bit, ≥40000 in 16-bit),
        // far above the ≤15 / ≤1983 background envelope.
        let data8 = ImageGenerator::new(ImagePattern::StarField).generate_8bit(W, H, 1);
        assert!(data8.iter().copied().max().unwrap() >= 100);
        let data16 = ImageGenerator::new(ImagePattern::StarField).generate_16bit(W, H, 1);
        let max = data16
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .max()
            .unwrap();
        assert!(max >= 30_000, "no star found: max sample {max}");
    }

    #[test]
    fn test_pattern_has_light_and_dark_blocks() {
        // 128×128 spans four 64-px checkerboard blocks; (0,0) sits in a
        // light block, (64,0) in a dark one. Both sample points share the
        // same ring modifier, so the block contrast dominates the noise.
        let (w, h) = (128, 128);
        let data8 = ImageGenerator::new(ImagePattern::TestPattern).generate_8bit(w, h, 1);
        let light = data8[0];
        let dark = data8[64];
        assert!(light > dark, "8-bit light {light} dark {dark}");
        let data16 = ImageGenerator::new(ImagePattern::TestPattern).generate_16bit(w, h, 1);
        assert!(px16(&data16, w, 0, 0) > px16(&data16, w, 64, 0));
    }

    #[test]
    fn channels_replicate_each_sample() {
        let data8 = ImageGenerator::new(ImagePattern::Gradient).generate_8bit(8, 8, 3);
        assert_eq!(data8.len(), 8 * 8 * 3);
        for px in data8.chunks_exact(3) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
        let data16 = ImageGenerator::new(ImagePattern::Gradient).generate_16bit(8, 8, 3);
        assert_eq!(data16.len(), 8 * 8 * 3 * 2);
        for px in data16.chunks_exact(6) {
            assert_eq!(px[0..2], px[2..4]);
            assert_eq!(px[2..4], px[4..6]);
        }
    }

    #[test]
    fn zero_noise_level_yields_uniform_flat_frames() {
        let generator = ImageGenerator::new(ImagePattern::Flat).with_noise_level(0.0);
        let data8 = generator.generate_8bit(8, 8, 1);
        assert!(data8.iter().all(|&v| v == data8[0]));
        let data16 = generator.generate_16bit(8, 8, 1);
        assert!(data16.chunks_exact(2).all(|c| c == &data16[0..2]));
    }
}

// ===== Tests for the simulated camera state =====

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod state_tests {
    use super::{SimulatedCameraConfig, SimulatedCameraState};
    use crate::{BayerMode, ControlType};

    #[test]
    fn test_new_state() {
        let config = SimulatedCameraConfig::default();
        let state = SimulatedCameraState::new(config);

        assert!(!state.is_open);
        assert!(!state.is_initialized);
        assert_eq!(state.binning, (1, 1));
    }

    #[test]
    fn test_image_dimensions() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        let (w, h) = state.get_current_image_dimensions();
        assert_eq!(w, 3072);
        assert_eq!(h, 2048);

        // When binning changes, the ROI should be updated to binned coordinates
        // (this is done by the ASCOM Alpaca server in practice)
        state.binning = (2, 2);
        state.roi.width = 1536; // 3072 / 2
        state.roi.height = 1024; // 2048 / 2
        let (w, h) = state.get_current_image_dimensions();
        assert_eq!(w, 1536);
        assert_eq!(h, 1024);
    }

    #[test]
    fn test_buffer_size() {
        let config = SimulatedCameraConfig::default();
        let state = SimulatedCameraState::new(config);

        // 3072 * 2048 * 2 bytes (16-bit) * 1 channel = 12,582,912
        assert_eq!(state.calculate_buffer_size(), 12_582_912);
    }

    #[test]
    fn test_stop_exposure() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        // Set exposure time via parameter (start_exposure reads from this)
        state.parameters.insert(ControlType::Exposure, 10_000_000.0); // 10 s — the exposure clock starts after frame generation, so the assertions below land well inside the window
        state.start_exposure();

        // Exposure should be in progress
        assert!(!state.is_exposure_complete());
        assert!(state.exposure_start.is_some());
        assert!(state.captured_image.is_some());

        // Stop the exposure (but keep image data)
        state.stop_exposure();

        // After stopping, exposure_start should be None
        assert!(state.exposure_start.is_none());
        // is_exposure_complete returns true when exposure_start is None
        assert!(state.is_exposure_complete());
        // Remaining time should be 0
        assert_eq!(state.get_remaining_exposure_us(), 0);
        // Image data should still be available
        assert!(state.captured_image.is_some());
        assert!(state.captured_image_metadata.is_some());
    }

    #[test]
    fn test_abort_exposure() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        // Set exposure time via parameter (start_exposure reads from this)
        state.parameters.insert(ControlType::Exposure, 10_000_000.0); // 10 s — the exposure clock starts after frame generation, so the assertions below land well inside the window
        state.start_exposure();

        // Exposure should be in progress
        assert!(!state.is_exposure_complete());
        assert!(state.exposure_start.is_some());
        assert!(state.captured_image.is_some());

        // Abort the exposure (and discard image data)
        state.abort_exposure();

        // After aborting, exposure_start should be None
        assert!(state.exposure_start.is_none());
        // is_exposure_complete returns true when exposure_start is None
        assert!(state.is_exposure_complete());
        // Remaining time should be 0
        assert_eq!(state.get_remaining_exposure_us(), 0);
        // Image data should be cleared
        assert!(state.captured_image.is_none());
        assert!(state.captured_image_metadata.is_none());
    }

    #[test]
    fn test_update_temperature_cooling() {
        let config = SimulatedCameraConfig::default().with_cooler();
        let mut state = SimulatedCameraState::new(config);

        // Set up cooling: current temp is 20C, target is 0C, PWM is max
        state.current_temperature = 20.0;
        state.target_temperature = 0.0;
        state.cooler_pwm = 255.0;

        let initial_temp = state.current_temperature;

        // Update temperature several times
        for _ in 0..10 {
            state.update_temperature();
        }

        // Temperature should have decreased
        assert!(state.current_temperature < initial_temp);
        // CurTemp parameter should be updated
        assert!(
            (state.parameters.get(&ControlType::CurTemp).unwrap() - state.current_temperature)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_update_temperature_warming() {
        let config = SimulatedCameraConfig::default().with_cooler();
        let mut state = SimulatedCameraState::new(config);

        // Camera is cold and cooler is off
        state.current_temperature = 0.0;
        state.cooler_pwm = 0.0;

        let initial_temp = state.current_temperature;

        // Update temperature several times
        for _ in 0..10 {
            state.update_temperature();
        }

        // Temperature should have increased toward ambient (20C)
        assert!(state.current_temperature > initial_temp);
        assert!(state.current_temperature <= 20.0);
    }

    #[test]
    fn test_get_channels_mono() {
        let config = SimulatedCameraConfig::default(); // Mono camera
        let state = SimulatedCameraState::new(config);

        // Mono camera should have 1 channel
        assert_eq!(state.get_channels(), 1);
    }

    #[test]
    fn test_get_channels_color_no_debayer() {
        let config = SimulatedCameraConfig::default().with_color(BayerMode::RGGB);
        let state = SimulatedCameraState::new(config);

        // Color camera with debayer disabled should return 1 channel
        assert_eq!(state.get_channels(), 1);
    }

    #[test]
    fn test_get_channels_color_debayer() {
        let config = SimulatedCameraConfig::default().with_color(BayerMode::RGGB);
        let mut state = SimulatedCameraState::new(config);

        // Enable debayer
        state.debayer_enabled = true;

        // Color camera with debayer enabled should return 3 channels
        assert_eq!(state.get_channels(), 3);
    }

    #[test]
    fn test_bytes_per_pixel_8bit() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        state.bit_depth = 8;
        assert_eq!(state.get_bytes_per_pixel(), 1);
    }

    #[test]
    fn test_bytes_per_pixel_16bit() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        state.bit_depth = 16;
        assert_eq!(state.get_bytes_per_pixel(), 2);

        // Also test intermediate values (12-bit, etc.)
        state.bit_depth = 12;
        assert_eq!(state.get_bytes_per_pixel(), 2);
    }

    #[test]
    fn test_buffer_size_with_binning_and_channels() {
        let config = SimulatedCameraConfig::default().with_color(BayerMode::RGGB);
        let mut state = SimulatedCameraState::new(config);

        // Set 2x2 binning, 8-bit mode, and enable debayer (3 channels)
        // When binning changes, ROI should be updated to binned coordinates
        state.binning = (2, 2);
        state.roi.width = 1536; // 3072 / 2
        state.roi.height = 1024; // 2048 / 2
        state.bit_depth = 8;
        state.debayer_enabled = true;

        // 1536 * 1024 * 1 byte * 3 channels = 4,718,592
        assert_eq!(state.calculate_buffer_size(), 4_718_592);
    }

    #[test]
    fn test_remaining_exposure_no_exposure_started() {
        let config = SimulatedCameraConfig::default();
        let state = SimulatedCameraState::new(config);

        // No exposure started, should return 0
        assert_eq!(state.get_remaining_exposure_us(), 0);
        // Should be considered complete
        assert!(state.is_exposure_complete());
    }

    #[test]
    fn test_start_exposure_uses_parameter() {
        let config = SimulatedCameraConfig::default();
        let mut state = SimulatedCameraState::new(config);

        // Set exposure parameter
        state.parameters.insert(ControlType::Exposure, 5_000_000.0); // 5 seconds

        state.start_exposure();

        // exposure_duration_us should be set from parameter
        assert_eq!(state.exposure_duration_us, 5_000_000);
    }
}
