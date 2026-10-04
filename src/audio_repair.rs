//! Original fixed-profile spectral suppression and sample-preserving PCM cleanup.
use crate::{
    Result, audio,
    audio_processing::{self, Effect, Processor},
    error, media,
    pcm_wave::{self, Layout},
    render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const N: usize = 4096;
const HOP: usize = 1024;
const RATE: Time = Time { num: 48000, den: 1 };
fn strength() -> u32 {
    1250
}
fn floor() -> u32 {
    100
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_AUDIO_REPAIR", message)
}
fn sample_index(time: Time) -> Result<usize> {
    usize::try_from(time.units(RATE)?)
        .map_err(|_| error("TIME_OVERFLOW", "Sample index exceeds the platform range"))
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub start: Time,
    pub duration: Time,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Noise {
    pub regions: Vec<Region>,
    #[serde(default = "strength")]
    pub strength_milli: u32,
    #[serde(default = "floor")]
    pub floor_milli: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub id: String,
    pub source: Identity,
    pub source_in: Time,
    pub duration: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    #[serde(default)]
    pub remove_dc: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

#[derive(Clone, Copy, Default)]
struct Complex {
    re: f64,
    im: f64,
}
struct Transform {
    reverse: Vec<usize>,
    roots: Vec<Complex>,
}
impl Transform {
    fn new() -> Self {
        Self {
            reverse: (0..N)
                .map(|i| i.reverse_bits() >> (usize::BITS - N.trailing_zeros()))
                .collect(),
            roots: (0..N / 2)
                .map(|i| {
                    let angle = -std::f64::consts::TAU * i as f64 / N as f64;
                    Complex {
                        re: angle.cos(),
                        im: angle.sin(),
                    }
                })
                .collect(),
        }
    }
    fn run(&self, values: &mut [Complex], inverse: bool) {
        for (i, j) in self.reverse.iter().copied().enumerate() {
            if i < j {
                values.swap(i, j);
            }
        }
        let mut width = 2;
        while width <= N {
            for block in values.chunks_exact_mut(width) {
                let (a, b) = block.split_at_mut(width / 2);
                for (i, (a, b)) in a.iter_mut().zip(b).enumerate() {
                    let root = self.roots[i * N / width];
                    let im = if inverse { -root.im } else { root.im };
                    let re = b.re * root.re - b.im * im;
                    let im = b.re * im + b.im * root.re;
                    *b = Complex {
                        re: a.re - re,
                        im: a.im - im,
                    };
                    a.re += re;
                    a.im += im;
                }
            }
            width *= 2;
        }
        if inverse {
            for value in values {
                value.re /= N as f64;
                value.im /= N as f64;
            }
        }
    }
}
struct Suppressor {
    transform: Transform,
    window: Vec<f64>,
    bins: Vec<Complex>,
}
impl Suppressor {
    fn new() -> Self {
        Self {
            transform: Transform::new(),
            window: (0..N)
                .map(|i| {
                    (std::f64::consts::PI * (i as f64 + 0.5) / N as f64)
                        .sin()
                        .powi(2)
                })
                .collect(),
            bins: vec![Complex::default(); N],
        }
    }
    fn profile(
        &mut self,
        source: &pcm_wave::Decoded,
        channel: usize,
        regions: &[(usize, usize)],
    ) -> (Vec<f64>, usize) {
        let mut power = vec![0.0; N / 2 + 1];
        let mut windows = 0;
        let channels = source.layout.channels();
        for &(start, length) in regions {
            for offset in (start..=start + length - N).step_by(HOP) {
                for (i, bin) in self.bins.iter_mut().enumerate() {
                    *bin = Complex {
                        re: source.data[(offset + i) * channels + channel] as f64 * self.window[i],
                        im: 0.0,
                    };
                }
                self.transform.run(&mut self.bins, false);
                for (p, value) in power.iter_mut().zip(&self.bins) {
                    *p += value.re * value.re + value.im * value.im;
                }
                windows += 1;
            }
        }
        for p in &mut power {
            *p /= windows as f64;
        }
        (power, windows)
    }
    fn channel(
        &mut self,
        input: &[i16],
        channels: usize,
        channel: usize,
        power: &[f64],
        noise: &Noise,
        output: &mut [f64],
    ) -> Value {
        if noise.strength_milli == 0 || noise.floor_milli == 1000 || power.iter().all(|p| *p == 0.0)
        {
            for (out, frame) in output
                .chunks_exact_mut(channels)
                .zip(input.chunks_exact(channels))
            {
                out[channel] = frame[channel] as f64;
            }
            return json!({"bypassed":true,"minimum_gain":1.0,"mean_gain":1.0,"windows":0});
        }
        let samples = input.len() / channels;
        let mut sum = vec![0.0; N];
        let mut normalization = vec![0.0; N];
        let mut gain = vec![0.0; N / 2 + 1];
        let mut minimum = 1.0_f64;
        let mut total = 0.0;
        let mut windows = 0;
        let strength = noise.strength_milli as f64 / 1000.0;
        let floor = (noise.floor_milli as f64 / 1000.0).powi(2);
        // A ring holds only one overlap window. Once a hop is finalized no later
        // window can contribute to those samples, including at the padded edges.
        let mut start = -(N as i64 - HOP as i64);
        while start < samples as i64 {
            for (i, bin) in self.bins.iter_mut().enumerate() {
                let at = start + i as i64;
                let x = if (0..samples as i64).contains(&at) {
                    input[at as usize * channels + channel] as f64
                } else {
                    0.0
                };
                *bin = Complex {
                    re: x * self.window[i],
                    im: 0.0,
                };
            }
            self.transform.run(&mut self.bins, false);
            for (i, g) in gain.iter_mut().enumerate() {
                let p = self.bins[i].re.powi(2) + self.bins[i].im.powi(2);
                *g = if p == 0.0 {
                    if power[i] == 0.0 { 1.0 } else { floor.sqrt() }
                } else {
                    (1.0 - strength * power[i] / p).max(floor).sqrt()
                };
            }
            for i in 0..=N / 2 {
                let g = [1.0, 2.0, 3.0, 2.0, 1.0]
                    .iter()
                    .enumerate()
                    .map(|(k, weight)| {
                        gain[(i as i64 + k as i64 - 2).clamp(0, (N / 2) as i64) as usize] * weight
                    })
                    .sum::<f64>()
                    / 9.0;
                minimum = minimum.min(g);
                total += g;
                self.bins[i].re *= g;
                self.bins[i].im *= g;
                if i > 0 && i < N / 2 {
                    self.bins[N - i].re *= g;
                    self.bins[N - i].im *= g;
                }
            }
            self.transform.run(&mut self.bins, true);
            for i in 0..N {
                let slot = (start + i as i64).rem_euclid(N as i64) as usize;
                sum[slot] += self.bins[i].re * self.window[i];
                normalization[slot] += self.window[i].powi(2);
            }
            for i in 0..HOP {
                let at = start + i as i64;
                let slot = at.rem_euclid(N as i64) as usize;
                if (0..samples as i64).contains(&at) {
                    output[at as usize * channels + channel] = sum[slot] / normalization[slot];
                }
                sum[slot] = 0.0;
                normalization[slot] = 0.0;
            }
            windows += 1;
            start += HOP as i64;
        }
        json!({"bypassed":false,"minimum_gain":minimum,"mean_gain":total/(windows*(N/2+1)) as f64,"windows":windows})
    }
}
struct Prepared {
    pcm: Vec<i16>,
    layout: Layout,
    report: Value,
}
fn prepare(recipe: &Recipe, root: &Path) -> Result<Prepared> {
    audio_processing::validate(&recipe.effects)?;
    let count = sample_index(recipe.duration)?;
    let source_in = sample_index(recipe.source_in)?;
    if recipe.schema_version != 1
        || !audio::id_ok(&recipe.id)
        || count == 0
        || count > 60 * 48000
        || recipe.source.bytes > 64 * 1024 * 1024
    {
        return Err(invalid(
            "Repair v1 requires an ID, 1 sample to 60 seconds and at most 64 MiB of PCM source",
        ));
    }
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Input root must be an existing absolute directory",
        ));
    }
    let mut regions = Vec::new();
    if let Some(noise) = &recipe.noise {
        if noise.regions.is_empty()
            || noise.regions.len() > 8
            || noise.strength_milli > 3000
            || noise.floor_milli > 1000
        {
            return Err(invalid(
                "Noise needs 1..8 regions, strength 0..3000 and floor 0..1000",
            ));
        }
        let mut total = 0u128;
        for region in &noise.regions {
            let start = sample_index(region.start)?;
            let length = sample_index(region.duration)?;
            total += length as u128;
            if length < N || total > 10 * 48000 {
                return Err(invalid(
                    "Each noise region needs 4096 samples; combined profiles must not exceed ten seconds",
                ));
            }
            regions.push((start, length));
        }
        regions.sort_unstable();
        if regions
            .windows(2)
            .any(|w| w[0].0 as u128 + w[0].1 as u128 > w[1].0 as u128)
        {
            return Err(invalid("Noise-only regions must not overlap"));
        }
    }
    let (_, bytes) = scene::identity_bytes(&recipe.source, root)?;
    let source = pcm_wave::decode(&bytes)?;
    drop(bytes);
    if source.rate != 48000 || !matches!(source.layout, Layout::Mono | Layout::Stereo) {
        return Err(error(
            "UNSUPPORTED_AUDIO",
            "Dialogue repair requires 48 kHz mono or stereo PCM16",
        ));
    }
    let channels = source.layout.channels();
    let frames = source.data.len() / channels;
    if source_in as u128 + count as u128 > frames as u128
        || regions
            .iter()
            .any(|(start, length)| *start as u128 + *length as u128 > frames as u128)
    {
        return Err(error(
            "INVALID_RANGE",
            "Repair or noise-profile range exceeds the original source",
        ));
    }
    let input = &source.data[source_in * channels..(source_in + count) * channels];
    let mut samples = vec![0.0; input.len()];
    let mut profile_reports = Vec::new();
    if let Some(noise) = &recipe.noise {
        let mut suppressor = Suppressor::new();
        for channel in 0..channels {
            let (power, windows) = suppressor.profile(&source, channel, &regions);
            let mut report =
                suppressor.channel(input, channels, channel, &power, noise, &mut samples);
            report["noise_power"] = json!(power);
            report["profile_windows"] = json!(windows);
            profile_reports.push(report);
        }
    } else {
        for (out, value) in samples.iter_mut().zip(input) {
            *out = *value as f64;
        }
    }
    let mut dc = vec![0.0; channels];
    if recipe.remove_dc {
        for frame in samples.chunks_exact(channels) {
            for (sum, x) in dc.iter_mut().zip(frame) {
                *sum += x;
            }
        }
        for sum in &mut dc {
            *sum /= count as f64;
        }
    }
    let mut processor = Processor::new(&recipe.effects, channels)?;
    let mut pcm = Vec::with_capacity(samples.len());
    let mut clipped = vec![0u64; channels];
    let mut peak = vec![0i32; channels];
    let mut hash = Sha256::new();
    for frame in samples.chunks_exact(channels) {
        let mut wide = [0i64; 2];
        for ch in 0..channels {
            if !frame[ch].is_finite() {
                return Err(error(
                    "AUDIO_PROCESSING_OVERFLOW",
                    "Nonfinite spectral reconstruction",
                ));
            }
            wide[ch] = (frame[ch] - dc[ch]).round() as i64;
        }
        processor.frame(&mut wide[..channels])?;
        for ch in 0..channels {
            if !(-1_000_000_000_000..=1_000_000_000_000).contains(&wide[ch]) {
                return Err(error(
                    "AUDIO_PROCESSING_OVERFLOW",
                    "Repair exceeded supported headroom",
                ));
            }
            if !(-32768..=32767).contains(&wide[ch]) {
                clipped[ch] += 1;
            }
            let value = wide[ch].clamp(-32768, 32767) as i16;
            peak[ch] = peak[ch].max((value as i32).abs());
            hash.update(value.to_le_bytes());
            pcm.push(value);
        }
    }
    scene::identity_bytes(&recipe.source, root)?;
    let report = json!({"profile":"pcm-dialogue-repair-v1","recipe":recipe,"sample_rate":48000,"samples":count,"channels":channels,"layout":source.layout,"speakers":source.layout.speakers(),"duration":recipe.duration,"source_in_sample":source_in,"source_identity":recipe.source,"noise_profiles":profile_reports,"dc_removed_pcm":dc,"effects":recipe.effects,"peak_absolute":peak,"clipped_samples":clipped,"meters":audio_processing::channel_meters(&pcm,channels),"pcm_sha256":format!("{:x}",hash.finalize()),"transform_samples":N,"hop_samples":HOP,"profile_clock":"original_source","output_clock":"selected_range_zero_based","noise_adaptation":"fixed_authored_profile","processing_order":"spectral_then_mean_dc_then_round_then_effects_then_final_saturation"});
    Ok(Prepared {
        pcm,
        layout: source.layout,
        report,
    })
}
pub fn inspect(recipe: &Recipe, root: &Path) -> Result<Value> {
    Ok(prepare(recipe, root)?.report)
}
fn publish(recipe: &Recipe, prepared: Prepared, root: &Path, output: &Path) -> Result<Value> {
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let temp = scratch.0.join("output.wav");
    pcm_wave::write(&temp, prepared.layout, &prepared.pcm)?;
    let actual = pcm_wave::decode(&fs::read(&temp)?)?;
    if actual.rate != 48000 || actual.layout != prepared.layout || actual.data != prepared.pcm {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Repaired WAV format or samples differ",
        ));
    }
    scene::identity_bytes(&recipe.source, root)?;
    let mut report = prepared.report;
    report["output"] = json!(output);
    report["sha256"] = json!(media::file_hash(&temp)?);
    report["recipe_sha256"] = json!(format!("{:x}", Sha256::digest(serde_json::to_vec(recipe)?)));
    media::publish(&temp, output)?;
    Ok(report)
}
pub fn run(recipe: &Recipe, root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let output = render::destination_extension(output, output_root, "wav")?;
    let prepared = prepare(recipe, root)?;
    publish(recipe, prepared, root, &output)
}
pub fn capabilities() -> Value {
    json!({"profile":"pcm-dialogue-repair-v1","sample_rate":48000,"layouts":["mono","stereo"],"maximum_seconds":60,"maximum_source_bytes":64*1024*1024,"maximum_noise_regions":8,"maximum_profile_samples":480000,"minimum_region_samples":N,"transform_samples":N,"hop_samples":HOP,"strength_milli_range":[0,3000],"floor_milli_range":[0,1000],"remove_dc":true,"post_effects":true,"automatic_speech_detection":false,"noise_profile_adaptation":false,"timing":"unchanged_sample_count"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_source_before_publication_preserves_files() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("cutbolt-repair-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let input = root.join("source.wav");
        pcm_wave::write(&input, Layout::Mono, &[100, 200, -200, -100]).unwrap();
        let original = fs::read(&input).unwrap();
        let recipe: Recipe = serde_json::from_value(json!({"schema_version":1,"id":"publication-fixture","source":{"path":"source.wav","bytes":original.len(),"sha256":media::file_hash(&input).unwrap()},"source_in":{"num":0,"den":1},"duration":{"num":1,"den":12000},"remove_dc":true})).unwrap();
        let prepared = prepare(&recipe, &root).unwrap();
        fs::write(&input, b"changed original fixture").unwrap();
        let destination = root.join("result.wav");
        assert_eq!(
            publish(&recipe, prepared, &root, &destination)
                .unwrap_err()
                .code,
            "MEDIA_CHANGED"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::write(&input, &original).unwrap();
        fs::write(&destination, b"existing output").unwrap();
        assert_eq!(
            run(&recipe, &root, &root, &destination).unwrap_err().code,
            "OUTPUT_EXISTS"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"existing output");
        assert_eq!(fs::read(&input).unwrap(), original);
        fs::remove_file(input).unwrap();
        fs::remove_file(destination).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
