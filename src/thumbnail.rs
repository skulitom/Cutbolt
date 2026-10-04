//! Small inline PNG thumbnails, so a multimodal agent can see a preview image in a tool result.
//!
//! Downscaling is exact area averaging: each output pixel is the coverage-weighted mean of the
//! source pixels under it, including partial coverage at its edges. Sums are exact integers and
//! colour is weighted by alpha, so fully transparent pixels do not tint their neighbours. Rows are
//! decoded one at a time, so memory grows with the source width rather than its area.

use crate::{Result, error};
use png::{BitDepth, ColorType};
use std::{fs::File, io::BufReader, path::Path};

/// Largest accepted source width or height. It bounds decode work and memory, and keeps every
/// exact sum below 255 * 255 * 8192 * 8192 < 2^43.
const MAX_SOURCE_EDGE: u32 = 8192;

/// Read an 8-bit RGB, RGBA, grayscale or grayscale-alpha PNG and return a PNG whose longest edge
/// is at most `max_edge`, preserving aspect ratio and never upscaling. The result is 8-bit RGB,
/// or RGBA when the source had alpha.
pub(crate) fn png(path: &Path, max_edge: u32) -> Result<Vec<u8>> {
    if max_edge == 0 {
        return Err(error(
            "INVALID_ARGUMENT",
            "Thumbnail max_edge must be at least 1",
        ));
    }
    let fail = |e: png::DecodingError| error("UNSUPPORTED_IMAGE", e.to_string());
    let mut reader = png::Decoder::new(BufReader::new(File::open(path)?))
        .read_info()
        .map_err(fail)?;
    let info = reader.info();
    if let Some(reason) = unsupported(info) {
        return Err(error(
            "UNSUPPORTED_IMAGE",
            format!(
                "Thumbnails need a non-interlaced, non-animated 8-bit RGB, RGBA, grayscale or grayscale-alpha PNG; this image uses {reason}"
            ),
        ));
    }
    let (width, height) = (info.width, info.height);
    if width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!(
                "Thumbnail source is {width}x{height}; the limit is {MAX_SOURCE_EDGE}x{MAX_SOURCE_EDGE}"
            ),
        ));
    }
    let channels = info.color_type.samples();
    let alpha = matches!(info.color_type, ColorType::GrayscaleAlpha | ColorType::Rgba);
    let (target_width, target_height) = fit(width, height, max_edge);
    let columns = spans(width, target_width);
    // Every output pixel's weights total width * height (see `spans`).
    let total = u64::from(width) * u64::from(height);
    let mut row = vec![0; width as usize * channels];
    let mut loaded = None;
    let mut sums = vec![0; row.len()];
    let mut pixels = Vec::with_capacity(
        target_width as usize * target_height as usize * if alpha { 4 } else { 3 },
    );
    for (first, weights) in spans(height, target_height) {
        sums.fill(0);
        for (index, weight) in (first..).zip(weights) {
            // Rows arrive in order; a source row straddling two output rows is reused.
            if loaded != Some(index) {
                let next = reader.next_row().map_err(fail)?.ok_or_else(|| {
                    error(
                        "UNSUPPORTED_IMAGE",
                        "PNG image data ended before its last row",
                    )
                })?;
                row.copy_from_slice(next.data());
                loaded = Some(index);
            }
            for (sum, pixel) in sums
                .chunks_exact_mut(channels)
                .zip(row.chunks_exact(channels))
            {
                accumulate(sum, pixel, weight, alpha);
            }
        }
        for (first, weights) in &columns {
            let mut pixel = [0; 4];
            for (index, &weight) in (*first..).zip(weights) {
                let source = &sums[index * channels..(index + 1) * channels];
                for (sum, &value) in pixel.iter_mut().zip(source) {
                    *sum += weight * value;
                }
            }
            push(&mut pixels, &pixel[..channels], total, alpha);
        }
    }
    reader.finish().map_err(fail)?;
    encode(target_width, target_height, alpha, &pixels)
}

/// Standard base64 with padding (RFC 4648 section 4).
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |index: usize| u32::from(chunk.get(index).copied().unwrap_or(0));
        let group = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for index in 0..4 {
            text.push(if index <= chunk.len() {
                char::from(ALPHABET[((group >> (18 - 6 * index)) & 63) as usize])
            } else {
                '='
            });
        }
    }
    text
}

fn unsupported(info: &png::Info<'_>) -> Option<&'static str> {
    [
        (
            info.bit_depth != BitDepth::Eight,
            "a bit depth other than 8",
        ),
        (info.color_type == ColorType::Indexed, "palette colour"),
        (info.interlaced, "Adam7 interlacing"),
        (info.trns.is_some(), "tRNS transparency"),
        (info.animation_control.is_some(), "APNG animation"),
        (info.width == 0 || info.height == 0, "an empty size"),
    ]
    .into_iter()
    .find_map(|(present, reason)| present.then_some(reason))
}

/// Output size: the longest edge becomes `max_edge` (rounded, at least 1), or unchanged if it fits.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = |side: u32| {
        let scaled =
            (u64::from(side) * u64::from(max_edge) + u64::from(longest) / 2) / u64::from(longest);
        scaled.max(1) as u32
    };
    (scale(width), scale(height))
}

/// For each of `target` output pixels along an axis, the first source pixel it covers and the
/// exact overlap with each covered source pixel. Lengths are in units of 1/target source pixel:
/// output `i` spans `[i * source, (i + 1) * source)` and source `j` spans
/// `[j * target, (j + 1) * target)`, so each output's weights total `source`.
fn spans(source: u32, target: u32) -> Vec<(usize, Vec<u64>)> {
    let (source, target) = (u64::from(source), u64::from(target));
    (0..target)
        .map(|output| {
            let (start, end) = (output * source, (output + 1) * source);
            let first = start / target;
            let weights = (first..end.div_ceil(target))
                .map(|index| end.min((index + 1) * target) - start.max(index * target))
                .collect();
            (first as usize, weights)
        })
        .collect()
}

/// Add one weighted source pixel: colour channels as weight * alpha * value, alpha as
/// weight * alpha. Without alpha, every channel is weight * value.
fn accumulate(sums: &mut [u64], pixel: &[u8], weight: u64, alpha: bool) {
    let (colour, coverage) = pixel.split_at(pixel.len() - usize::from(alpha));
    let coverage = coverage.first().map_or(weight, |&a| weight * u64::from(a));
    for (sum, &value) in sums.iter_mut().zip(colour) {
        *sum += coverage * u64::from(value);
    }
    if alpha {
        sums[colour.len()] += coverage;
    }
}

/// Resolve one output pixel's exact sums to rounded 8-bit RGB or RGBA samples.
fn push(pixels: &mut Vec<u8>, sums: &[u64], total: u64, alpha: bool) {
    let round = |sum: u64, weight: u64| match weight {
        0 => 0,
        weight => ((sum + weight / 2) / weight) as u8,
    };
    let (colour, coverage) = sums.split_at(sums.len() - usize::from(alpha));
    let divisor = coverage.first().copied().unwrap_or(total);
    if let [grey] = colour {
        let grey = round(*grey, divisor);
        pixels.extend([grey; 3]);
    } else {
        pixels.extend(colour.iter().map(|&sum| round(sum, divisor)));
    }
    if let Some(&coverage) = coverage.first() {
        pixels.push(round(coverage, total));
    }
}

fn encode(width: u32, height: u32, alpha: bool, pixels: &[u8]) -> Result<Vec<u8>> {
    let fail = |e: png::EncodingError| error("ENCODE_FAILED", e.to_string());
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(if alpha {
        ColorType::Rgba
    } else {
        ColorType::Rgb
    });
    encoder.set_depth(BitDepth::Eight);
    // Previews do not declare colour management; do not invent a colour tag.
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(pixels).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Cursor,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cutbolt-thumbnail-test-{}-{}.png",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            assert!(self.0.starts_with(std::env::temp_dir()));
            let _ = fs::remove_file(&self.0);
        }
    }

    fn encoded_with(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        data: &[u8],
        setup: impl FnOnce(&mut png::Encoder<'_, &mut Vec<u8>>),
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        setup(&mut encoder);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        bytes
    }

    fn encoded(width: u32, height: u32, color: ColorType, data: &[u8]) -> Vec<u8> {
        encoded_with(width, height, color, BitDepth::Eight, data, |_| {})
    }

    /// Thumbnail an encoded PNG and decode the result.
    fn thumbnail(bytes: &[u8], max_edge: u32) -> (u32, u32, ColorType, Vec<u8>) {
        let source = Temp::new(bytes);
        decode(&png(&source.0, max_edge).unwrap())
    }

    fn decode(bytes: &[u8]) -> (u32, u32, ColorType, Vec<u8>) {
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(info.bit_depth, BitDepth::Eight);
        buffer.truncate(info.buffer_size());
        (info.width, info.height, info.color_type, buffer)
    }

    fn rejected(bytes: &[u8], max_edge: u32) -> crate::Error {
        let source = Temp::new(bytes);
        png(&source.0, max_edge).unwrap_err()
    }

    #[test]
    fn solid_colour_stays_exact() {
        let data = [12, 200, 99].repeat(37 * 23);
        let (width, height, color, pixels) = thumbnail(&encoded(37, 23, ColorType::Rgb, &data), 8);
        assert_eq!((width, height, color), (8, 5, ColorType::Rgb));
        assert_eq!(pixels, [12, 200, 99].repeat(8 * 5));
    }

    #[test]
    fn checker_averages_to_one_pixel() {
        let data = [[0; 3], [255; 3], [255; 3], [0; 3]].concat();
        let (width, height, color, pixels) = thumbnail(&encoded(2, 2, ColorType::Rgb, &data), 1);
        assert_eq!((width, height, color), (1, 1, ColorType::Rgb));
        // The exact mean is 127.5; halves round up.
        assert_eq!(pixels, [128; 3]);
    }

    #[test]
    fn partial_coverage_is_weighted_and_grey_becomes_rgb() {
        // Three source pixels into two: each output takes one whole and one half source pixel.
        let (width, height, color, pixels) =
            thumbnail(&encoded(3, 1, ColorType::Grayscale, &[0, 90, 180]), 2);
        assert_eq!((width, height, color), (2, 1, ColorType::Rgb));
        assert_eq!(pixels, [30, 30, 30, 150, 150, 150]);
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        assert_eq!(fit(1920, 1080, 768), (768, 432));
        assert_eq!(fit(1080, 1920, 768), (432, 768));
        assert_eq!(fit(10_000, 1, 768), (768, 1));
        let data = vec![77; 90 * 160];
        let (width, height, _, pixels) =
            thumbnail(&encoded(90, 160, ColorType::Grayscale, &data), 16);
        assert_eq!((width, height), (9, 16));
        assert_eq!(pixels, vec![77; 9 * 16 * 3]);
    }

    #[test]
    fn small_images_are_not_upscaled() {
        let data: Vec<u8> = (0..40 * 30)
            .flat_map(|index: u32| {
                let (x, y) = (index % 40, index / 40);
                [(x * 6) as u8, (y * 8) as u8, (x ^ y) as u8]
            })
            .collect();
        let source = encoded(40, 30, ColorType::Rgb, &data);
        for max_edge in [40, 768] {
            let (width, height, color, pixels) = thumbnail(&source, max_edge);
            assert_eq!((width, height, color), (40, 30, ColorType::Rgb));
            assert_eq!(pixels, data);
        }
    }

    #[test]
    fn alpha_is_preserved() {
        let data = [200, 100, 50, 128].repeat(4 * 4);
        let (width, height, color, pixels) = thumbnail(&encoded(4, 4, ColorType::Rgba, &data), 2);
        assert_eq!((width, height, color), (2, 2, ColorType::Rgba));
        assert_eq!(pixels, [200, 100, 50, 128].repeat(2 * 2));

        // A transparent pixel lowers coverage without tinting the opaque colour.
        let data = [255, 0, 0, 255, 0, 255, 0, 0];
        let (_, _, color, pixels) = thumbnail(&encoded(2, 1, ColorType::Rgba, &data), 1);
        assert_eq!(color, ColorType::Rgba);
        assert_eq!(pixels, [255, 0, 0, 128]);

        let data = [10, 255, 30, 255, 50, 0, 70, 0];
        let (_, _, color, pixels) = thumbnail(&encoded(2, 2, ColorType::GrayscaleAlpha, &data), 1);
        assert_eq!(color, ColorType::Rgba);
        assert_eq!(pixels, [20, 20, 20, 128]);
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    #[test]
    fn unsupported_input_is_rejected() {
        let rgb = encoded(1, 1, ColorType::Rgb, &[1, 2, 3]);
        let mut interlaced = rgb.clone();
        // The encoder cannot interlace, so set the IHDR interlace flag and repair its CRC.
        assert_eq!(&interlaced[12..16], b"IHDR");
        interlaced[28] = 1;
        let crc = crc32(&interlaced[12..29]);
        interlaced[29..33].copy_from_slice(&crc.to_be_bytes());
        let cases = [
            (
                encoded_with(1, 1, ColorType::Rgb, BitDepth::Sixteen, &[0; 6], |_| {}),
                "bit depth",
            ),
            (
                encoded_with(8, 1, ColorType::Grayscale, BitDepth::One, &[0x55], |_| {}),
                "bit depth",
            ),
            (
                encoded_with(1, 1, ColorType::Indexed, BitDepth::Eight, &[1], |e| {
                    e.set_palette(vec![0, 0, 0, 255, 255, 255]);
                }),
                "palette",
            ),
            (interlaced, "interlacing"),
            (
                encoded_with(1, 1, ColorType::Rgb, BitDepth::Eight, &[1, 2, 3], |e| {
                    e.set_trns(vec![0, 1, 0, 2, 0, 3]);
                }),
                "tRNS",
            ),
            (b"not a png".to_vec(), ""),
        ];
        for (bytes, reason) in cases {
            let error = rejected(&bytes, 16);
            assert_eq!(error.code, "UNSUPPORTED_IMAGE", "{}", error.message);
            assert!(error.message.contains(reason), "{}", error.message);
        }

        let wide = encoded(MAX_SOURCE_EDGE + 1, 1, ColorType::Grayscale, &[0; 8193]);
        assert_eq!(rejected(&wide, 16).code, "LIMIT_EXCEEDED");
        assert_eq!(rejected(&rgb, 0).code, "INVALID_ARGUMENT");
    }

    /// Run with `--nocapture` to see the inline payload size for a full-HD preview.
    #[test]
    fn full_hd_preview_size() {
        let (width, height) = (1920u32, 1080u32);
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut data = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                for base in [
                    x * 255 / (width - 1),
                    y * 255 / (height - 1),
                    (x + y) * 255 / (width + height - 2),
                ] {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let noise = (state % 17) as i64 - 8;
                    data.push((i64::from(base) + noise).clamp(0, 255) as u8);
                }
            }
        }
        let source = Temp::new(&encoded(width, height, ColorType::Rgb, &data));
        let bytes = png(&source.0, 768).unwrap();
        let (thumb_width, thumb_height, color, _) = decode(&bytes);
        assert_eq!(
            (thumb_width, thumb_height, color),
            (768, 432, ColorType::Rgb)
        );
        println!(
            "1920x1080 RGB gradients + uniform noise in [-8, 8]: source PNG {} bytes, 768x432 thumbnail {} bytes, base64 {} characters",
            fs::metadata(&source.0).unwrap().len(),
            bytes.len(),
            base64(&bytes).len()
        );
    }
}
