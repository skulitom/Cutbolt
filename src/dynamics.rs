//! Dynamics in the placed-track mix: an original lookahead peak limiter on an audio track's
//! submix or on the master, and the 4x oversampled peak estimate that it and the meters share.
//!
//! Every step is integer arithmetic on PCM units, so results are exact and platform independent
//! (apart from converting the ceiling from dB once). A limiter's output at a sample depends only
//! on the input within a bounded distance of it ([`Limiter::reach`]), so a window of a timeline
//! (a preview, a chunk of a long render) renders exactly the samples of the whole render.
use crate::{Result, error, media, render};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Dynamics on an audio track's summed clips, or on the master mix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dynamics {
    /// Lookahead peak limiter.
    pub limiter: Limiter,
}

/// Lookahead peak limiter. The gain needed to hold each peak at the ceiling is reached by a linear
/// ramp over the lookahead before the peak, so no sample passes the ceiling; afterwards the gain
/// rises back linearly. Peaks between samples are found by 4x oversampling and held at the ceiling
/// too, so the true peak stays close to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limiter {
    /// Highest output level in dBFS, -20 to 0.
    pub ceiling_dbfs: f64,
    /// Lookahead in whole milliseconds, 1-20; default 5.
    #[serde(default = "default_lookahead")]
    pub lookahead_ms: u32,
    /// Release in whole milliseconds, 10-2000; default 150. The gain rises at a rate that would take this long from silence to unity.
    #[serde(default = "default_release")]
    pub release_ms: u32,
}
// Validation keeps the ceiling finite.
impl Eq for Limiter {}

fn default_lookahead() -> u32 {
    5
}
fn default_release() -> u32 {
    150
}

impl Limiter {
    pub(crate) fn new(ceiling_dbfs: f64) -> Self {
        Self {
            ceiling_dbfs,
            lookahead_ms: default_lookahead(),
            release_ms: default_release(),
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if !(self.ceiling_dbfs.is_finite() && (-20.0..=0.0).contains(&self.ceiling_dbfs))
            || !(1..=20).contains(&self.lookahead_ms)
            || !(10..=2000).contains(&self.release_ms)
        {
            return Err(error(
                "INVALID_AUDIO_EFFECT",
                "A limiter needs ceiling_dbfs -20..0, lookahead_ms 1..20 and release_ms 10..2000",
            ));
        }
        Ok(())
    }
    /// The ceiling in PCM16 codes: floor(32768 * 10^(ceiling_dbfs / 20)), at most 32767.
    pub(crate) fn ceiling(&self) -> u64 {
        ((32768.0 * 10f64.powf(self.ceiling_dbfs / 20.0)).floor() as u64).min(32767)
    }
    fn lookahead(&self) -> usize {
        self.lookahead_ms as usize * 48
    }
    fn release(&self) -> u64 {
        u64::from(self.release_ms) * 48
    }
    /// Samples before and after a sample that its output depends on: the lookahead, the release
    /// memory and the interpolation taps.
    pub(crate) fn reach(&self) -> (u64, u64) {
        let lookahead = self.lookahead() as u64;
        (lookahead + self.release() + 15, lookahead + 16)
    }
}

pub fn capabilities() -> Value {
    json!({"profile":"timeline-limiter-v1","edit":"audio_dynamics","scopes":["audio_track","master"],"timeline":"project_tracks_only",
        "order":"track_limiters_then_sum_then_master_limiter_then_pcm16_saturation","ceiling_dbfs":[-20,0],"ceiling_codes":"floor(32768*10^(dB/20)) at most 32767",
        "lookahead_ms":[1,20],"release_ms":[10,2000],"defaults":{"lookahead_ms":5,"release_ms":150},"detection":"linked_stereo_4x_oversampled_peaks_within_8_samples",
        "attack":"linear_over_lookahead","release":"linear","arithmetic":"integer","sample_peak":"never_above_ceiling","true_peak":"detected_at_the_meters_4x_points; output_within_0.01_dB_of_the_ceiling_in_tests_not_guaranteed",
        "windows":"exact_any_range_or_chunk","reach":"lookahead+release+15 samples before, lookahead+16 after"})
}

/// Unity gain: gains are fractions of 2^32.
const ONE: u64 = 1 << 32;
/// Interpolated values carry 16 fractional bits; each phase's taps sum to this.
const SCALE: i64 = 1 << 16;
/// Taps that interpolate the points a quarter, a half and three quarters of the way from sample
/// `j` to `j + 1`, applied to samples `j - 7 ..= j + 8`. They are a Kaiser-windowed sinc
/// (beta 5, half-width 8 samples) normalized to sum to `SCALE`, rounded, with the rounding residue
/// on the tap(s) nearest the point; the outer phases mirror each other. The response is flat
/// within 0.04 dB to 18 kHz and 0.35 dB down at 20 kHz.
const TAPS: [[i64; 16]; 3] = [
    [
        -185, 467, -960, 1769, -3098, 5482, -11181, 58928, 19302, -7573, 4094, -2348, 1316, -682,
        305, -100,
    ],
    [
        -196, 537, -1149, 2163, -3816, 6685, -12865, 41409, 41409, -12865, 6685, -3816, 2163,
        -1149, 537, -196,
    ],
    [
        -100, 305, -682, 1316, -2348, 4094, -7573, 19302, 58928, -11181, 5482, -3098, 1769, -960,
        467, -185,
    ],
];
/// The largest sum of absolute taps of a phase: no interpolated magnitude exceeds this many
/// times the largest magnitude it reads.
const GAIN_BOUND: u64 = {
    let mut best = 0;
    let mut phase = 0;
    while phase < 3 {
        let (mut sum, mut tap) = (0, 0);
        while tap < 16 {
            sum += TAPS[phase][tap].unsigned_abs();
            tap += 1;
        }
        if sum > best {
            best = sum;
        }
        phase += 1;
    }
    best
};

/// The largest magnitude, in 1/SCALE codes, at sample `j` and the three interpolated points
/// toward `j + 1`, given `x[j - 7 ..= j + 8]` oldest first.
fn peak(window: &[i64; 16]) -> u64 {
    let mut best = window[7].unsigned_abs() * SCALE as u64;
    for taps in &TAPS {
        let value: i64 = taps.iter().zip(window).map(|(h, x)| h * x).sum();
        best = best.max(value.unsigned_abs());
    }
    best
}

/// Push a sample into a channel's interpolation history.
fn shift(history: &mut [i64; 16], value: i64) {
    history.copy_within(1.., 0);
    history[15] = value;
}

/// `x * gain / ONE`, rounded to nearest with ties away from zero.
fn apply(x: i64, gain: u64) -> i64 {
    let product = i128::from(x) * i128::from(gain);
    let half = i128::from(ONE / 2);
    if product < 0 {
        -(((-product + half) >> 32) as i64)
    } else {
        ((product + half) >> 32) as i64
    }
}

/// The true peak of final stereo PCM16: the largest magnitude over the samples and the points
/// interpolated at quarter-sample steps, as BS.1770-5 Annex 2 describes, with silence outside
/// the signal. It uses the limiter's interpolation taps; no meter certification is claimed.
pub(crate) struct TruePeak {
    history: [[i64; 16]; 2],
    best: [u64; 2],
    /// Pushes since a sample loud enough to reach the current best; the taps are skipped when
    /// none is in reach.
    since: [usize; 2],
}
impl TruePeak {
    pub(crate) fn new() -> Self {
        Self {
            history: [[0; 16]; 2],
            best: [0; 2],
            since: [16; 2],
        }
    }
    pub(crate) fn push(&mut self, pair: [i16; 2]) {
        for (channel, sample) in pair.into_iter().enumerate() {
            self.sample(channel, i64::from(sample));
        }
    }
    fn sample(&mut self, channel: usize, value: i64) {
        shift(&mut self.history[channel], value);
        // An interpolated value is at most GAIN_BOUND times the largest magnitude it reads.
        if value.unsigned_abs() * GAIN_BOUND > self.best[channel] {
            self.since[channel] = 0;
        } else {
            self.since[channel] = self.since[channel].saturating_add(1);
        }
        if self.since[channel] < 16 {
            self.best[channel] = self.best[channel].max(peak(&self.history[channel]));
        }
    }
    /// Per channel in dBTP; null for silence.
    pub(crate) fn finish(mut self) -> [Option<f64>; 2] {
        // The last samples' interpolated neighbours read the silence after the signal.
        for _ in 0..16 {
            self.sample(0, 0);
            self.sample(1, 0);
        }
        self.best
            .map(|best| (best > 0).then(|| 20.0 * (best as f64 / (SCALE as f64 * 32768.0)).log10()))
    }
}

/// Gain reduction a limiter applied inside an output window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reduction {
    pub frames: u64,
    pub reduced: u64,
    pub minimum: u64,
}
impl Default for Reduction {
    fn default() -> Self {
        Self {
            frames: 0,
            reduced: 0,
            minimum: ONE,
        }
    }
}
impl Reduction {
    fn add(&mut self, gain: u64) {
        self.frames += 1;
        if gain < ONE {
            self.reduced += 1;
            self.minimum = self.minimum.min(gain);
        }
    }
    fn merge(&mut self, other: &Reduction) {
        self.frames += other.frames;
        self.reduced += other.reduced;
        self.minimum = self.minimum.min(other.minimum);
    }
    /// The deepest reduction in dB (positive), to hundredths.
    pub(crate) fn max_db(&self) -> f64 {
        let db = -20.0 * (self.minimum.max(1) as f64 / ONE as f64).log10();
        (db * 100.0).round() / 100.0
    }
    fn value(&self, limiter: &Limiter) -> Value {
        let seconds = self.reduced as f64 / 48_000.0;
        json!({"ceiling_dbfs":limiter.ceiling_dbfs,"max_reduction_db":self.max_db(),
            "reduced_seconds":(seconds * 1000.0).round() / 1000.0,
            "reduced_fraction":if self.frames == 0 {0.0} else {((self.reduced as f64 / self.frames as f64) * 10_000.0).round() / 10_000.0}})
    }
}

/// One limiter processing a stereo stream in order. Push `x[k]`; it returns `y[k - latency]`
/// and the gain applied to it. Before its first input the stream is taken as silence.
struct Stage {
    /// The ceiling in 1/SCALE codes; larger peaks are reduced.
    ceiling: u64,
    /// `ceiling * ONE`: dividing by a peak gives the gain that puts it on the ceiling.
    numerator: u128,
    lookahead: usize,
    step: u64,
    history: [[i64; 16]; 2],
    /// Pushes since a sample whose interpolated neighbours could pass the ceiling.
    since: usize,
    /// The last 17 sample-interval peaks, so each required gain covers every gain its taps read.
    peaks: VecDeque<u64>,
    /// Required gains of the lookahead window as (index, gain), gains increasing.
    window: VecDeque<(u64, u64)>,
    index: u64,
    release: u64,
    /// The last `lookahead + 1` released gains and their sum, whose mean is the applied gain.
    smoothing: Vec<u64>,
    at: usize,
    sum: u128,
    delay: VecDeque<[i64; 2]>,
    /// Output window and the position of the next output.
    output: (i64, i64),
    position: i64,
    reduction: Reduction,
}
impl Stage {
    /// A stage whose first input is at `first`, counting reduction over `output`.
    fn new(limiter: &Limiter, first: i64, output: (i64, i64)) -> Self {
        let lookahead = limiter.lookahead();
        let ceiling = limiter.ceiling() * SCALE as u64;
        let latency = lookahead + 16;
        Self {
            ceiling,
            numerator: u128::from(ceiling) * u128::from(ONE),
            lookahead,
            step: ONE.div_ceil(limiter.release()),
            history: [[0; 16]; 2],
            since: 16,
            peaks: VecDeque::from(vec![0; 17]),
            window: VecDeque::new(),
            index: 0,
            release: ONE,
            smoothing: vec![ONE; lookahead + 1],
            at: 0,
            sum: u128::from(ONE) * (lookahead as u128 + 1),
            delay: VecDeque::from(vec![[0; 2]; latency]),
            output,
            position: first - latency as i64,
            reduction: Reduction::default(),
        }
    }
    fn latency(&self) -> usize {
        self.lookahead + 16
    }
    fn push(&mut self, x: [i64; 2]) -> [i64; 2] {
        // The interval peak at j = k - 8, unless no sample its taps read could pass the ceiling
        // (then its exact value cannot matter).
        let quiet = self.ceiling / GAIN_BOUND;
        for (channel, &value) in x.iter().enumerate() {
            shift(&mut self.history[channel], value);
        }
        if x.iter().any(|v| v.unsigned_abs() > quiet) {
            self.since = 0;
        } else {
            self.since = self.since.saturating_add(1);
        }
        let interval = if self.since < 16 {
            peak(&self.history[0]).max(peak(&self.history[1]))
        } else {
            0
        };
        self.peaks.pop_front();
        self.peaks.push_back(interval);
        // The gain sample i = k - 16 needs: every interval within 8 samples held at the ceiling.
        let largest = self.peaks.iter().copied().max().unwrap_or(0);
        let required = if largest <= self.ceiling {
            ONE
        } else {
            (self.numerator / u128::from(largest)) as u64
        };
        // The smallest required gain of samples i - lookahead ..= i, for sample i - lookahead.
        while self.window.back().is_some_and(|&(_, g)| g >= required) {
            self.window.pop_back();
        }
        self.window.push_back((self.index, required));
        while self
            .window
            .front()
            .is_some_and(|&(i, _)| i + (self.lookahead as u64) < self.index)
        {
            self.window.pop_front();
        }
        self.index += 1;
        let held = self.window.front().map_or(ONE, |&(_, g)| g);
        // Linear release, then the mean over the lookahead turns each step down into a ramp
        // that ends at (or below) the required gain.
        self.release = held.min(self.release.saturating_add(self.step).min(ONE));
        self.sum = self.sum - u128::from(self.smoothing[self.at]) + u128::from(self.release);
        self.smoothing[self.at] = self.release;
        self.at = (self.at + 1) % self.smoothing.len();
        let gain = (self.sum / self.smoothing.len() as u128) as u64;
        self.delay.push_back(x);
        let delayed = self.delay.pop_front().expect("delay line");
        if (self.output.0..self.output.1).contains(&self.position) {
            self.reduction.add(gain);
        }
        self.position += 1;
        delayed.map(|v| apply(v, gain))
    }
}

/// Track limiters, their sum and the master limiter, aligned to one latency.
struct Mixer {
    /// Per group: its limiter, and a delay that aligns it with the slowest track limiter.
    lanes: Vec<(Option<Stage>, VecDeque<[i64; 2]>)>,
    master: Option<Stage>,
    latency: usize,
}
impl Mixer {
    fn new(
        groups: &[Option<Limiter>],
        master: Option<&Limiter>,
        first: i64,
        output: (i64, i64),
    ) -> Self {
        let stages: Vec<Option<Stage>> = groups
            .iter()
            .map(|l| l.as_ref().map(|l| Stage::new(l, first, output)))
            .collect();
        let tracks = stages
            .iter()
            .flatten()
            .map(Stage::latency)
            .max()
            .unwrap_or(0);
        let lanes = stages
            .into_iter()
            .map(|stage| {
                let own = stage.as_ref().map_or(0, Stage::latency);
                (stage, VecDeque::from(vec![[0; 2]; tracks - own]))
            })
            .collect();
        let master = master.map(|l| Stage::new(l, first - tracks as i64, output));
        let latency = tracks + master.as_ref().map_or(0, Stage::latency);
        Self {
            lanes,
            master,
            latency,
        }
    }
    /// One frame of every group in; the mix `latency` frames earlier out, unsaturated.
    fn push(&mut self, groups: &[[i64; 2]]) -> [i64; 2] {
        let mut sum = [0i64; 2];
        for ((stage, delay), &x) in self.lanes.iter_mut().zip(groups) {
            let y = stage.as_mut().map_or(x, |s| s.push(x));
            delay.push_back(y);
            let aligned = delay.pop_front().expect("alignment delay");
            sum[0] += aligned[0];
            sum[1] += aligned[1];
        }
        match &mut self.master {
            Some(stage) => stage.push(sum),
            None => sum,
        }
    }
}

/// Gain reduction of a rendered window's limiters.
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub master: Option<(Limiter, Reduction)>,
    pub tracks: Vec<(String, Limiter, Reduction)>,
}
impl Report {
    pub(crate) fn merge(&mut self, other: &Report) {
        match (&mut self.master, &other.master) {
            (Some((_, mine)), Some((_, theirs))) => mine.merge(theirs),
            (None, Some(theirs)) => self.master = Some(theirs.clone()),
            _ => {}
        }
        for (id, limiter, reduction) in &other.tracks {
            match self.tracks.iter_mut().find(|(t, _, _)| t == id) {
                Some((_, _, mine)) => mine.merge(reduction),
                None => self.tracks.push((id.clone(), limiter.clone(), *reduction)),
            }
        }
    }
    pub(crate) fn value(&self) -> Value {
        json!({"master":self.master.as_ref().map(|(l, r)| r.value(l)),
            "tracks":self.tracks.iter().map(|(id, l, r)| {
                let mut value = r.value(l);
                value["track_id"] = json!(id);
                value
            }).collect::<Vec<_>>()})
    }
}

/// One FFmpeg run of a premix: the groups' unsaturated sums over consecutive samples, as
/// interleaved signed 32-bit PCM codes (two channels per group) on stdout.
#[derive(Debug, Clone)]
pub(crate) struct Run {
    pub arguments: Vec<String>,
    pub generated: Vec<render::Generated>,
    pub frames: u64,
}

/// The audio of a window `[window.0, window.1)` of a timeline with dynamics, written as a PCM16
/// WAV just before the graph that reads it runs: the groups are mixed over a wider range by
/// FFmpeg, then limited, summed, limited again on the master and saturated here.
#[derive(Debug, Clone)]
pub(crate) struct Premix {
    pub path: PathBuf,
    pub runs: Vec<Run>,
    /// Per group: its track's ID and limiter, or none for the unprocessed tracks.
    pub groups: Vec<Option<(String, Limiter)>>,
    pub master: Option<Limiter>,
    /// First mixed sample; the runs cover `[first, first + their frames)`.
    pub first: u64,
    pub window: (u64, u64),
    pub report: Arc<Mutex<Option<Report>>>,
}
/// Frames read from a premix at a time.
const BLOCK: usize = 4096;

/// One frame of every group's sums from a premix's interleaved signed 32-bit codes.
fn decode(bytes: &[u8], frame: &mut [[i64; 2]]) {
    for (pair, group) in frame.iter_mut().zip(bytes.as_chunks::<8>().0) {
        for (value, code) in pair.iter_mut().zip(group.as_chunks::<4>().0) {
            *value = i64::from(i32::from_le_bytes(*code));
        }
    }
}

/// The groups' frames in, through the track and master limiters, out as the window's saturated
/// PCM16: the one path by which a limited render and a replayed [`Capture`] become what plays.
struct Output {
    mixer: Mixer,
    window: (i64, i64),
    /// The timeline sample the next push completes.
    position: i64,
    written: u64,
}
impl Output {
    fn new(
        groups: &[Option<Limiter>],
        master: Option<&Limiter>,
        first: i64,
        window: (i64, i64),
    ) -> Self {
        let mixer = Mixer::new(groups, master, first, window);
        // Input frame k (from `first`) yields the mix at `first + k - latency`.
        let position = first - mixer.latency as i64;
        Self {
            mixer,
            window,
            position,
            written: 0,
        }
    }
    fn push(
        &mut self,
        groups: &[[i64; 2]],
        emit: &mut impl FnMut([i16; 2]) -> Result<()>,
    ) -> Result<()> {
        let mix = self.mixer.push(groups);
        if (self.window.0..self.window.1).contains(&self.position) {
            emit(mix.map(|value| value.clamp(-32768, 32767) as i16))?;
            self.written += 1;
        }
        self.position += 1;
        Ok(())
    }
    /// Flush the limiters' lookahead with silence after the mixed range, check that the window is
    /// complete, and report each limiter's gain reduction inside it.
    fn finish(
        mut self,
        groups: &[Option<(String, Limiter)>],
        master: Option<&Limiter>,
        emit: &mut impl FnMut([i16; 2]) -> Result<()>,
    ) -> Result<Report> {
        let silence = vec![[0i64; 2]; groups.len()];
        for _ in 0..self.mixer.latency {
            self.push(&silence, emit)?;
        }
        if self.written != (self.window.1 - self.window.0) as u64 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Limited audio does not cover the rendered window",
            ));
        }
        let mut report = Report::default();
        for ((stage, _), group) in self.mixer.lanes.iter().zip(groups) {
            if let (Some(stage), Some((id, limiter))) = (stage, group) {
                report
                    .tracks
                    .push((id.clone(), limiter.clone(), stage.reduction));
            }
        }
        if let (Some(stage), Some(limiter)) = (&self.mixer.master, master) {
            report.master = Some((limiter.clone(), stage.reduction));
        }
        Ok(report)
    }
}

fn limiters(groups: &[Option<(String, Limiter)>]) -> Vec<Option<Limiter>> {
    groups
        .iter()
        .map(|g| g.as_ref().map(|(_, l)| l.clone()))
        .collect()
}

impl Premix {
    pub(crate) fn write(&self, control: &dyn media::Control, timeout: Duration) -> Result<()> {
        let (a, b) = (self.window.0 as i64, self.window.1 as i64);
        let mut output = Output::new(
            &limiters(&self.groups),
            self.master.as_ref(),
            self.first as i64,
            (a, b),
        );
        let frames = self.window.1 - self.window.0;
        let bytes = u32::try_from(frames * 4)
            .map_err(|_| error("LIMIT_EXCEEDED", "Limited audio window exceeds 4 GiB"))?;
        let mut out = std::io::BufWriter::new(std::fs::File::create_new(&self.path)?);
        out.write_all(b"RIFF")?;
        out.write_all(&(36 + bytes).to_le_bytes())?;
        out.write_all(b"WAVEfmt ")?;
        for value in [16u32, 1 | (2 << 16), 48_000, 192_000, 4 | (16 << 16)] {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(b"data")?;
        out.write_all(&bytes.to_le_bytes())?;
        let mut emit = |pair: [i16; 2]| -> Result<()> {
            for value in pair {
                out.write_all(&value.to_le_bytes())?;
            }
            Ok(())
        };
        let mut frame = vec![[0i64; 2]; self.groups.len()];
        let program = control.tool("ffmpeg");
        let width = self.groups.len() * 8;
        let mut buffer = vec![0u8; BLOCK * width];
        for run in &self.runs {
            let _inputs = render::write_generated(&run.generated, control, timeout)?;
            let mut reader = media::StreamReader::spawn_with(
                control.command(&program, &run.arguments)?,
                &program,
                timeout,
                media::Watch::default(),
            )?;
            let mut left = run.frames;
            while left > 0 {
                control.check()?;
                let count = left.min(BLOCK as u64) as usize;
                reader.read_exact(&mut buffer[..count * width])?;
                for bytes in buffer[..count * width].chunks_exact(width) {
                    decode(bytes, &mut frame);
                    output.push(&frame, &mut emit)?;
                }
                left -= count as u64;
            }
            reader.finish()?;
        }
        let report = output.finish(&self.groups, self.master.as_ref(), &mut emit)?;
        out.flush()?;
        *self.report.lock().expect("dynamics report") = Some(report);
        Ok(())
    }
}

/// A whole timeline's group sums, kept in scratch files, so its mix can be replayed through
/// another master limiter, or with every clip level scaled, without rendering it again.
pub(crate) struct Capture {
    /// Consecutive pieces of the timeline: a file of interleaved signed 32-bit group sums and its
    /// frame count.
    parts: Vec<(PathBuf, u64)>,
    groups: Vec<Option<(String, Limiter)>>,
    frames: u64,
}
impl Capture {
    /// Run a premix of a whole timeline (from its first sample to its end), up to `lanes` of its
    /// runs at once, each into its own file beside the premix's path.
    pub(crate) fn record(
        premix: &Premix,
        control: &(dyn media::Control + Sync),
        timeout: Duration,
        lanes: usize,
    ) -> Result<Self> {
        let frames = premix.window.1;
        if premix.first != 0
            || premix.window.0 != 0
            || premix.runs.iter().map(|r| r.frames).sum::<u64>() != frames
        {
            return Err(error(
                "INTERNAL_ERROR",
                "A capture needs a premix of the whole timeline",
            ));
        }
        // The capture owns its files from the start, so a failure removes whatever was written.
        let capture = Self {
            parts: (premix.runs.iter().enumerate())
                .map(|(i, run)| (premix.path.with_extension(format!("{i}.s32")), run.frames))
                .collect(),
            groups: premix.groups.clone(),
            frames,
        };
        let width = premix.groups.len() as u64 * 8;
        let tool = control.tool("ffmpeg");
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed = std::sync::atomic::AtomicBool::new(false);
        let work = || -> Result<()> {
            use std::sync::atomic::Ordering::Relaxed;
            while !failed.load(Relaxed) {
                let index = next.fetch_add(1, Relaxed);
                let (Some(run), Some((path, _))) =
                    (premix.runs.get(index), capture.parts.get(index))
                else {
                    break;
                };
                let done = (|| {
                    let _inputs = render::write_generated(&run.generated, control, timeout)?;
                    let mut arguments = run.arguments.clone();
                    *arguments.last_mut().expect("output argument") =
                        path.to_string_lossy().into_owned();
                    media::capture_controlled(&tool, &arguments, timeout, control)?;
                    if std::fs::metadata(path)?.len() != run.frames * width {
                        return Err(error(
                            "RENDER_VALIDATION_FAILED",
                            "Captured audio does not cover the timeline",
                        ));
                    }
                    Ok(())
                })();
                if done.is_err() {
                    failed.store(true, Relaxed);
                    return done;
                }
            }
            Ok(())
        };
        let lanes = lanes.clamp(1, premix.runs.len().max(1));
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..lanes).map(|_| scope.spawn(work)).collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("capture worker"))
                .collect::<Result<Vec<()>>>()
        })?;
        Ok(capture)
    }
    /// The timeline's final PCM, with every group's sum multiplied by `factor` and rounded to whole
    /// codes (ties away from zero), through the groups' own limiters and `master`: `each` gets
    /// every frame in order. At factor 1 this is exactly what the timeline plays with `master` on
    /// its master track. The reduction report is None when there is no limiter at all.
    pub(crate) fn replay(
        &self,
        factor: f64,
        master: Option<&Limiter>,
        mut each: impl FnMut([i16; 2]),
    ) -> Result<Option<Report>> {
        use std::io::Read;
        let mut output = Output::new(&limiters(&self.groups), master, 0, (0, self.frames as i64));
        let mut emit = |pair: [i16; 2]| -> Result<()> {
            each(pair);
            Ok(())
        };
        let width = self.groups.len() * 8;
        let mut buffer = vec![0u8; BLOCK * width];
        let mut frame = vec![[0i64; 2]; self.groups.len()];
        for (path, frames) in &self.parts {
            let mut input = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
            let mut left = *frames;
            while left > 0 {
                let count = left.min(BLOCK as u64) as usize;
                input.read_exact(&mut buffer[..count * width])?;
                for bytes in buffer[..count * width].chunks_exact(width) {
                    decode(bytes, &mut frame);
                    if factor != 1.0 {
                        for value in frame.iter_mut().flatten() {
                            *value = (*value as f64 * factor).round() as i64;
                        }
                    }
                    output.push(&frame, &mut emit)?;
                }
                left -= count as u64;
            }
        }
        let report = output.finish(&self.groups, master, &mut emit)?;
        let limited = master.is_some() || self.groups.iter().any(Option::is_some);
        Ok(limited.then_some(report))
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        for (path, _) in &self.parts {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bessel I0 by its power series.
    fn bessel(x: f64) -> f64 {
        let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
        loop {
            term *= (x / (2.0 * k)) * (x / (2.0 * k));
            sum += term;
            if term < 1e-17 * sum {
                return sum;
            }
            k += 1.0;
        }
    }

    /// The tap table follows from its documented design.
    #[test]
    fn taps_are_the_documented_kaiser_sinc() {
        let raw = |tau: f64| -> Vec<f64> {
            (0..16)
                .map(|t| {
                    let d = t as f64 - 7.0 - tau;
                    let sinc = (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d);
                    sinc * bessel(5.0 * (1.0 - (d / 8.0).powi(2)).sqrt()) / bessel(5.0)
                })
                .collect()
        };
        let quantize = |c: Vec<f64>| -> Vec<i64> {
            let total: f64 = c.iter().sum();
            c.iter()
                .map(|v| (SCALE as f64 * v / total).round() as i64)
                .collect()
        };
        let mut quarter = quantize(raw(0.25));
        quarter[7] += SCALE - quarter.iter().sum::<i64>();
        let mut half = quantize(raw(0.5));
        let residue = SCALE - half.iter().sum::<i64>();
        half[7] += residue / 2;
        half[8] += residue / 2;
        let mut three = quarter.clone();
        three.reverse();
        assert_eq!(TAPS.map(|t| t.to_vec()), [quarter, half, three]);
        assert_eq!(GAIN_BOUND, 137_640);
    }

    /// A deterministic test signal: loud bursts with sharp edges over a quiet bed.
    fn signal(frames: usize) -> Vec<[i64; 2]> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..frames)
            .map(|n| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let noise = (state % 2001) as i64 - 1000;
                let burst = if (n / 900) % 5 == 0 { 30 } else { 1 };
                let tone = ((n as f64 * 0.37).sin() * 2000.0) as i64;
                [noise * burst + tone, (noise * burst) / 2 - tone]
            })
            .collect()
    }

    fn run(
        limiter: &Limiter,
        input: &[[i64; 2]],
        first: usize,
        end: usize,
        window: (usize, usize),
    ) -> Vec<[i64; 2]> {
        let mut stage = Stage::new(limiter, first as i64, (window.0 as i64, window.1 as i64));
        let latency = stage.latency();
        let mut out = Vec::new();
        let padded = input[first..end]
            .iter()
            .copied()
            .chain(std::iter::repeat_n([0, 0], latency));
        for (position, x) in (first as i64 - latency as i64..).zip(padded) {
            let y = stage.push(x);
            if (window.0 as i64..window.1 as i64).contains(&position) {
                out.push(y);
            }
        }
        out
    }

    /// No output sample passes the ceiling, and a window rendered with its reach of context
    /// equals the same samples of the whole render.
    #[test]
    fn limiter_holds_the_ceiling_and_windows_are_exact() {
        let input = signal(48_000);
        let limiter = Limiter {
            ceiling_dbfs: -3.0,
            lookahead_ms: 2,
            release_ms: 40,
        };
        let ceiling = limiter.ceiling() as i64;
        let whole = run(&limiter, &input, 0, input.len(), (0, input.len()));
        assert!(whole.iter().flatten().all(|v| v.abs() <= ceiling));
        // This full-band noise peaks between samples; those peaks are what the limiter holds.
        let mut meter = TruePeak::new();
        whole.iter().for_each(|f| meter.push(f.map(|v| v as i16)));
        let true_peak = meter.finish()[0].unwrap();
        assert!(
            (true_peak - limiter.ceiling_dbfs).abs() < 0.1,
            "{true_peak}"
        );
        let (before, after) = limiter.reach();
        for (a, b) in [(1000, 1500), (9000, 20_000), (47_000, 48_000)] {
            let first = (a as u64).saturating_sub(before) as usize;
            let end = ((b as u64 + after) as usize).min(input.len());
            assert_eq!(
                run(&limiter, &input, first, end, (a, b)),
                whole[a..b],
                "{a}..{b}"
            );
        }
        // Too little context changes the result: the reach is needed.
        let short = run(&limiter, &input, 1000 - 10, 1500 + 200, (1000, 1500));
        assert_ne!(short, whole[1000..1500]);
    }

    /// Below the ceiling (with room for the ringing of a step from silence) the limiter changes
    /// nothing; a long loud step is held exactly at the gain it requires.
    #[test]
    fn quiet_input_passes_and_steps_ramp() {
        let limiter = Limiter {
            ceiling_dbfs: -6.0,
            lookahead_ms: 1,
            release_ms: 10,
        };
        let quiet = vec![[10_000i64, -10_000]; 2000];
        assert_eq!(run(&limiter, &quiet, 0, 2000, (0, 2000)), quiet);
        // A constant 32000 for 1000 samples needs gain C/32000 in full.
        let mut step = vec![[0i64; 2]; 3000];
        for frame in &mut step[1000..2000] {
            *frame = [32_000, 32_000];
        }
        let out = run(&limiter, &step, 0, 3000, (0, 3000));
        let c = limiter.ceiling() as i64;
        assert!(out[1000..2000].iter().all(|f| f[0] <= c));
        // Constant input well inside the burst sits exactly at the required gain.
        let gain = ((u128::from(limiter.ceiling() * SCALE as u64) << 32) / (32_000 * SCALE as u128))
            as u64;
        assert_eq!(out[1500][0], apply(32_000, gain));
    }

    /// A capture split into parts replays the mix a limited render writes: the saturated sum
    /// without limiters, the limiter stage's own output through a master limiter, and with a
    /// factor, the same of the input scaled and rounded with ties away from zero.
    #[test]
    fn capture_replays_the_mix_at_any_level() {
        let input = signal(20_000);
        let dir = std::env::temp_dir().join(format!("cutbolt-capture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let parts = [(0, 7_001), (7_001, 20_000)]
            .iter()
            .enumerate()
            .map(|(i, &(a, b))| {
                let path = dir.join(format!("{i}.s32"));
                let bytes: Vec<u8> = input[a..b]
                    .iter()
                    .flatten()
                    .flat_map(|&v| (v as i32).to_le_bytes())
                    .collect();
                std::fs::write(&path, bytes).unwrap();
                (path, (b - a) as u64)
            })
            .collect();
        let capture = Capture {
            parts,
            groups: vec![None],
            frames: input.len() as u64,
        };
        let limiter = Limiter {
            ceiling_dbfs: -3.0,
            lookahead_ms: 2,
            release_ms: 40,
        };
        let replay = |factor: f64, master: Option<&Limiter>| {
            let mut out = Vec::new();
            let report = capture
                .replay(factor, master, |pair| out.push(pair))
                .unwrap();
            (out, report)
        };
        let pcm = |frames: &[[i64; 2]]| -> Vec<[i16; 2]> {
            frames
                .iter()
                .map(|f| f.map(|v| v.clamp(-32768, 32767) as i16))
                .collect()
        };
        let scaled: Vec<[i64; 2]> = input
            .iter()
            .map(|f| f.map(|v| (v as f64 * 1.37).round() as i64))
            .collect();
        let (plain, report) = replay(1.0, None);
        assert!(report.is_none());
        assert_eq!(plain, pcm(&input));
        assert_eq!(replay(1.37, None).0, pcm(&scaled));
        let (limited, report) = replay(1.0, Some(&limiter));
        assert_eq!(
            limited,
            pcm(&run(&limiter, &input, 0, input.len(), (0, input.len())))
        );
        assert!(report.unwrap().master.unwrap().1.reduced > 0);
        let whole = run(&limiter, &scaled, 0, scaled.len(), (0, scaled.len()));
        assert_eq!(replay(1.37, Some(&limiter)).0, pcm(&whole));
        drop(capture);
        assert!(!dir.join("0.s32").exists() && !dir.join("1.s32").exists());
        std::fs::remove_dir(&dir).unwrap();
    }

    /// The limiter detects peaks the way the meter measures them, so its output's true peak stays
    /// on the ceiling (within 0.01 dB) even under 20 dB of reduction with the shortest lookahead
    /// and release: full-band noise bursts, a gated tone near 11 kHz, clicks over a tone and a
    /// square wave.
    #[test]
    fn output_true_peak_stays_on_the_ceiling() {
        let frames = 12_000;
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut noise = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 20_001) as f64 - 10_000.0
        };
        let wave = |n: usize, hz: f64, phase: f64| {
            (2.0 * std::f64::consts::PI * hz * n as f64 / 48_000.0 + phase).sin()
        };
        let bursts: Vec<[f64; 2]> = (0..frames)
            .map(|n| {
                let on = if (n / 900) % 5 == 0 { 1.0 } else { 0.05 };
                [noise() * on, noise() * on]
            })
            .collect();
        let gated: Vec<[f64; 2]> = (0..frames)
            .map(|n| {
                let v = 20_000.0 * wave(n, 11_000.0, 0.3) * ((n / 2400) % 2) as f64;
                [v, -v]
            })
            .collect();
        let clicks: Vec<[f64; 2]> = (0..frames)
            .map(|n| {
                let click = match n % 4800 {
                    0 => 30_000.0,
                    1 => -30_000.0,
                    _ => 0.0,
                };
                let v = click + 3000.0 * wave(n, 7000.0, 0.0);
                [v, v]
            })
            .collect();
        let square: Vec<[f64; 2]> = (0..frames)
            .map(|n| {
                let v = 30_000.0 * wave(n, 3.0, 0.0).signum();
                [v, v]
            })
            .collect();
        let mut worst = f64::NEG_INFINITY;
        for signal in [&bursts, &gated, &clicks, &square] {
            for scale in [2.0, 10.0] {
                let input: Vec<[i64; 2]> = signal
                    .iter()
                    .map(|f| f.map(|v| (v * scale).round() as i64))
                    .collect();
                for (lookahead_ms, release_ms) in [(1, 10), (5, 150)] {
                    for ceiling_dbfs in [-1.0, -5.02] {
                        let limiter = Limiter {
                            ceiling_dbfs,
                            lookahead_ms,
                            release_ms,
                        };
                        let output = run(&limiter, &input, 0, frames, (0, frames));
                        let mut meter = TruePeak::new();
                        output.iter().for_each(|f| meter.push(f.map(|v| v as i16)));
                        let [left, right] = meter.finish();
                        let peak = left.unwrap().max(right.unwrap());
                        worst = worst.max(peak - ceiling_dbfs);
                        assert!(
                            peak <= ceiling_dbfs + 0.01,
                            "{peak} dBTP over {ceiling_dbfs} at {lookahead_ms}/{release_ms} ms, x{scale}"
                        );
                    }
                }
            }
        }
        // The ceiling is reached, not undershot by a margin.
        assert!(worst > -0.05, "{worst}");
    }

    /// The true-peak meter finds the intersample peak of a quarter-rate sine at 45 degrees.
    #[test]
    fn true_peak_finds_intersample_peaks() {
        let mut meter = TruePeak::new();
        // Samples at 45 + 90n degrees of a 0.5-amplitude sine read 0.354; the peak is 0.5. Faded
        // ends keep the ringing of an abrupt start out of the reading.
        for n in 0..4800 {
            let fade = (n.min(4799 - n) as f64 / 480.0).min(1.0);
            let v = (fade
                * 0.5
                * 32768.0
                * (std::f64::consts::FRAC_PI_4 + n as f64 * std::f64::consts::FRAC_PI_2).sin())
            .round() as i16;
            meter.push([v, 0]);
        }
        let [left, right] = meter.finish();
        assert!(
            (left.unwrap() - 20.0 * 0.5f64.log10()).abs() < 0.05,
            "{left:?}"
        );
        assert_eq!(right, None);
    }
}
