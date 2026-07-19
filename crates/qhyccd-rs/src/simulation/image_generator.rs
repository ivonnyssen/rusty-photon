//! Image generation utilities for simulated cameras

use rand::{Rng, RngExt};
use rayon::prelude::*;

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
