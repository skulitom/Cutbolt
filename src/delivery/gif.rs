//! Animated GIF delivery with an exact palette for every frame.
//!
//! Each frame stores its own distinct colors in a local color table, so decoded pixels equal the
//! timeline's exactly. A frame with more than 256 colors is refused; nothing is quantized or
//! dithered. After the first, full frame, each frame stores only the rectangle that changed and
//! keeps the rest of the previous picture. The layout follows the public GIF89a specification:
//! variable-length-code LZW image data, a graphic control extension carrying each frame's delay in
//! centiseconds, and the NETSCAPE2.0 application extension for repetition.
use super::*;
use std::collections::HashSet;
use std::io::{BufWriter, Write};

/// GIF settings, only for the `gif` profile.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(deny_unknown_fields)]
pub struct Gif {
    /// Times the animation plays: omit or null to loop endlessly, 1 to play once, at most 65536.
    #[serde(default)]
    pub plays: Option<u32>,
    /// How frame times become whole-centisecond delays; omit for `exact`.
    #[serde(default)]
    pub timing: GifTiming,
}
/// GIF frame timing: `exact` gives every frame the same whole number of centiseconds (4 at 25 fps,
/// 2 at 50 fps); `nearest_centisecond` starts each frame at its exact time rounded to the nearest
/// centisecond, for 24 and 30 fps and their 1000/1001 rates.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum GifTiming {
    #[default]
    Exact,
    NearestCentisecond,
}

/// Start of output frame `n` in whole centiseconds: its exact time, rounded half up.
pub(super) fn start(rate: Time, n: u64) -> u64 {
    (200 * n * rate.den + rate.num) / (2 * rate.num)
}
/// Length of `frames` output frames, the sum of their delays.
pub(super) fn length(rate: Time, frames: u64) -> Result<Time> {
    Time::new(start(rate, frames), 100)
}
fn rate_text(rate: Time) -> String {
    if rate.den == 1 {
        rate.num.to_string()
    } else {
        format!("{}/{}", rate.num, rate.den)
    }
}
impl Gif {
    /// The NETSCAPE2.0 repeat count after the first play, or None to write no extension.
    fn repeats(self) -> Option<u16> {
        match self.plays {
            None => Some(0),
            Some(1) => None,
            Some(plays) => Some((plays - 1) as u16),
        }
    }
    pub(super) fn validate(self, rate: Time) -> Result<()> {
        if self
            .plays
            .is_some_and(|plays| !(1..=65536).contains(&plays))
        {
            return Err(invalid(
                "gif.plays must be 1..65536; omit it to loop endlessly",
            ));
        }
        // Browsers play delays under 2 cs as 10 cs, so a GIF frame lasts at least 1/50 s.
        if 100 * rate.den < 2 * rate.num {
            return Err(invalid(&format!(
                "GIF delays are whole centiseconds, and viewers stretch delays under 2 cs to 10 cs, so a GIF holds at most 50 fps; this timeline runs at {} fps. Make the animation at 25 or 50 fps, or export png_sequence",
                rate_text(rate)
            )));
        }
        if self.timing == GifTiming::Exact && !(100 * rate.den).is_multiple_of(rate.num) {
            let delay = Time::new(100 * rate.den, rate.num)?;
            let shortest = 100 * rate.den / rate.num;
            return Err(invalid(&format!(
                "GIF delays are whole centiseconds, and a {} fps frame lasts {}/{} cs; 25 and 50 fps are exact. Pass gif.timing \"nearest_centisecond\" to start every frame within half a centisecond of its time, with delays of {} and {} cs, or make the animation at 25 or 50 fps",
                rate_text(rate),
                delay.num,
                delay.den,
                shortest,
                shortest + 1
            )));
        }
        Ok(())
    }
    /// The receipt's video settings for `frames` frames at `rate`.
    pub(super) fn report(self, rate: Time, frames: u64) -> Value {
        let delays = (0..frames).map(|n| start(rate, n + 1) - start(rate, n));
        let (shortest, longest) = delays.fold((u64::MAX, 0), |(lo, hi), d| (lo.min(d), hi.max(d)));
        // The largest distance between a frame's start and its exact time, in seconds.
        let error = (0..frames)
            .map(|n| (start(rate, n) * rate.num).abs_diff(100 * n * rate.den))
            .max()
            .unwrap_or(0);
        json!({"codec":"gif","pixel_format":"pal8","palette":"exact_local_table_per_frame","maximum_colors_per_frame":256,
            "alpha":"opaque","color":"encoded_values_preserved","frames":"changed_rectangle_over_previous_picture",
            "plays":self.plays,"netscape_repeat_count":self.repeats(),"timing":self.timing,
            "delay_centiseconds":{"minimum":shortest,"maximum":longest},"duration_centiseconds":start(rate,frames),
            "maximum_start_error":Time::new(error,100*rate.num).ok()})
    }
}

const SLOTS: usize = 1024;
const EMPTY: u32 = u32::MAX;
/// Up to 256 distinct packed RGB colors in an open-addressed table, each with a palette index.
struct Palette {
    keys: Vec<u32>,
    indices: Vec<u8>,
    colors: Vec<u32>,
}
impl Palette {
    fn new() -> Self {
        Self {
            keys: vec![EMPTY; SLOTS],
            indices: vec![0; SLOTS],
            colors: Vec::with_capacity(256),
        }
    }
    fn clear(&mut self) {
        self.keys.fill(EMPTY);
        self.colors.clear();
    }
    fn slot(&self, color: u32) -> usize {
        let mut slot = (color.wrapping_mul(0x9E37_79B1) >> 22) as usize;
        while self.keys[slot] != color && self.keys[slot] != EMPTY {
            slot = (slot + 1) % SLOTS;
        }
        slot
    }
    /// Add `color`; false when it would be the 257th.
    fn insert(&mut self, color: u32) -> bool {
        let slot = self.slot(color);
        if self.keys[slot] == EMPTY {
            if self.colors.len() == 256 {
                return false;
            }
            self.keys[slot] = color;
            self.colors.push(color);
        }
        true
    }
    /// Number the colors in ascending order and return them.
    fn sort(&mut self) -> &[u32] {
        self.colors.sort_unstable();
        for (index, &color) in self.colors.iter().enumerate() {
            let slot = self.slot(color);
            self.indices[slot] = index as u8;
        }
        &self.colors
    }
    fn index(&self, color: u32) -> u8 {
        self.indices[self.slot(color)]
    }
}
fn rgb(pixel: &[u8]) -> u32 {
    u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2])
}
/// Collect the colors of `rows` (packed RGB24); false past 256.
fn collect<'a>(palette: &mut Palette, rows: impl Iterator<Item = &'a [u8]>) -> bool {
    palette.clear();
    let mut last = EMPTY;
    for row in rows {
        for pixel in row.chunks_exact(3) {
            let color = rgb(pixel);
            if color != last {
                if !palette.insert(color) {
                    return false;
                }
                last = color;
            }
        }
    }
    true
}

/// A frame's rectangle: left, top, width and height in pixels.
type Rectangle = (usize, usize, usize, usize);
/// The smallest rectangle holding every pixel of `current` that differs from `previous`, or one
/// pixel when nothing changed (a GIF image holds at least one).
fn changed(previous: &[u8], current: &[u8], width: usize, height: usize) -> Rectangle {
    let row = width * 3;
    fn line(frame: &[u8], row: usize, y: usize) -> &[u8] {
        &frame[y * row..(y + 1) * row]
    }
    let differs = |y: usize| line(previous, row, y) != line(current, row, y);
    let Some(top) = (0..height).find(|&y| differs(y)) else {
        return (0, 0, 1, 1);
    };
    let bottom = (top..height).rfind(|&y| differs(y)).expect("a changed row") + 1;
    let (mut left, mut right) = (width, 0);
    for y in top..bottom {
        let (a, b) = (line(previous, row, y), line(current, row, y));
        let pixel = |x: usize| a[x * 3..x * 3 + 3] != b[x * 3..x * 3 + 3];
        if let Some(x) = (0..left).find(|&x| pixel(x)) {
            left = x;
        }
        if let Some(x) = (right..width).rfind(|&x| pixel(x)) {
            right = x + 1;
        }
    }
    (left, top, right - left, bottom - top)
}
fn rows(frame: &[u8], width: usize, rectangle: Rectangle) -> impl Iterator<Item = &[u8]> {
    let (left, top, w, h) = rectangle;
    (top..top + h).map(move |y| &frame[(y * width + left) * 3..(y * width + left + w) * 3])
}

/// Codes packed least significant bit first.
struct Bits<'a> {
    out: &'a mut Vec<u8>,
    value: u32,
    count: u8,
}
impl Bits<'_> {
    fn put(&mut self, code: u16, size: u8) {
        self.value |= u32::from(code) << self.count;
        self.count += size;
        while self.count >= 8 {
            self.out.push(self.value as u8);
            self.value >>= 8;
            self.count -= 8;
        }
    }
    fn flush(self) {
        if self.count > 0 {
            self.out.push(self.value as u8);
        }
    }
}
/// GIF's LZW compression: codes grow from `minimum + 1` to 12 bits, and the table is cleared when
/// it holds 4096 codes.
struct Lzw {
    /// The code of string `code` followed by palette index `index`, at `code * 256 + index`; 0 when
    /// absent (no string has code 0 after the clear code).
    child: Vec<u16>,
    used: Vec<usize>,
}
impl Lzw {
    fn new() -> Self {
        Self {
            child: vec![0; 4096 * 256],
            used: Vec::with_capacity(4096),
        }
    }
    fn reset(&mut self) {
        for slot in self.used.drain(..) {
            self.child[slot] = 0;
        }
    }
    fn compress(&mut self, minimum: u8, indices: &[u8], out: &mut Vec<u8>) {
        let clear = 1u16 << minimum;
        let (mut size, mut next) = (minimum + 1, clear + 2);
        let mut bits = Bits {
            out,
            value: 0,
            count: 0,
        };
        self.reset();
        bits.put(clear, size);
        let mut prefix = u16::from(indices[0]);
        for &index in &indices[1..] {
            let slot = usize::from(prefix) * 256 + usize::from(index);
            if self.child[slot] != 0 {
                prefix = self.child[slot];
                continue;
            }
            bits.put(prefix, size);
            // A decoder adds this string when it reads the next code, so codes grow when the next
            // one could reach 1 << size.
            if next < 4096 {
                if next >= 1 << size {
                    size += 1;
                }
                self.child[slot] = next;
                self.used.push(slot);
                next += 1;
            } else {
                bits.put(clear, size);
                self.reset();
                (size, next) = (minimum + 1, clear + 2);
            }
            prefix = u16::from(index);
        }
        bits.put(prefix, size);
        if next >= 1 << size && size < 12 {
            size += 1;
        }
        bits.put(clear + 1, size);
        bits.flush();
    }
}

struct Writer {
    out: BufWriter<File>,
    lzw: Lzw,
    data: Vec<u8>,
    indices: Vec<u8>,
}
impl Writer {
    fn create(path: &Path, width: u32, height: u32, repeats: Option<u16>) -> Result<Self> {
        let file = File::options().write(true).create_new(true).open(path)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        out.write_all(b"GIF89a")?;
        out.write_all(&(width as u16).to_le_bytes())?;
        out.write_all(&(height as u16).to_le_bytes())?;
        // No global color table, 8-bit color resolution, background 0, square pixels.
        out.write_all(&[0x70, 0, 0])?;
        if let Some(count) = repeats {
            out.write_all(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01")?;
            out.write_all(&count.to_le_bytes())?;
            out.write_all(&[0])?;
        }
        Ok(Self {
            out,
            lzw: Lzw::new(),
            data: Vec::new(),
            indices: Vec::new(),
        })
    }
    /// One image over the previous picture, with its delay and a local table of `colors`.
    fn frame(&mut self, delay: u16, rectangle: Rectangle, colors: &[u32]) -> Result<()> {
        let (left, top, width, height) = rectangle;
        let bits = (usize::BITS - (colors.len() - 1).leading_zeros()).max(1) as u8;
        // Keep the picture for the next frame (disposal 1); no transparency.
        self.out.write_all(&[0x21, 0xF9, 4, 0x04])?;
        self.out.write_all(&delay.to_le_bytes())?;
        self.out.write_all(&[0, 0, 0x2C])?;
        for value in [left, top, width, height] {
            self.out.write_all(&(value as u16).to_le_bytes())?;
        }
        self.out.write_all(&[0x80 | (bits - 1)])?;
        for entry in 0..1usize << bits {
            let color = colors.get(entry).copied().unwrap_or(0);
            self.out
                .write_all(&[(color >> 16) as u8, (color >> 8) as u8, color as u8])?;
        }
        let minimum = bits.max(2);
        self.out.write_all(&[minimum])?;
        self.data.clear();
        self.lzw.compress(minimum, &self.indices, &mut self.data);
        for block in self.data.chunks(255) {
            self.out.write_all(&[block.len() as u8])?;
            self.out.write_all(block)?;
        }
        self.out.write_all(&[0])?;
        Ok(())
    }
    fn finish(mut self) -> Result<()> {
        self.out.write_all(&[0x3B])?;
        let file = self
            .out
            .into_inner()
            .map_err(|e| crate::Error::from(e.into_error()))?;
        file.sync_all()?;
        Ok(())
    }
}

/// Encode the lossless `reference` as `encoded`, frame by frame, and return what was measured:
/// colors per frame and distinct colors over the animation. A frame with more than 256 colors
/// fails the export after the remaining frames are counted.
pub(super) fn encode(
    request: &Export,
    c: &Checked,
    reference: &Path,
    encoded: &Path,
    control: &dyn media::Control,
) -> Result<Value> {
    let settings = request.gif.unwrap_or_default();
    let rate = c.project.frame_rate;
    let (width, height) = (c.project.width as usize, c.project.height as usize);
    let frames = c.reference.frames;
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(reference.to_string_lossy().into_owned());
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ]
        .map(str::to_owned),
    );
    let pixels = (width * height) as u64;
    let timeout = Duration::from_secs(600) + Duration::from_millis(frames * pixels / 2_000);
    control.phase("encoding")?;
    control.total(frames);
    let mut decoder = media::StreamReader::spawn(&media::tool("ffmpeg"), &args, timeout)?;
    let mut writer = Writer::create(
        encoded,
        c.project.width,
        c.project.height,
        settings.repeats(),
    )?;
    let (mut previous, mut current) = (vec![0; width * height * 3], vec![0; width * height * 3]);
    let mut palette = Palette::new();
    let (mut fewest, mut most) = (usize::MAX, 0);
    let mut distinct = HashSet::new();
    // The first frame over 256 colors with its exact count, and how many frames exceed it.
    let mut refused: Option<(u64, usize)> = None;
    let mut exceeding = 0;
    for n in 0..frames {
        decoder.read_exact(&mut current)?;
        if !collect(&mut palette, std::iter::once(&current[..])) {
            exceeding += 1;
            if refused.is_none() {
                let colors: HashSet<u32> = current.chunks_exact(3).map(rgb).collect();
                refused = Some((n, colors.len()));
            }
            continue;
        }
        if refused.is_some() {
            continue;
        }
        fewest = fewest.min(palette.colors.len());
        most = most.max(palette.colors.len());
        distinct.extend(palette.colors.iter().copied());
        let rectangle = if n == 0 {
            (0, 0, width, height)
        } else {
            changed(&previous, &current, width, height)
        };
        // The palette holds the whole frame's colors; a smaller rectangle may need fewer.
        if rectangle != (0, 0, width, height) {
            collect(&mut palette, rows(&current, width, rectangle));
        }
        palette.sort();
        writer.indices.clear();
        let mut last = (EMPTY, 0);
        for row in rows(&current, width, rectangle) {
            for pixel in row.chunks_exact(3) {
                let color = rgb(pixel);
                if color != last.0 {
                    last = (color, palette.index(color));
                }
                writer.indices.push(last.1);
            }
        }
        let delay = (start(rate, n + 1) - start(rate, n)) as u16;
        writer.frame(delay, rectangle, &palette.colors)?;
        std::mem::swap(&mut previous, &mut current);
        control.frames(n + 1)?;
    }
    decoder.finish()?;
    if let Some((n, colors)) = refused {
        let first = c.range.start.units(rate)?;
        return Err(error(
            "TOO_MANY_COLORS",
            format!(
                "Frame {n} of the range (timeline frame {}) has {colors} distinct colors, and a GIF frame holds at most 256 exact colors; {exceeding} of {frames} frames exceed it. Nothing is quantized: reduce the colors in the timeline, or export png_sequence or png_mov",
                first + n
            ),
        ));
    }
    writer.finish()?;
    Ok(
        json!({"colors_per_frame":{"minimum":fewest,"maximum":most},"distinct_colors":distinct.len()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An independent reading of GIF's LZW: the decoder adds the previous string extended by the
    /// current one's first index, and widens its codes when the next code reaches 1 << size.
    fn decompress(minimum: u8, data: &[u8]) -> Vec<u8> {
        let clear = 1usize << minimum;
        let (mut size, mut position) = (minimum as usize + 1, 0usize);
        let mut table: Vec<Vec<u8>> = Vec::new();
        let mut previous: Option<usize> = None;
        let mut out = Vec::new();
        loop {
            let mut code = 0;
            for bit in 0..size {
                let at = position + bit;
                code |= ((data[at / 8] >> (at % 8)) as usize & 1) << bit;
            }
            position += size;
            if code == clear {
                table = (0..clear).map(|i| vec![i as u8]).collect();
                table.extend([Vec::new(), Vec::new()]);
                size = minimum as usize + 1;
                previous = None;
                continue;
            }
            if code == clear + 1 {
                return out;
            }
            let entry = match previous {
                None => table[code].clone(),
                Some(p) => {
                    let entry = if code < table.len() {
                        table[code].clone()
                    } else {
                        assert_eq!(code, table.len(), "code beyond the table");
                        let mut e = table[p].clone();
                        e.push(table[p][0]);
                        e
                    };
                    let mut added = table[p].clone();
                    added.push(entry[0]);
                    if table.len() < 4096 {
                        table.push(added);
                        if table.len() == 1 << size && size < 12 {
                            size += 1;
                        }
                    }
                    entry
                }
            };
            out.extend_from_slice(&entry);
            previous = Some(code);
        }
    }

    #[test]
    fn lzw_round_trips_every_code_size_and_table_reset() {
        let mut lzw = Lzw::new();
        let mut seed = 12345u32;
        for bits in 1..=8u8 {
            let colors = 1u32 << bits;
            for length in [1, 2, 3, 255, 4096, 70_000] {
                // Noise fills the table and forces clears; runs exercise long strings.
                let indices: Vec<u8> = (0..length)
                    .map(|i| {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        if i % 7 < 3 {
                            (i / 50 % colors as usize) as u8
                        } else {
                            ((seed >> 16) % colors) as u8
                        }
                    })
                    .collect();
                let minimum = bits.max(2);
                let mut data = Vec::new();
                lzw.compress(minimum, &indices, &mut data);
                assert_eq!(decompress(minimum, &data), indices, "{bits} bits, {length}");
            }
        }
        // One repeated index: KwKwK strings, each code the one just added.
        let mut data = Vec::new();
        lzw.compress(2, &[3; 100_000], &mut data);
        assert_eq!(decompress(2, &data), vec![3; 100_000]);
    }

    #[test]
    fn palettes_hold_256_exact_colors_in_ascending_order() {
        let mut palette = Palette::new();
        let pixels: Vec<u8> = (0..256u32)
            .rev()
            .flat_map(|i| [(i * 7) as u8, i as u8, 255 - i as u8])
            .collect();
        assert!(collect(&mut palette, std::iter::once(&pixels[..])));
        assert_eq!(palette.colors.len(), 256);
        let sorted = palette.sort().to_vec();
        assert!(sorted.windows(2).all(|w| w[0] < w[1]));
        for (index, &color) in sorted.iter().enumerate() {
            assert_eq!(palette.index(color), index as u8);
        }
        let mut more = pixels.clone();
        more.extend([1, 2, 3]);
        assert!(!collect(&mut palette, std::iter::once(&more[..])));
    }

    #[test]
    fn frames_store_the_rectangle_that_changed() {
        let (w, h) = (6, 4);
        let before = vec![9u8; w * h * 3];
        assert_eq!(changed(&before, &before, w, h), (0, 0, 1, 1));
        let mut after = before.clone();
        after[(w + 4) * 3 + 1] = 0; // (4, 1)
        after[(2 * w + 2) * 3] = 0; // (2, 2)
        assert_eq!(changed(&before, &after, w, h), (2, 1, 3, 2));
        let picked: Vec<u8> = rows(&after, w, (2, 1, 3, 2)).flatten().copied().collect();
        assert_eq!(picked.len(), 3 * 2 * 3);
        assert_eq!(picked[2 * 3 + 1], 0);
        assert_eq!(picked[3 * 3], 0);
    }

    #[test]
    fn delays_are_exact_at_25_and_50_fps_or_nearest_when_asked() {
        let rate = |n, d| Time::new(n, d).unwrap();
        assert_eq!(
            (0..4).map(|n| start(rate(25, 1), n)).collect::<Vec<_>>(),
            [0, 4, 8, 12]
        );
        assert_eq!(start(rate(50, 1), 3), 6);
        // 30 fps: 0, 3.33, 6.67, 10 cs.
        assert_eq!(
            (0..4).map(|n| start(rate(30, 1), n)).collect::<Vec<_>>(),
            [0, 3, 7, 10]
        );
        // 24 fps: 12.5 cs rounds half up.
        assert_eq!(start(rate(24, 1), 3), 13);
        assert_eq!(length(rate(30000, 1001), 30000).unwrap(), rate(1001, 1));
        let exact = Gif::default();
        let nearest = Gif {
            timing: GifTiming::NearestCentisecond,
            ..Gif::default()
        };
        for (n, d) in [(25, 1), (50, 1)] {
            exact.validate(rate(n, d)).unwrap();
        }
        for (n, d) in [(24, 1), (30, 1), (24000, 1001), (30000, 1001)] {
            assert!(exact.validate(rate(n, d)).is_err());
            nearest.validate(rate(n, d)).unwrap();
        }
        for (n, d) in [(60, 1), (60000, 1001)] {
            assert!(nearest.validate(rate(n, d)).is_err());
        }
        let report = nearest.report(rate(30, 1), 30);
        assert_eq!(
            report["delay_centiseconds"],
            json!({"minimum":3,"maximum":4})
        );
        assert_eq!(report["duration_centiseconds"], 100);
        assert_eq!(report["maximum_start_error"], json!({"num":1,"den":300}));
        assert_eq!(
            exact.report(rate(25, 1), 3)["maximum_start_error"],
            json!({"num":0,"den":1})
        );
    }

    #[test]
    fn plays_map_to_the_netscape_repeat_count() {
        let plays = |plays| Gif {
            plays,
            ..Gif::default()
        };
        assert_eq!(plays(None).repeats(), Some(0));
        assert_eq!(plays(Some(1)).repeats(), None);
        assert_eq!(plays(Some(3)).repeats(), Some(2));
        assert_eq!(plays(Some(65536)).repeats(), Some(65535));
        let rate = Time::new(25, 1).unwrap();
        assert!(plays(Some(0)).validate(rate).is_err());
        assert!(plays(Some(65537)).validate(rate).is_err());
    }
}
