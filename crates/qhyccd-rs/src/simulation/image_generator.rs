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

#[cfg(test)]
mod tests {
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
