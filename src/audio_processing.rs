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

/// Final PCM measurement: stereo only, 48 kHz block counts, no true-peak claim.
pub(crate) fn meters(pcm: &[i16]) -> Value {
    // Public 48 kHz coefficients: BS.1770-5 Annex 1, Tables 1 and 2.
    let mut shelf = Biquad::new(
        [1.53512485958697, -2.69169618940638, 1.19839281085285],
        [1.0, -1.69065929318241, 0.73248077421585],
    );
    let mut highpass = Biquad::new([1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]);
    const WINDOW: usize = 19200;
    const HOP: usize = 4800;
    let frames = pcm.len() / 2;
    let mut window = vec![0.0; WINDOW];
    let mut sum = 0.0;
    let mut blocks = Vec::new();
    let mut squares = [0.0; 2];
    let mut peaks = [0.0_f64; 2];
    for (n, pair) in pcm.as_chunks::<2>().0.iter().enumerate() {
        let mut energy = 0.0;
        for ch in 0..2 {
            let x = pair[ch] as f64 / 32768.0;
            peaks[ch] = peaks[ch].max(x.abs());
            squares[ch] += x * x;
            let weighted = highpass.sample(shelf.sample(x, ch), ch);
            energy += weighted * weighted;
        }
        let slot = n % WINDOW;
        sum += energy - window[slot];
        window[slot] = energy;
        if n + 1 >= WINDOW && (n + 1 - WINDOW).is_multiple_of(HOP) {
            blocks.push(sum.max(0.0) / WINDOW as f64);
        }
    }
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
