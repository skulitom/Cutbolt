//! Original master processing and stereo meters from public DSP equations.
//! EQ: W3C Audio EQ Cookbook (2021). Loudness: ITU-R BS.1770-5 Annex 1.
use crate::{Result, error};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One EQ or compression stage, tagged by `type`, for mix masters, buses and repair recipes. All values must be finite.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    /// Two-pole low-pass filter.
    LowPass {
        /// Cutoff frequency in Hz, 20..20000.
        frequency_hz: f64,
        /// Filter Q, 0.1..20.
        q: f64,
    },
    /// Two-pole high-pass filter.
    HighPass {
        /// Cutoff frequency in Hz, 20..20000.
        frequency_hz: f64,
        /// Filter Q, 0.1..20.
        q: f64,
    },
    /// Two-pole peaking EQ band.
    Peaking {
        /// Center frequency in Hz, 20..20000.
        frequency_hz: f64,
        /// Filter Q, 0.1..20.
        q: f64,
        /// Band gain in dB, -24..24.
        gain_db: f64,
    },
    /// Hard-knee peak compressor; detection is linked across all channels.
    Compressor {
        /// Threshold in dBFS, -60..0.
        threshold_db: f64,
        /// Compression ratio, 1..20.
        ratio: f64,
        /// Attack time in milliseconds, 0..1000; 0 applies the reduction immediately.
        attack_ms: f64,
        /// Release time in milliseconds, 0..5000.
        release_ms: f64,
        /// Makeup gain in dB applied after reduction, 0..24.
        makeup_db: f64,
    },
}

fn bounded(x: f64, low: f64, high: f64) -> bool {
    x.is_finite() && (low..=high).contains(&x)
}
pub(crate) fn validate(effects: &[Effect]) -> Result<()> {
    if effects.len() > 8 {
        return Err(error(
            "INVALID_AUDIO_EFFECT",
            "At most eight master effects are supported",
        ));
    }
    for effect in effects {
        let valid = match *effect {
            Effect::LowPass { frequency_hz, q } | Effect::HighPass { frequency_hz, q } => {
                bounded(frequency_hz, 20.0, 20000.0) && bounded(q, 0.1, 20.0)
            }
            Effect::Peaking {
                frequency_hz,
                q,
                gain_db,
            } => {
                bounded(frequency_hz, 20.0, 20000.0)
                    && bounded(q, 0.1, 20.0)
                    && bounded(gain_db, -24.0, 24.0)
            }
            Effect::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => {
                bounded(threshold_db, -60.0, 0.0)
                    && bounded(ratio, 1.0, 20.0)
                    && bounded(attack_ms, 0.0, 1000.0)
                    && bounded(release_ms, 0.0, 5000.0)
                    && bounded(makeup_db, 0.0, 24.0)
            }
        };
        if !valid {
            return Err(error(
                "INVALID_AUDIO_EFFECT",
                "Audio effect parameter is nonfinite or outside its supported range",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    state: [[f64; 2]; 8],
}
impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b: b.map(|x| x / a[0]),
            a: [a[1] / a[0], a[2] / a[0]],
            state: [[0.0; 2]; 8],
        }
    }
    fn sample(&mut self, x: f64, channel: usize) -> f64 {
        let state = &mut self.state[channel];
        let y = self.b[0] * x + state[0];
        state[0] = self.b[1] * x - self.a[0] * y + state[1];
        state[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

fn equalizer(effect: &Effect) -> Option<Biquad> {
    let (frequency, q) = match *effect {
        Effect::LowPass { frequency_hz, q }
        | Effect::HighPass { frequency_hz, q }
        | Effect::Peaking {
            frequency_hz, q, ..
        } => (frequency_hz, q),
        _ => return None,
    };
    let omega = std::f64::consts::TAU * frequency / 48000.0;
    let c = omega.cos();
    let alpha = omega.sin() / (2.0 * q);
    let a = [1.0 + alpha, -2.0 * c, 1.0 - alpha];
    Some(match *effect {
        Effect::LowPass { .. } => Biquad::new([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], a),
        Effect::HighPass { .. } => Biquad::new([(1.0 + c) / 2.0, -1.0 - c, (1.0 + c) / 2.0], a),
        Effect::Peaking { gain_db, .. } => {
            let gain = 10.0_f64.powf(gain_db / 40.0);
            Biquad::new(
                [1.0 + alpha * gain, -2.0 * c, 1.0 - alpha * gain],
                [1.0 + alpha / gain, -2.0 * c, 1.0 - alpha / gain],
            )
        }
        _ => unreachable!("only EQ variants pass the first match"),
    })
}

fn smoothing(ms: f64) -> f64 {
    if ms == 0.0 {
        0.0
    } else {
        (-1.0 / (48.0 * ms)).exp()
    }
}

enum Stage {
    Eq(Biquad),
    Compressor {
        threshold_db: f64,
        ratio: f64,
        attack: f64,
        release: f64,
        makeup_db: f64,
        reduction: f64,
    },
}
pub(crate) struct Processor {
    stages: Vec<Stage>,
    channels: usize,
}
impl Processor {
    pub fn new(effects: &[Effect], channels: usize) -> Result<Self> {
        validate(effects)?;
        if !(1..=8).contains(&channels) {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Processing requires 1..8 declared channels",
            ));
        }
        let stages = effects
            .iter()
            .map(|effect| {
                if let Some(filter) = equalizer(effect) {
                    Stage::Eq(filter)
                } else if let Effect::Compressor {
                    threshold_db,
                    ratio,
                    attack_ms,
                    release_ms,
                    makeup_db,
                } = *effect
                {
                    Stage::Compressor {
                        threshold_db,
                        ratio,
                        attack: smoothing(attack_ms),
                        release: smoothing(release_ms),
                        makeup_db,
                        reduction: 0.0,
                    }
                } else {
                    unreachable!("validated processing type")
                }
            })
            .collect();
        Ok(Self { stages, channels })
    }
    pub fn frame(&mut self, wide: &mut [i64]) -> Result<()> {
        if wide.len() != self.channels {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Processing channel layout changed",
            ));
        }
        if self.stages.is_empty() {
            return Ok(());
        }
        let mut samples = [0.0_f64; 8];
        let samples = &mut samples[..self.channels];
        for (s, w) in samples.iter_mut().zip(wide.iter()) {
            *s = *w as f64 / 32768.0;
        }
        for stage in &mut self.stages {
            match stage {
                Stage::Eq(filter) => {
                    for (ch, value) in samples.iter_mut().enumerate() {
                        *value = filter.sample(*value, ch);
                    }
                }
                Stage::Compressor {
                    threshold_db,
                    ratio,
                    attack,
                    release,
                    makeup_db,
                    reduction,
                } => {
                    let peak = samples.iter().fold(0.0_f64, |a, b| a.max(b.abs()));
                    let over = if peak > 0.0 {
                        (20.0 * peak.log10() - *threshold_db).max(0.0)
                    } else {
                        0.0
                    };
                    let target = -over * (1.0 - 1.0 / *ratio);
                    let coefficient = if target < *reduction {
                        *attack
                    } else {
                        *release
                    };
                    *reduction = coefficient * *reduction + (1.0 - coefficient) * target;
                    let gain = 10.0_f64.powf((*reduction + *makeup_db) / 20.0);
                    for value in samples.iter_mut() {
                        *value *= gain;
                    }
                }
            }
            if samples.iter().any(|x| !x.is_finite() || x.abs() > 1.0e12) {
                return Err(error(
                    "AUDIO_PROCESSING_OVERFLOW",
                    "Effect chain exceeded finite supported headroom",
                ));
            }
        }
        for (output, value) in wide.iter_mut().zip(samples) {
            *output = (*value * 32768.0).round() as i64;
        }
        Ok(())
    }
}
pub(crate) fn process(wide: &mut [i64], effects: &[Effect]) -> Result<()> {
    let mut processor = Processor::new(effects, 2)?;
    if effects.is_empty() {
        return Ok(());
    }
    for pair in wide.as_chunks_mut::<2>().0 {
        processor.frame(pair)?;
    }
    Ok(())
}

fn decibels(power: f64) -> Option<f64> {
    (power > 0.0).then(|| 10.0 * power.log10())
}

/// Silence: every channel within this many codes of zero (-60 dBFS).
const SILENT_CODE: i32 = 32;
/// Shortest reported silent run: half a second at 48 kHz.
const SILENT_SAMPLES: usize = 24_000;
/// Runs listed per kind; counts stay exact.
const MAX_RUNS: usize = 50;

/// K-weighting filters: public 48 kHz coefficients, BS.1770-5 Annex 1, Tables 1 and 2.
fn k_weighting() -> (Biquad, Biquad) {
    (
        Biquad::new(
            [1.53512485958697, -2.69169618940638, 1.19839281085285],
            [1.0, -1.69065929318241, 0.73248077421585],
        ),
        Biquad::new([1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]),
    )
}

/// Loudness over time and level events of final stereo 48 kHz PCM, for reviewing a timeline.
/// For each whole second it gives the short-term loudness of the 3 s ending there and the
/// loudest momentary (400 ms, 100 ms hop) loudness ending inside it, K-weighted as in `meters`
/// and rounded to 0.1 LKFS (null below the -70 LKFS absolute gate). It also lists silent runs (at most -60 dBFS
/// for 0.5 s or more) and runs of clipped samples (full-scale codes), with exact sample times.
/// Callers stream through [`Profile`] or [`measure_wav`]; this slice form serves the unit tests.
#[cfg(test)]
pub(crate) fn profile(pcm: &[i16]) -> Value {
    let mut profile = Profile::new();
    pcm.as_chunks::<2>()
        .0
        .iter()
        .for_each(|pair| profile.push(*pair));
    profile.finish()
}

/// Streaming form of [`profile`]: push stereo frames in order, then finish.
pub(crate) struct Profile {
    shelf: Biquad,
    highpass: Biquad,
    blocks: Vec<f64>,
    energy: f64,
    silent_from: Option<usize>,
    silent: Vec<(usize, usize)>,
    clipped_from: Option<usize>,
    clipped: Vec<(usize, usize)>,
    clipped_samples: usize,
    frames: usize,
}
impl Profile {
    const BLOCK: usize = 4800;
    pub(crate) fn new() -> Self {
        let (shelf, highpass) = k_weighting();
        Self {
            shelf,
            highpass,
            blocks: Vec::new(),
            energy: 0.0,
            silent_from: None,
            silent: Vec::new(),
            clipped_from: None,
            clipped: Vec::new(),
            clipped_samples: 0,
            frames: 0,
        }
    }
    pub(crate) fn push(&mut self, pair: [i16; 2]) {
        let n = self.frames;
        self.frames += 1;
        for (ch, &sample) in pair.iter().enumerate() {
            let x = sample as f64 / 32768.0;
            let weighted = self.highpass.sample(self.shelf.sample(x, ch), ch);
            self.energy += weighted * weighted;
        }
        if (n + 1).is_multiple_of(Self::BLOCK) {
            self.blocks.push(self.energy);
            self.energy = 0.0;
        }
        let quiet = pair.iter().all(|&s| i32::from(s).abs() <= SILENT_CODE);
        match (quiet, self.silent_from) {
            (true, None) => self.silent_from = Some(n),
            (false, Some(from)) => {
                if n - from >= SILENT_SAMPLES {
                    self.silent.push((from, n));
                }
                self.silent_from = None;
            }
            _ => {}
        }
        let clip = pair.iter().any(|&s| s == i16::MAX || s == i16::MIN);
        self.clipped_samples += pair
            .iter()
            .filter(|&&s| s == i16::MAX || s == i16::MIN)
            .count();
        match (clip, self.clipped_from) {
            (true, None) => self.clipped_from = Some(n),
            (false, Some(from)) => {
                self.clipped.push((from, n));
                self.clipped_from = None;
            }
            _ => {}
        }
    }
    pub(crate) fn finish(mut self) -> Value {
        const BLOCK: usize = Profile::BLOCK;
        let frames = self.frames;
        // Exact sample times, reduced like every other engine time.
        let time = |n: usize| json!(crate::time::Time::new(n as u64, 48_000).expect("sample time"));
        if let Some(from) = self
            .silent_from
            .filter(|from| frames - from >= SILENT_SAMPLES)
        {
            self.silent.push((from, frames));
        }
        if let Some(from) = self.clipped_from {
            self.clipped.push((from, frames));
        }
        let blocks = &self.blocks;
        // Below the -70 LKFS absolute gate, as in integrated loudness, reads as silence (null).
        let loudness = |sum: f64, count: usize| {
            decibels(sum / (count * BLOCK) as f64)
                .map(|x| x - 0.691)
                .filter(|x| *x >= -70.0)
                .map(|x| (x * 10.0).round() / 10.0)
        };
        let seconds = blocks.len() / 10;
        let mut short_term = Vec::with_capacity(seconds);
        let mut momentary = Vec::with_capacity(seconds);
        for second in 0..seconds {
            let end = (second + 1) * 10;
            let first = end.saturating_sub(30);
            short_term.push(loudness(blocks[first..end].iter().sum(), end - first));
            let loudest = (end - 9..=end)
                .filter(|&e| e >= 4)
                .filter_map(|e| loudness(blocks[e - 4..e].iter().sum(), 4))
                .fold(None, |best: Option<f64>, x| {
                    Some(best.map_or(x, |b| b.max(x)))
                });
            momentary.push(loudest);
        }
        let runs = |runs: &[(usize, usize)]| {
            runs.iter()
                .take(MAX_RUNS)
                .map(|&(from, to)| json!({"start":time(from),"end":time(to)}))
                .collect::<Vec<_>>()
        };
        json!({"step_seconds":1,"short_term_lkfs":short_term,"momentary_max_lkfs":momentary,
            "silence":{"threshold_dbfs":-60,"minimum_seconds":0.5,"count":self.silent.len(),"runs":runs(&self.silent)},
            "clipping":{"clipped_samples":self.clipped_samples,"count":self.clipped.len(),"runs":runs(&self.clipped)}})
    }
}

/// Final PCM measurement: stereo only, 48 kHz block counts, no true-peak claim.
pub(crate) fn meters(pcm: &[i16]) -> Value {
    let mut meter = Meter::new();
    pcm.as_chunks::<2>()
        .0
        .iter()
        .for_each(|pair| meter.push(*pair));
    meter.finish()
}

/// Streaming form of [`meters`]: push stereo frames in order, then finish.
pub(crate) struct Meter {
    shelf: Biquad,
    highpass: Biquad,
    window: Vec<f64>,
    sum: f64,
    blocks: Vec<f64>,
    squares: [f64; 2],
    peaks: [f64; 2],
    frames: usize,
}
impl Meter {
    const WINDOW: usize = 19200;
    const HOP: usize = 4800;
    pub(crate) fn new() -> Self {
        let (shelf, highpass) = k_weighting();
        Self {
            shelf,
            highpass,
            window: vec![0.0; Self::WINDOW],
            sum: 0.0,
            blocks: Vec::new(),
            squares: [0.0; 2],
            peaks: [0.0; 2],
            frames: 0,
        }
    }
    pub(crate) fn push(&mut self, pair: [i16; 2]) {
        let n = self.frames;
        self.frames += 1;
        let mut energy = 0.0;
        for (ch, &sample) in pair.iter().enumerate() {
            let x = sample as f64 / 32768.0;
            self.peaks[ch] = self.peaks[ch].max(x.abs());
            self.squares[ch] += x * x;
            let weighted = self.highpass.sample(self.shelf.sample(x, ch), ch);
            energy += weighted * weighted;
        }
        let slot = n % Self::WINDOW;
        self.sum += energy - self.window[slot];
        self.window[slot] = energy;
        if n + 1 >= Self::WINDOW && (n + 1 - Self::WINDOW).is_multiple_of(Self::HOP) {
            self.blocks.push(self.sum.max(0.0) / Self::WINDOW as f64);
        }
    }
    pub(crate) fn finish(self) -> Value {
        const WINDOW: usize = Meter::WINDOW;
        const HOP: usize = Meter::HOP;
        let (frames, blocks, squares, peaks) = (self.frames, self.blocks, self.squares, self.peaks);
        let absolute = 10.0_f64.powf((-70.0 + 0.691) / 10.0);
        let above: Vec<_> = blocks.iter().copied().filter(|x| *x > absolute).collect();
        let relative = if above.is_empty() {
            None
        } else {
            Some(above.iter().sum::<f64>() / above.len() as f64 * 0.1)
        };
        let gated: Vec<_> = above
            .iter()
            .copied()
            .filter(|x| *x > relative.unwrap_or(f64::INFINITY))
            .collect();
        let loudness = if gated.is_empty() {
            None
        } else {
            decibels(gated.iter().sum::<f64>() / gated.len() as f64).map(|x| x - 0.691)
        };
        let status = if frames < WINDOW {
            "insufficient_duration"
        } else if loudness.is_none() {
            "below_gate"
        } else {
            "measured"
        };
        json!({"profile":"stereo-48k-meters-v1", "measured_signal":"final_pcm16", "sample_peak_dbfs":peaks.map(|x| decibels(x*x)),
            "rms_dbfs":squares.map(|x| decibels(x / frames as f64)), "integrated_lkfs":loudness,
            "loudness_status":status,"relative_gate_lkfs":relative.and_then(decibels).map(|x| x-0.691),
            "gating_blocks":blocks.len(),"absolute_gated_blocks":above.len(),"relative_gated_blocks":gated.len(),
            "unmeasured_tail_samples":if frames < WINDOW {frames} else {(frames-WINDOW)%HOP},
            "true_peak_available":false})
    }
}

/// Meter a 48 kHz stereo PCM16 WAV file as it streams from disk, so length costs no memory:
/// its frame count, [`meters`] and, with `curve`, [`profile`].
pub(crate) fn measure_wav(
    path: &std::path::Path,
    curve: bool,
) -> Result<(u64, Value, Option<Value>)> {
    use std::io::Read;
    let mut input = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
    let mut header = [0u8; 12];
    input.read_exact(&mut header)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return Err(error("INVALID_AUDIO", "Expected a RIFF WAVE file"));
    }
    let size = loop {
        let mut chunk = [0u8; 8];
        input.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().expect("chunk size")) as u64;
        if &chunk[..4] == b"data" {
            break size;
        }
        std::io::copy(
            &mut (&mut input).take(size + size % 2),
            &mut std::io::sink(),
        )?;
    };
    let mut meter = Meter::new();
    let mut profile = curve.then(Profile::new);
    let mut buffer = vec![0u8; 1 << 16];
    let mut left = size - size % 4;
    while left > 0 {
        let part = &mut buffer[..(left as usize).min(1 << 16)];
        input.read_exact(part)?;
        for frame in part.as_chunks::<4>().0 {
            let pair = [
                i16::from_le_bytes([frame[0], frame[1]]),
                i16::from_le_bytes([frame[2], frame[3]]),
            ];
            meter.push(pair);
            if let Some(profile) = &mut profile {
                profile.push(pair);
            }
        }
        left -= part.len() as u64;
    }
    Ok((size / 4, meter.finish(), profile.map(Profile::finish)))
}

/// Sample peaks/RMS for named multichannel output; loudness stays explicitly unmeasured.
pub(crate) fn channel_meters(pcm: &[i16], channels: usize) -> Value {
    if channels == 2 {
        return meters(pcm);
    }
    let mut peaks = vec![0.0_f64; channels];
    let mut squares = vec![0.0; channels];
    for (i, sample) in pcm.iter().enumerate() {
        let x = *sample as f64 / 32768.0;
        peaks[i % channels] = peaks[i % channels].max(x.abs());
        squares[i % channels] += x * x;
    }
    let frames = pcm.len() / channels;
    json!({"profile":"channel-48k-meters-v1","measured_signal":"final_pcm16", "sample_peak_dbfs":peaks.iter().map(|x| decibels(x*x)).collect::<Vec<_>>(),
        "rms_dbfs":squares.iter().map(|x| decibels(x / frames as f64)).collect::<Vec<_>>(), "integrated_lkfs":null,"loudness_status":"unsupported_layout","true_peak":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a stereo sine at `amplitude` codes and 1 kHz.
    fn tone(amplitude: f64) -> Vec<i16> {
        (0..48_000)
            .flat_map(|n| {
                let x = (amplitude
                    * (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / 48_000.0).sin())
                .round() as i16;
                [x, x]
            })
            .collect()
    }

    #[test]
    fn profile_reports_loudness_per_second_silence_and_clipping() {
        // 2 s of tone, 2 s of silence, 1 s of a louder tone with three clipped frames.
        let mut pcm = tone(3000.0);
        pcm.extend(tone(3000.0));
        pcm.extend(vec![0; 192_000]);
        let mut loud = tone(12_000.0);
        for n in [1000, 1001, 30_000] {
            loud[2 * n] = i16::MAX;
            loud[2 * n + 1] = i16::MIN;
        }
        pcm.extend(loud);
        let profile = profile(&pcm);
        let short: Vec<Option<f64>> =
            serde_json::from_value(profile["short_term_lkfs"].clone()).unwrap();
        let momentary: Vec<Option<f64>> =
            serde_json::from_value(profile["momentary_max_lkfs"].clone()).unwrap();
        assert_eq!((short.len(), momentary.len()), (5, 5));
        // The fourth second is wholly silent: no momentary loudness, but its 3 s short-term window
        // still reaches the tone.
        assert!(momentary[3].is_none() && short[3].is_some());
        // Four times the amplitude is about 12 dB louder.
        let gain = momentary[4].unwrap() - momentary[1].unwrap();
        assert!((gain - 12.0).abs() < 0.2, "{gain}");
        // The run ends one sample into the loud tone, whose first sample, sin(0), is silent.
        assert_eq!(
            profile["silence"]["runs"],
            json!([{"start":{"num":2,"den":1},"end":{"num":192001,"den":48000}}])
        );
        assert_eq!(profile["clipping"]["clipped_samples"], 6);
        assert_eq!(profile["clipping"]["count"], 2);
        assert_eq!(
            profile["clipping"]["runs"][0],
            json!({"start":{"num":193,"den":48},"end":{"num":32167,"den":8000}})
        );
    }
}
