//! Composite a placed-track timeline's top-level transitions and `alpha_over` tracks in the engine.
//!
//! FFmpeg decodes the opaque base picture (one graph), the incoming side of each visible
//! transition and each visible overlay clip (one small graph each, overlays cropped) to raw planar
//! RGB on pipes. Where a transition shows, the base picture holds its outgoing side, and the
//! engine mixes in the incoming side by the exact equations of `transitions.md`. It then applies
//! each overlay clip's shrink, opacity and position and the exact straight-alpha "over" of
//! `tracks.md`. Both run in parallel row bands, and the result streams to the encoder. Every
//! decoder reads its input once, in order.
use crate::{
    Result, error,
    media::{self, Control, StreamReader, Watch},
    tracks::TransitionKind,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// One overlay clip: its decoder and how its frames land on the canvas.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Layer {
    /// The clip's track in the arrangement; it shows where it lies above the visible base track.
    pub track: usize,
    pub track_id: String,
    pub clip_id: String,
    /// Window frames `[first, end)` the decoder supplies, one per frame, in order.
    pub first: u64,
    pub end: u64,
    /// Decoded (cropped) size; frames are planar G, B, R, plus A for straight-alpha sources.
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub divisor: u32,
    pub opacity: u8,
    pub position: [i32; 2],
    pub arguments: Vec<String>,
}
impl Layer {
    fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * if self.alpha { 4 } else { 3 }
    }
}

/// A top-level transition over consecutive window frames where its track is the visible one: the
/// base picture holds the outgoing side there, and a decoder supplies the incoming side.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Mix {
    pub track_id: String,
    pub transition_id: String,
    /// Window frames `[first, end)` the decoder supplies, one per frame, in order.
    pub first: u64,
    pub end: u64,
    pub kind: TransitionKind,
    /// Frames in the whole effect interval, and the effect frame of window frame `first`.
    pub count: u64,
    pub offset: u64,
    /// Decoder of the incoming side: planar G, B, R frames of the canvas size on stdout.
    pub arguments: Vec<String>,
}

/// The base picture's decoder plus the transitions mixed into it and the overlay clips
/// composited over it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Compositor {
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    /// Decoder of the opaque base picture: planar G, B, R frames on stdout.
    pub base: Vec<String>,
    /// Window frame runs `[start, end)` and the index of the visible opaque track (None: black).
    #[serde(skip)]
    pub runs: Vec<(u64, u64, Option<usize>)>,
    /// Transitions in window order; they never overlap, and apply before any layer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Mix>,
    pub layers: Vec<Layer>,
}

impl Compositor {
    fn base_index(&self, frame: u64) -> Option<usize> {
        self.runs
            .iter()
            .find(|(start, end, _)| (*start..*end).contains(&frame))
            .and_then(|(_, _, index)| *index)
    }
    fn plane(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// A decoder whose frames are read on their own thread, up to two ahead, so the pipe transfers
/// of all decoders overlap one another and the composition. Read frames can be handed back for
/// reuse.
struct Ahead {
    frames: mpsc::Receiver<Result<Vec<u8>>>,
    spare: mpsc::Sender<Vec<u8>>,
}
impl Ahead {
    /// Read `count` frames of `size` bytes, then require the decoder to end cleanly; its result
    /// is the last message. The thread stops when the receiver is dropped, and the decoder with it.
    fn start(mut reader: StreamReader, size: usize, count: u64) -> Self {
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
    fn next(&self) -> Result<Vec<u8>> {
        self.frames.recv().map_err(|_| {
            error(
                "RENDER_VALIDATION_FAILED",
                "A composition decoder stopped early",
            )
        })?
    }
    fn recycle(&self, frame: Vec<u8>) {
        let _ = self.spare.send(frame);
    }
    /// The decoder's own end, after its last frame.
    fn finish(self) -> Result<()> {
        self.next().map(|_| ())
    }
}

/// Transition and layer decoders start up to this many frames before their first one, at most
/// `EARLY` at a time, so that their startup overlaps the frames before.
const AHEAD: u64 = 25;
const EARLY: usize = 2;

/// Decoders of one composition and the frame they are on.
struct Decoders<'a> {
    compositor: &'a Compositor,
    base: Ahead,
    /// Transition decoders not yet started, in window order, then the started ones; the first
    /// supplies frames during its run.
    mixes: VecDeque<(usize, Command)>,
    mixing: VecDeque<(usize, Ahead)>,
    /// Layer decoders not yet started, by first frame, then the started ones by track.
    pending: VecDeque<(usize, Command)>,
    active: Vec<(usize, Ahead)>,
    program: String,
    timeout: Duration,
    abort: Arc<AtomicBool>,
}
impl Decoders<'_> {
    /// Start a decoder of `count` frames of `size` bytes.
    fn start(&self, command: Command, size: usize, count: u64) -> Result<Ahead> {
        let reader = StreamReader::spawn_with(
            command,
            &self.program,
            self.timeout,
            Watch {
                abort: Some(self.abort.clone()),
            },
        )?;
        Ok(Ahead::start(reader, size, count))
    }
    /// Start the decoders due by frame `n`, then the next ones early, in order of first frame.
    fn start_due(&mut self, n: u64) -> Result<()> {
        let c = self.compositor;
        loop {
            let mix = (self.mixes.front()).map(|(index, _)| c.transitions[*index].first);
            let layer = (self.pending.front()).map(|(index, _)| c.layers[*index].first);
            let Some(first) = mix.into_iter().chain(layer).min() else {
                return Ok(());
            };
            let early = (self.mixing.iter())
                .filter(|(index, _)| c.transitions[*index].first > n)
                .count()
                + (self.active.iter())
                    .filter(|(index, _)| c.layers[*index].first > n)
                    .count();
            if first > n && (first > n + AHEAD || early >= EARLY) {
                return Ok(());
            }
            if mix == Some(first) {
                let (index, command) = self.mixes.pop_front().expect("pending transition");
                let m = &c.transitions[index];
                let decoder = self.start(command, c.plane() * 3, m.end - m.first)?;
                self.mixing.push_back((index, decoder));
            } else {
                let (index, command) = self.pending.pop_front().expect("pending layer");
                let layer = &c.layers[index];
                let decoder = self.start(command, layer.frame_bytes(), layer.end - layer.first)?;
                self.active.push((index, decoder));
                self.active.sort_by_key(|(index, _)| c.layers[*index].track);
            }
        }
    }
    /// The composited frame `n` as planar G, B, R.
    fn next(&mut self, n: u64) -> Result<Vec<u8>> {
        let c = self.compositor;
        self.start_due(n)?;
        let mut frame = self.base.next()?;
        if let Some((index, decoder)) = self.mixing.front()
            && c.transitions[*index].first <= n
        {
            let m = &c.transitions[*index];
            let incoming = decoder.next()?;
            let effect = Effect::new(m.kind, c.width, m.count, m.offset + (n - m.first));
            effect.apply(&mut frame, &incoming, c.width as usize);
            decoder.recycle(incoming);
            if m.end == n + 1 {
                let (_, decoder) = self.mixing.pop_front().expect("transition decoder");
                decoder.finish()?;
            }
        }
        let base = c.base_index(n);
        for (index, decoder) in &self.active {
            let layer = &c.layers[*index];
            if layer.first > n {
                continue;
            }
            let pixels = decoder.next()?;
            if base.is_none_or(|b| layer.track > b) {
                over(&mut frame, c.width, c.height, layer, &pixels);
            }
            decoder.recycle(pixels);
        }
        let mut still = Vec::with_capacity(self.active.len());
        for (index, decoder) in self.active.drain(..) {
            if c.layers[index].end == n + 1 {
                decoder.finish()?;
            } else {
                still.push((index, decoder));
            }
        }
        self.active = still;
        Ok(frame)
    }
}

fn decoders<'a>(
    compositor: &'a Compositor,
    timeout: Duration,
    control: &dyn Control,
    abort: &Arc<AtomicBool>,
) -> Result<Decoders<'a>> {
    let program = control.tool("ffmpeg");
    let base = Ahead::start(
        StreamReader::spawn_with(
            control.command(&program, &compositor.base)?,
            &program,
            timeout,
            Watch {
                abort: Some(abort.clone()),
            },
        )?,
        compositor.plane() * 3,
        compositor.frames,
    );
    let mixes = (compositor.transitions.iter().enumerate())
        .map(|(index, mix)| Ok((index, control.command(&program, &mix.arguments)?)))
        .collect::<Result<VecDeque<_>>>()?;
    let mut layers: Vec<usize> = (0..compositor.layers.len()).collect();
    layers.sort_by_key(|&index| compositor.layers[index].first);
    let pending = (layers.into_iter())
        .map(|index| {
            let arguments = &compositor.layers[index].arguments;
            Ok((index, control.command(&program, arguments)?))
        })
        .collect::<Result<VecDeque<_>>>()?;
    Ok(Decoders {
        compositor,
        base,
        mixes,
        mixing: VecDeque::new(),
        pending,
        active: Vec::new(),
        program,
        timeout,
        abort: abort.clone(),
    })
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
    let mut decoders = decoders(compositor, timeout, control, &abort)?;
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
                    let finished = decoders.base.finish();
                    if finished.is_err() {
                        let _ = sender.send(finished.map(|()| Vec::new()));
                    }
                });
                let outcome = (|| {
                    let mut written = 0;
                    let mut reported = Instant::now();
                    for frame in receiver.iter() {
                        let frame = frame?;
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
                if outcome.is_err() {
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
    stopped
}

/// The single composited frame of a one-frame window, as packed RGB.
pub(crate) fn frame_rgb(
    compositor: &Compositor,
    timeout: Duration,
    control: &dyn Control,
) -> Result<Vec<u8>> {
    let abort = Arc::new(AtomicBool::new(false));
    let outcome = (|| -> Result<Vec<u8>> {
        let mut decoders = decoders(compositor, timeout, control, &abort)?;
        let planar = decoders.next(0)?;
        decoders.base.finish()?;
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

/// Straight-alpha over in encoded RGB with exact integer rounding:
/// `floor((2*(s*a + d*(255-a)) + 255) / 510)`, the nearest integer (255 is odd, so no ties).
#[inline]
fn blend(s: u8, d: u8, a: u32) -> u8 {
    ((2 * (s as u32 * a + d as u32 * (255 - a)) + 255) / 510) as u8
}

/// Composite one decoded overlay frame onto planar G, B, R `frame`: shrink by the floor of each
/// block's mean, scale alpha by the opacity rounded to nearest, place, clip and blend.
fn over(frame: &mut [u8], width: u32, height: u32, layer: &Layer, pixels: &[u8]) {
    let (w, h) = (width as i64, height as i64);
    let k = layer.divisor.max(1) as i64;
    let (placed_w, placed_h) = (layer.width as i64 / k, layer.height as i64 / k);
    let [x, y] = layer.position.map(i64::from);
    let (left, top) = (x.max(0), y.max(0));
    let (right, bottom) = ((x + placed_w).min(w), (y + placed_h).min(h));
    if left >= right || top >= bottom {
        return;
    }
    let mut opacity = [0u8; 256];
    for (a, scaled) in opacity.iter_mut().enumerate() {
        *scaled = ((a as u32 * layer.opacity as u32 + 127) / 255) as u8;
    }
    let plane = (width * height) as usize;
    let rows = top as usize * width as usize..bottom as usize * width as usize;
    let (g, rest) = frame.split_at_mut(plane);
    let (b, r) = rest.split_at_mut(plane);
    let (g, b, r) = (&mut g[rows.clone()], &mut b[rows.clone()], &mut r[rows]);
    let area = (right - left) * (bottom - top);
    let workers = if area < 1 << 18 {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, 16)
    };
    let band = ((bottom - top) as usize).div_ceil(workers) * width as usize;
    let span = Span {
        width: width as usize,
        left: left as usize,
        right: right as usize,
        x,
        y,
        k: k as usize,
    };
    let opacity = &opacity;
    if workers == 1 {
        span.rows(g, b, r, top as usize, layer, pixels, opacity);
        return;
    }
    std::thread::scope(|scope| {
        for (i, ((g, b), r)) in g
            .chunks_mut(band)
            .zip(b.chunks_mut(band))
            .zip(r.chunks_mut(band))
            .enumerate()
        {
            let first = top as usize + i * band / width as usize;
            scope.spawn(move || span.rows(g, b, r, first, layer, pixels, opacity));
        }
    });
}

/// The visible columns of a placed overlay and its placement.
#[derive(Clone, Copy)]
struct Span {
    width: usize,
    left: usize,
    right: usize,
    x: i64,
    y: i64,
    k: usize,
}
impl Span {
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &self,
        g: &mut [u8],
        b: &mut [u8],
        r: &mut [u8],
        first: usize,
        layer: &Layer,
        pixels: &[u8],
        opacity: &[u8; 256],
    ) {
        let (lw, lh) = (layer.width as usize, layer.height as usize);
        let plane = lw * lh;
        let (sg, rest) = pixels.split_at(plane);
        let (sb, rest) = rest.split_at(plane);
        let (sr, sa) = rest.split_at(plane);
        let k = self.k;
        let count = (k * k) as u32;
        for row in 0..g.len() / self.width {
            let cy = first + row;
            let py = (cy as i64 - self.y) as usize;
            let line = row * self.width;
            for cx in self.left..self.right {
                let px = (cx as i64 - self.x) as usize;
                let (s, a) = if k == 1 {
                    let i = py * lw + px;
                    let a = if layer.alpha {
                        opacity[sa[i] as usize]
                    } else {
                        opacity[255]
                    };
                    ([sg[i], sb[i], sr[i]], a)
                } else {
                    // Only opaque sources shrink: every alpha in the block is 255.
                    let mut sum = [0u32; 3];
                    for dy in 0..k {
                        let start = (py * k + dy) * lw + px * k;
                        for i in start..start + k {
                            sum[0] += sg[i] as u32;
                            sum[1] += sb[i] as u32;
                            sum[2] += sr[i] as u32;
                        }
                    }
                    (sum.map(|v| (v / count) as u8), opacity[255])
                };
                let d = line + cx;
                match a {
                    0 => {}
                    255 => {
                        g[d] = s[0];
                        b[d] = s[1];
                        r[d] = s[2];
                    }
                    a => {
                        let a = a as u32;
                        g[d] = blend(s[0], g[d], a);
                        b[d] = blend(s[1], b[d], a);
                        r[d] = blend(s[2], r[d], a);
                    }
                }
            }
        }
    }
}

/// One frame of a transition by the equations of `transitions.md`, at p = (2i + 1) / (2n) for
/// effect frame i of n, applied alike to every plane of the outgoing (base) frame. With d = 2n and
/// k = 2i + 1, the weighted sums are rounded to nearest with ties up as `floor((x + n) / d)`.
enum Effect {
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
    fn new(kind: TransitionKind, width: u32, count: u64, index: u64) -> Self {
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

    fn layer(
        width: u32,
        height: u32,
        alpha: bool,
        divisor: u32,
        opacity: u8,
        position: [i32; 2],
    ) -> Layer {
        Layer {
            track: 1,
            track_id: "o".into(),
            clip_id: "c".into(),
            first: 0,
            end: 1,
            width,
            height,
            alpha,
            divisor,
            opacity,
            position,
            arguments: Vec::new(),
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

    fn planar(rgb: &[[u8; 3]]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in [1, 2, 0] {
            out.extend(rgb.iter().map(|p| p[c]));
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
        let mut pixels = Vec::new();
        for c in [1, 2, 0, 3] {
            if c < 3 || l.alpha {
                pixels.extend(src.iter().map(|p| p[c]));
            }
        }
        let mut frame = planar(&base);
        over(&mut frame, w as u32, h as u32, &l, &pixels);
        assert_eq!(frame, planar(&reference(&base, w, h, &l, &src)));
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
                    assert_eq!(blend(s, d, a), exact.round() as u8);
                }
            }
        }
    }
}
