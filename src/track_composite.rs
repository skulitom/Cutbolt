//! Native timeline decoder streams, exact transitions and bounded encoder transport.
use crate::{
    Result, error,
    media::{self, Control, StreamReader, Watch},
    tracks::TransitionKind,
};
use serde::Serialize;
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// The ordered native picture stream and its shared rendering resources.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Compositor {
    pub stack: crate::track_stack::Plan,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    #[serde(skip)]
    pub effect_resources: crate::clip_effects::Resources,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_report: Option<serde_json::Value>,
}
impl Compositor {
    fn plane(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// All commands are prepared on the controlling thread; only decoding moves to workers.
pub(crate) enum PreparedReader {
    Tool(Command),
    Direct(crate::direct::PreparedPicture),
}
impl PreparedReader {
    pub(crate) fn prepare(
        arguments: &[String],
        direct: Option<&crate::direct::Picture>,
        program: &str,
        control: &dyn Control,
    ) -> Result<Self> {
        match direct {
            Some(picture) => Ok(Self::Direct(picture.prepare(control)?)),
            None => Ok(Self::Tool(control.command(program, arguments)?)),
        }
    }
    pub(crate) fn start(self, program: &str, timeout: Duration, watch: Watch) -> Result<Reader> {
        match self {
            Self::Tool(command) => Ok(Reader::Tool(StreamReader::spawn_with(
                command, program, timeout, watch,
            )?)),
            Self::Direct(picture) => Ok(Reader::Direct(picture.start(watch, timeout)?)),
        }
    }
}
pub(crate) enum Reader {
    Tool(StreamReader),
    Direct(crate::direct::PlanarFrames),
}
impl Reader {
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        match self {
            Self::Tool(r) => r.read_exact(bytes),
            Self::Direct(r) => r.read_exact(bytes),
        }
    }
    fn finish(self) -> Result<()> {
        match self {
            Self::Tool(r) => r.finish(),
            Self::Direct(r) => r.finish(),
        }
    }
}

/// A decoder whose frames are read on their own thread, up to two ahead, so the pipe transfers
/// of all decoders overlap one another and the composition. Read frames can be handed back for
/// reuse.
pub(crate) struct Ahead {
    frames: mpsc::Receiver<Result<Vec<u8>>>,
    spare: mpsc::Sender<Vec<u8>>,
}
impl Ahead {
    /// Read `count` frames of `size` bytes, then require the decoder to end cleanly; its result
    /// is the last message. The thread stops when the receiver is dropped, and the decoder with it.
    pub(crate) fn start(mut reader: Reader, size: usize, count: u64) -> Self {
        let (sender, frames) = mpsc::sync_channel(2);
        let (spare, spares) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for _ in 0..count {
                let mut frame = spares.try_recv().unwrap_or_else(|_| vec![0; size]);
                let read = reader.read_exact(&mut frame).map(|()| frame);
                let failed = read.is_err();
                if sender.send(read).is_err() || failed {
                    return;
                }
            }
            let _ = sender.send(reader.finish().map(|()| Vec::new()));
        });
        Self { frames, spare }
    }
    pub(crate) fn next(&self) -> Result<Vec<u8>> {
        self.frames.recv().map_err(|_| {
            error(
                "RENDER_VALIDATION_FAILED",
                "A composition decoder stopped early",
            )
        })?
    }
    pub(crate) fn recycle(&self, frame: Vec<u8>) {
        let _ = self.spare.send(frame);
    }
    /// The decoder's own end, after its last frame.
    pub(crate) fn finish(self) -> Result<()> {
        self.next().map(|_| ())
    }
}
/// Decode, composite and write every frame to an encoder reading raw planar G, B, R on stdin.
/// Decoding and compositing run on one thread while this one feeds the encoder and reports
/// progress (after `done` earlier frames), so the decoders, the compositor and the encoder work at
/// the same time.
pub(crate) fn encode(
    compositor: &Compositor,
    encoder: &[String],
    timeout: Duration,
    done: u64,
    control: &dyn Control,
) -> Result<()> {
    let discard = |mut stdout: std::process::ChildStdout| {
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        Ok(())
    };
    encode_consuming(compositor, encoder, timeout, done, control, discard)
}

/// `encode` for an encoder that also writes a stream to stdout, which `consume` reads to its end.
pub(crate) fn encode_consuming<T: Send + 'static>(
    compositor: &Compositor,
    encoder: &[String],
    timeout: Duration,
    done: u64,
    control: &dyn Control,
    consume: impl FnOnce(std::process::ChildStdout) -> Result<T> + Send + 'static,
) -> Result<T> {
    let program = control.tool("ffmpeg");
    let abort = Arc::new(AtomicBool::new(false));
    let mut decoders =
        crate::track_stack::Frames::prepare(&compositor.stack, timeout, control, &abort)?;
    let mut composition_error = None;
    let stopped = media::feed_stdin_consuming(
        control.command(&program, encoder)?,
        &program,
        timeout,
        Watch {
            abort: Some(abort.clone()),
        },
        consume,
        |stdin| {
            std::thread::scope(|scope| {
                let (sender, receiver) = mpsc::sync_channel::<Result<Vec<u8>>>(2);
                let frames = compositor.frames;
                let producer = scope.spawn(move || {
                    for n in 0..frames {
                        let frame = decoders.next(n);
                        let failed = frame.is_err();
                        if sender.send(frame).is_err() || failed {
                            return;
                        }
                    }
                    let finished = decoders.finish();
                    if finished.is_err() {
                        let _ = sender.send(finished.map(|()| Vec::new()));
                    }
                });
                let outcome = (|| {
                    let mut written = 0;
                    let mut reported = Instant::now();
                    for frame in receiver.iter() {
                        let frame = frame.inspect_err(|e| composition_error = Some(e.clone()))?;
                        if written == frames {
                            break;
                        }
                        stdin.write_all(&frame)?;
                        written += 1;
                        if reported.elapsed() >= Duration::from_millis(250) {
                            control.frames(done + written)?;
                            reported = Instant::now();
                        } else {
                            control.check()?;
                        }
                    }
                    if written != frames {
                        return Err(error(
                            "RENDER_VALIDATION_FAILED",
                            "Overlay composition ended before the last frame",
                        ));
                    }
                    control.frames(done + written)
                })();
                if let Err(e) = &outcome {
                    // Preserve the initiating decoder/control failure across shutdown. A stdin
                    // write error alone may instead be explained by the encoder's diagnostics.
                    if composition_error.is_none() && e.code != "IO_ERROR" {
                        composition_error = Some(e.clone());
                    }
                    // Stop every decoder so the producer cannot block on a pipe.
                    abort.store(true, Ordering::SeqCst);
                }
                drop(receiver);
                let _ = producer.join();
                outcome
            })
        },
    );
    abort.store(true, Ordering::SeqCst);
    match composition_error {
        Some(error) => Err(error),
        None => stopped,
    }
}

/// The single composited frame of a one-frame window, as packed RGB.
pub(crate) fn frame_rgb(
    compositor: &Compositor,
    timeout: Duration,
    control: &dyn Control,
) -> Result<Vec<u8>> {
    let abort = Arc::new(AtomicBool::new(false));
    let outcome = (|| -> Result<Vec<u8>> {
        let mut decoders =
            crate::track_stack::Frames::prepare(&compositor.stack, timeout, control, &abort)?;
        let planar = decoders.next(0)?;
        decoders.finish()?;
        Ok(planar)
    })();
    abort.store(true, Ordering::SeqCst);
    let planar = outcome?;
    let plane = compositor.plane();
    let (g, rest) = planar.split_at(plane);
    let (b, r) = rest.split_at(plane);
    let mut rgb = Vec::with_capacity(plane * 3);
    for i in 0..plane {
        rgb.extend_from_slice(&[r[i], g[i], b[i]]);
    }
    Ok(rgb)
}

/// One frame of a transition by the equations of `transitions.md`, at p = (2i + 1) / (2n) for
/// effect frame i of n, applied alike to every plane of the outgoing (base) frame. With d = 2n and
/// k = 2i + 1, the weighted sums are rounded to nearest with ties up as `floor((x + n) / d)`.
pub(crate) enum Effect {
    /// `floor((L(d - k) + Rk + n) / d)`, which is `L + steps[R - L + 255]` with
    /// `steps[s + 255] = floor((sk + n) / d)`.
    Dissolve(Box<[i16; 511]>),
    /// Dip to black: `max(d - 2k, 0)` weights the outgoing side and `max(2k - d, 0)` the incoming
    /// one, and at most one of them is positive, so each value maps through `levels` of one side.
    Dip {
        incoming: bool,
        levels: Box<[u8; 256]>,
    },
    /// Wipe: these columns of every row take the incoming side.
    Columns(std::ops::Range<usize>),
}
impl Effect {
    pub(crate) fn new(kind: TransitionKind, width: u32, count: u64, index: u64) -> Self {
        let (n, k, w) = (
            i128::from(count),
            2 * i128::from(index) + 1,
            i128::from(width),
        );
        let d = 2 * n;
        let rounded = |x: i128| (x + n).div_euclid(d);
        match kind {
            TransitionKind::Dissolve => {
                let mut steps = Box::new([0; 511]);
                for (s, step) in (-255..=255).zip(steps.iter_mut()) {
                    *step = rounded(s * k) as i16;
                }
                Effect::Dissolve(steps)
            }
            TransitionKind::DipBlack => {
                let (incoming, weight) = if 2 * k > d {
                    (true, 2 * k - d)
                } else {
                    (false, d - 2 * k)
                };
                let mut levels = Box::new([0; 256]);
                for (v, level) in (0..).zip(levels.iter_mut()) {
                    *level = rounded(v * weight) as u8;
                }
                Effect::Dip { incoming, levels }
            }
            // Incoming where (2x + 1)n <= Wk, which holds on a prefix of each row.
            TransitionKind::WipeLeft => {
                Effect::Columns(0..(0..w).filter(|x| (2 * x + 1) * n <= w * k).count())
            }
            // Incoming where (2(W - x) - 1)n <= Wk, which holds on a suffix of each row.
            TransitionKind::WipeRight => {
                let count = (0..w).filter(|x| (2 * (w - x) - 1) * n <= w * k).count();
                Effect::Columns(width as usize - count..width as usize)
            }
        }
    }
    /// Mix `incoming` into the planar `frame` of rows `width` long, in parallel row bands.
    fn apply(&self, frame: &mut [u8], incoming: &[u8], width: usize) {
        if let Effect::Columns(columns) = self
            && columns.is_empty()
        {
            return;
        }
        let workers = if frame.len() < 1 << 19 {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .clamp(1, 16)
        };
        if workers == 1 {
            return self.rows(frame, incoming, width);
        }
        let band = (frame.len() / width).div_ceil(workers) * width;
        std::thread::scope(|scope| {
            for (frame, incoming) in frame.chunks_mut(band).zip(incoming.chunks(band)) {
                scope.spawn(move || self.rows(frame, incoming, width));
            }
        });
    }
    pub(crate) fn apply_packed(&self, frame: &mut [u8], incoming: &[u8], width: usize) {
        if let Effect::Columns(columns) = self {
            let span = columns.start * 3..columns.end * 3;
            for (left, right) in frame
                .chunks_exact_mut(width * 3)
                .zip(incoming.chunks_exact(width * 3))
            {
                left[span.clone()].copy_from_slice(&right[span.clone()]);
            }
        } else {
            self.apply(frame, incoming, width * 3);
        }
    }
    fn rows(&self, frame: &mut [u8], incoming: &[u8], width: usize) {
        match self {
            Effect::Dissolve(steps) => {
                for (l, &r) in frame.iter_mut().zip(incoming) {
                    let step = steps[(i16::from(r) - i16::from(*l) + 255) as usize];
                    *l = (i16::from(*l) + step) as u8;
                }
            }
            Effect::Dip {
                incoming: false,
                levels,
            } => frame.iter_mut().for_each(|l| *l = levels[*l as usize]),
            Effect::Dip {
                incoming: true,
                levels,
            } => {
                for (l, &r) in frame.iter_mut().zip(incoming) {
                    *l = levels[r as usize];
                }
            }
            Effect::Columns(columns) => {
                for (l, r) in frame
                    .chunks_exact_mut(width)
                    .zip(incoming.chunks_exact(width))
                {
                    l[columns.clone()].copy_from_slice(&r[columns.clone()]);
                }
            }
        }
    }
}

/// The raw input options of an encoder reading composited frames on stdin.
pub(crate) fn raw_input(width: u32, height: u32, rate: crate::time::Time) -> Vec<String> {
    [
        "-f".to_owned(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "gbrp".into(),
        "-video_size".into(),
        format!("{width}x{height}"),
        "-framerate".into(),
        format!("{}/{}", rate.num, rate.den),
        "-i".into(),
        "pipe:0".into(),
    ]
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Layer {
        width: u32,
        height: u32,
        alpha: bool,
        divisor: u32,
        opacity: u8,
        position: [i32; 2],
    }

    fn layer(
        width: u32,
        height: u32,
        alpha: bool,
        divisor: u32,
        opacity: u8,
        position: [i32; 2],
    ) -> Layer {
        Layer {
            width,
            height,
            alpha,
            divisor,
            opacity,
            position,
        }
    }

    /// Independent per-pixel reference of the documented equations, on packed RGBA.
    fn reference(base: &[[u8; 3]], w: usize, h: usize, l: &Layer, src: &[[u8; 4]]) -> Vec<[u8; 3]> {
        let mut out = base.to_vec();
        let k = l.divisor as usize;
        let (pw, ph) = (l.width as usize / k, l.height as usize / k);
        for py in 0..ph {
            for px in 0..pw {
                let (cx, cy) = (
                    px as i64 + l.position[0] as i64,
                    py as i64 + l.position[1] as i64,
                );
                if cx < 0 || cy < 0 || cx >= w as i64 || cy >= h as i64 {
                    continue;
                }
                let mut sum = [0u64; 4];
                for dy in 0..k {
                    for dx in 0..k {
                        let p = src[(py * k + dy) * l.width as usize + px * k + dx];
                        for c in 0..4 {
                            sum[c] += p[c] as u64;
                        }
                    }
                }
                let mean = sum.map(|v| v / (k * k) as u64);
                let a = (mean[3] * l.opacity as u64 + 127) / 255;
                let d = &mut out[cy as usize * w + cx as usize];
                for c in 0..3 {
                    d[c] = ((2 * (mean[c] * a + d[c] as u64 * (255 - a)) + 255) / 510) as u8;
                }
            }
        }
        out
    }

    fn check(w: usize, h: usize, l: Layer, seed: u32) {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 24) as u8
        };
        let base: Vec<[u8; 3]> = (0..w * h).map(|_| [next(), next(), next()]).collect();
        let levels = [0u8, 1, 64, 127, 128, 200, 254, 255];
        let src: Vec<[u8; 4]> = (0..(l.width * l.height) as usize)
            .map(|i| {
                let a = if l.alpha {
                    levels[(i + next() as usize) % 8]
                } else {
                    255
                };
                [next(), next(), next(), a]
            })
            .collect();
        let canvas = [
            w.max(l.width as usize) as u32,
            h.max(l.height as usize) as u32,
        ];
        let plane = canvas[0] as usize * canvas[1] as usize;
        let mut image = crate::scene::Pixels {
            width: canvas[0],
            height: canvas[1],
            rgba: vec![0; plane * 4],
        };
        let mut frame = vec![0; plane * 3];
        for y in 0..l.height as usize {
            for x in 0..l.width as usize {
                let dst = (y * canvas[0] as usize + x) * 4;
                image.rgba[dst..dst + 4].copy_from_slice(&src[y * l.width as usize + x]);
            }
        }
        for y in 0..h {
            for x in 0..w {
                let dst = (y * canvas[0] as usize + x) * 3;
                frame[dst..dst + 3].copy_from_slice(&base[y * w + x]);
            }
        }
        let old = crate::tracks::OverlayTransform {
            crop: Some([0, 0, l.width, l.height]),
            divisor: l.divisor,
            opacity: l.opacity,
            position: l.position,
        };
        let duration = crate::time::Time::new(1, 1).unwrap();
        let properties = crate::clip_properties::normalize(&[], Some(&old), canvas, duration)
            .unwrap()
            .unwrap();
        let sampler = properties.recipe().prepare(duration).unwrap();
        properties
            .draw(&sampler, crate::time::Time::ZERO, &mut image, &mut frame, 1)
            .unwrap();
        let actual = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y * canvas[0] as usize + x) * 3))
            .flat_map(|i| frame[i..i + 3].iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            reference(&base, w, h, &l, &src)
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn composition_matches_the_documented_equations() {
        check(40, 30, layer(40, 30, true, 1, 255, [0, 0]), 1);
        check(40, 30, layer(40, 30, true, 1, 128, [0, 0]), 2);
        check(40, 30, layer(24, 16, true, 1, 77, [-5, 20]), 3);
        check(40, 30, layer(24, 16, false, 1, 255, [30, -3]), 4);
        check(40, 30, layer(24, 16, false, 4, 180, [35, 25]), 5);
        check(40, 30, layer(40, 32, false, 2, 255, [-20, 0]), 6);
        check(40, 30, layer(16, 16, true, 1, 0, [3, 3]), 7);
        check(40, 30, layer(16, 16, false, 1, 255, [40, 0]), 8);
        // Large enough for parallel row bands.
        check(640, 480, layer(640, 480, true, 1, 200, [0, 0]), 9);
        check(640, 480, layer(1280, 960, false, 2, 255, [0, 0]), 10);
    }

    /// Independent per-value reference of the transition table in `transitions.md` on one row-major
    /// plane sequence, with p = (2i + 1) / (2n) kept as a fraction.
    fn transition(
        kind: TransitionKind,
        width: usize,
        l: &[u8],
        r: &[u8],
        n: u64,
        i: u64,
    ) -> Vec<u8> {
        let (num, den, w) = (u128::from(2 * i + 1), u128::from(2 * n), width as u128);
        // The nearest integer to v / den, ties up.
        let nearest = |v: u128| ((2 * v + den) / (2 * den)) as u8;
        (l.iter().zip(r).enumerate())
            .map(|(at, (&a, &b))| {
                let (x, outgoing, incoming) = ((at % width) as u128, u128::from(a), u128::from(b));
                let wipe = |shown: bool| if shown { b } else { a };
                match kind {
                    TransitionKind::Dissolve => nearest((den - num) * outgoing + num * incoming),
                    TransitionKind::DipBlack => nearest(
                        den.saturating_sub(2 * num) * outgoing
                            + (2 * num).saturating_sub(den) * incoming,
                    ),
                    // (x + 1/2) / W <= p
                    TransitionKind::WipeLeft => wipe((2 * x + 1) * den <= 2 * w * num),
                    // (W - x - 1/2) / W <= p
                    TransitionKind::WipeRight => wipe((2 * w - 2 * x - 1) * den <= 2 * w * num),
                }
            })
            .collect()
    }

    #[test]
    fn transitions_match_the_documented_equations() {
        let kinds = [
            TransitionKind::Dissolve,
            TransitionKind::DipBlack,
            TransitionKind::WipeLeft,
            TransitionKind::WipeRight,
        ];
        let frames: [(u64, u64); 15] = [
            (1, 0),
            (2, 0),
            (2, 1),
            (3, 1),
            (5, 2),
            (12, 0),
            (12, 5),
            (12, 6),
            (12, 11),
            (25, 7),
            (24, 12),
            (180000, 0),
            (180000, 89999),
            (180000, 90000),
            (180000, 179999),
        ];
        // Every pair of outgoing and incoming values, in each of three planes.
        let size = 256;
        let l: Vec<u8> = (0..3 * size * size).map(|v| (v % size) as u8).collect();
        let r: Vec<u8> = (0..3 * size * size)
            .map(|v| (v / size % size) as u8)
            .collect();
        for kind in kinds {
            for (n, i) in frames {
                let mut frame = l.clone();
                Effect::new(kind, size as u32, n, i).apply(&mut frame, &r, size);
                let expected = transition(kind, size, &l, &r, n, i);
                assert!(frame == expected, "{kind:?} frame {i} of {n}");
            }
        }
        // Odd and one-column widths, and frames large enough for parallel row bands.
        let mut state = 11u32;
        let mut next = || {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 24) as u8
        };
        for (width, height) in [(1, 1), (1, 5), (7, 3), (640, 480), (1919, 3)] {
            let l: Vec<u8> = (0..3 * width * height).map(|_| next()).collect();
            let r: Vec<u8> = (0..3 * width * height).map(|_| next()).collect();
            for kind in kinds {
                for (n, i) in frames {
                    let mut frame = l.clone();
                    Effect::new(kind, width as u32, n, i).apply(&mut frame, &r, width);
                    let expected = transition(kind, width, &l, &r, n, i);
                    assert!(frame == expected, "{kind:?} {width}x{height} {i} of {n}");
                }
            }
        }
    }

    #[test]
    fn blend_is_the_rounded_quotient() {
        for s in [0u8, 1, 127, 128, 254, 255] {
            for d in [0u8, 3, 128, 255] {
                for a in 0..=255u32 {
                    let exact = (s as f64 * a as f64 + d as f64 * (255 - a) as f64) / 255.0;
                    assert_eq!(
                        crate::composite::channel(
                            d,
                            s,
                            a as u8,
                            255,
                            crate::composite::BlendMode::Normal,
                            crate::composite::AlphaMode::Straight
                        ),
                        exact.round() as u8
                    );
                }
            }
        }
    }
}
