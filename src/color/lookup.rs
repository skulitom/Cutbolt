//! Exact byte lookup for progressive 8-bit 4:2:0, retaining `Input::convert` as the f64 oracle.
use super::{Input, Matrix, Range, Transfer, invalid};
use crate::Result;
use std::sync::{Arc, OnceLock};

type Pixels = Arc<[[u8; 3]]>;
static TABLES: [OnceLock<Pixels>; 6] = [const { OnceLock::new() }; 6];

/// A reusable native YUV420-to-packed-RGB converter. Construction lazily builds an exact
/// 48 MiB table for the range/transfer pair, shared by subsequent converters in this process.
/// The two equal-transfer declarations share a table; at most six tables (288 MiB) can exist.
/// Chroma reconstruction is the same nearest 2x2 block interpretation as SDR conform.
#[derive(Clone)]
pub struct Yuv420Converter {
    pixels: Pixels,
}

impl Yuv420Converter {
    /// Interpret BT.709 YUV bytes using `input` and emit encoded RGB in `output`.
    /// This does not validate media tags; the caller must first validate the source declaration.
    pub fn new(input: Input, output: Transfer) -> Result<Self> {
        if input.matrix != Matrix::Bt709 {
            return Err(invalid("YUV420 lookup requires the BT.709 matrix"));
        }
        let conversion = match (input.transfer, output) {
            (Transfer::Bt709, Transfer::Srgb) => 1,
            (Transfer::Srgb, Transfer::Bt709) => 2,
            _ => 0,
        };
        let slot = usize::from(input.range == Range::Limited) * 3 + conversion;
        Ok(Self {
            pixels: TABLES[slot].get_or_init(|| build(input, output)).clone(),
        })
    }

    /// Convert exactly one native planar Y/U/V frame to packed R/G/B. Dimensions must be
    /// positive and even; both buffers must have exactly the corresponding frame byte counts.
    pub fn convert(&self, width: u32, height: u32, source: &[u8], rgb: &mut [u8]) -> Result<()> {
        let size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3));
        if width == 0
            || height == 0
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || size.is_none_or(|n| rgb.len() != n || source.len() != n / 2)
        {
            return Err(invalid(
                "YUV420 lookup requires positive even dimensions and exact YUV/RGB frame buffers",
            ));
        }
        let width = width as usize;
        let n = width * height as usize;
        for (y, row) in rgb.chunks_exact_mut(width * 3).enumerate() {
            let chroma_row = y / 2 * (width / 2);
            for (x, pixel) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let chroma = chroma_row + x / 2;
                let index = ((source[y * width + x] as usize) << 16)
                    | ((source[n + chroma] as usize) << 8)
                    | source[n + n / 4 + chroma] as usize;
                pixel.copy_from_slice(&self.pixels[index]);
            }
        }
        Ok(())
    }
}

fn build(input: Input, output: Transfer) -> Pixels {
    let (offset, yscale, cscale) = if input.range == Range::Limited {
        (16.0, 219.0, 224.0)
    } else {
        (0.0, 255.0, 255.0)
    };
    let mut pixels = Vec::with_capacity(1 << 24);
    for y in 0..256 {
        let y = (y as f64 - offset) / yscale;
        // Reuse red/blue values only where their operands are identical. Keep the f64
        // expression order, clipping and rounding of Input::convert, including out-of-range codes.
        let red: [f64; 256] = std::array::from_fn(|v| y + 1.5748 * ((v as f64 - 128.0) / cscale));
        let red_byte = red.map(|r| input.byte(r, output));
        for u in 0..256 {
            let b = y + 1.8556 * ((u as f64 - 128.0) / cscale);
            let blue_byte = input.byte(b, output);
            for v in 0..256 {
                let g = (y - 0.2126 * red[v] - 0.0722 * b) / 0.7152;
                pixels.push([red_byte[v], input.byte(g, output), blue_byte]);
            }
        }
    }
    pixels.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{Layout, MissingTags};

    #[test]
    fn every_yuv_byte_triple_matches_f64_for_every_declaration() {
        const N: usize = 256 * 256;
        let mut native = vec![0; N * 3];
        for i in 0..N {
            native[N + i] = (i >> 8) as u8;
            native[2 * N + i] = i as u8;
        }
        let mut expected = vec![0; N * 3];
        // Equal transfers share storage but each declared pair is compared with the old oracle.
        for range in [Range::Full, Range::Limited] {
            for transfer in [Transfer::Srgb, Transfer::Bt709] {
                for output in [Transfer::Srgb, Transfer::Bt709] {
                    let input = Input {
                        matrix: Matrix::Bt709,
                        range,
                        transfer,
                        missing_tags: MissingTags::Reject,
                    };
                    let converter = Yuv420Converter::new(input, output).unwrap();
                    assert_eq!(converter.pixels.len() * 3, 48 * 1024 * 1024);
                    for y in 0..256 {
                        native[..N].fill(y as u8);
                        // Unchanged f64 converter; YUV444 supplies every U/V pair at this luma.
                        input.convert(Layout::Yuv444, 256, 256, &native, output, &mut expected);
                        for (i, pixel) in expected.as_chunks::<3>().0.iter().enumerate() {
                            assert_eq!(
                                &converter.pixels[y * N + i],
                                pixel,
                                "Y={y}, U={}, V={}, {range:?}, {transfer:?} -> {output:?}",
                                i >> 8,
                                i & 255
                            );
                        }
                    }
                    let again = Yuv420Converter::new(input, output).unwrap();
                    assert!(Arc::ptr_eq(&converter.pixels, &again.pixels));
                    if transfer == output {
                        let other = if transfer == Transfer::Srgb {
                            Transfer::Bt709
                        } else {
                            Transfer::Srgb
                        };
                        let same = Yuv420Converter::new(
                            Input {
                                transfer: other,
                                ..input
                            },
                            other,
                        )
                        .unwrap();
                        assert!(Arc::ptr_eq(&converter.pixels, &same.pixels));
                    }
                }
            }
        }
    }

    #[test]
    fn native_420_planes_match_reference_and_wrong_buffers_are_rejected() {
        let input = Input {
            matrix: Matrix::Bt709,
            range: Range::Limited,
            transfer: Transfer::Bt709,
            missing_tags: MissingTags::Reject,
        };
        let converter = Yuv420Converter::new(input, Transfer::Srgb).unwrap();
        let native: Vec<u8> = (0..12 * 8 * 3 / 2)
            .map(|n| ((n * 37 + 91) % 256) as u8)
            .collect();
        let mut reference = vec![0; 12 * 8 * 3];
        let mut actual = reference.clone();
        input.convert(
            Layout::Yuv420,
            12,
            8,
            &native,
            Transfer::Srgb,
            &mut reference,
        );
        converter.convert(12, 8, &native, &mut actual).unwrap();
        assert_eq!(actual, reference);
        for (w, h) in [
            (0, 8),
            (12, 0),
            (11, 8),
            (12, 7),
            (u32::MAX - 1, u32::MAX - 1),
        ] {
            assert!(converter.convert(w, h, &native, &mut actual).is_err());
        }
        assert!(
            converter
                .convert(12, 8, &native[..native.len() - 1], &mut actual)
                .is_err()
        );
        assert!(converter.convert(12, 8, &native, &mut actual[..3]).is_err());
        assert!(
            Yuv420Converter::new(
                Input {
                    matrix: Matrix::Rgb,
                    ..input
                },
                Transfer::Srgb
            )
            .is_err()
        );
    }
}
