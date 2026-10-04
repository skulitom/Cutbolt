//! Original sample-aligned PCM editing, gain envelopes and deterministic track summation.
use crate::{
    Result,
    animation::Curve,
    error, media, render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufWriter, Cursor},
    path::{Path, PathBuf},
};

const RATE: Time = Time { num: 48000, den: 1 };
fn unity() -> u32 {
    1000
}
fn zero() -> Time {
    Time::ZERO
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_AUDIO", message)
}
pub(crate) fn id_ok(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 128
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channels {
    DuplicateMono,
    PreserveStereo,
    PreserveMono,
    PreserveLayout,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub id: String,
    pub file: Identity,
    pub channels: Channels,
    pub start: Time,
    pub source_in: Time,
    pub duration: Time,
    #[serde(default = "unity")]
    pub gain_milli: u32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default = "zero")]
    pub fade_in: Time,
    #[serde(default = "zero")]
    pub fade_out: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_curve: Option<Curve>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: String,
    #[serde(default = "unity")]
    pub gain_milli: u32,
    #[serde(default)]
    pub mute: bool,
    pub clips: Vec<Clip>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mix {
    pub schema_version: u32,
    pub id: String,
    pub duration: Time,
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<crate::audio_processing::Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<crate::audio_routing::Routing>,
}

pub(crate) struct Source {
    pub data: Vec<i16>,
    pub rate: u32,
    pub channels: u16,
}
impl Source {
    pub fn frames(&self) -> u64 {
        self.data.len() as u64 / self.channels as u64
    }
    pub fn sample(&self, source_in: u64, n: u64, ch: usize) -> i64 {
        let q = n * self.rate as u64;
        let a = source_in + q / 48000;
        let b = (a + 1).min(self.frames() - 1);
        let rem = (q % 48000) as i128;
        let channel = if self.channels == 1 { 0 } else { ch };
        let x = self.data[a as usize * self.channels as usize + channel] as i128;
        let y = self.data[b as usize * self.channels as usize + channel] as i128;
        rounded(x * (48000 - rem) + y * rem, 48000)
    }
}
pub(crate) fn rounded(n: i128, d: i128) -> i64 {
    ((n.abs() + d / 2) / d * n.signum()) as i64
}

fn decode(bytes: Vec<u8>) -> Result<Source> {
    let mut chunk = 12usize;
    let mut classic = false;
    while let Some(header) = bytes.get(chunk..chunk.saturating_add(8)) {
        let size = u32::from_le_bytes(header[4..8].try_into().expect("four-byte size")) as usize;
        if &header[..4] == b"fmt " {
            classic = size >= 16 && bytes.get(chunk + 8..chunk + 10) == Some(&[1, 0]);
            break;
        }
        chunk = chunk
            .saturating_add(8)
            .saturating_add(size)
            .saturating_add(size % 2);
    }
    if !classic {
        return Err(error(
            "UNSUPPORTED_AUDIO",
            "Only classic PCM16 WAV is supported",
        ));
    }
    let mut reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|e| error("UNSUPPORTED_AUDIO", e.to_string()))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int
        || spec.bits_per_sample != 16
        || ![24000, 44100, 48000].contains(&spec.sample_rate)
        || ![1, 2].contains(&spec.channels)
    {
        return Err(error(
            "UNSUPPORTED_AUDIO",
            "Expected PCM16 mono/stereo WAV at 24/44.1/48 kHz",
        ));
    }
    let data = reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| error("UNSUPPORTED_AUDIO", e.to_string()))?;
    if data.is_empty() || !data.len().is_multiple_of(spec.channels as usize) {
        return Err(error("UNSUPPORTED_AUDIO", "Empty or incomplete PCM frames"));
    }
    Ok(Source {
        data,
        rate: spec.sample_rate,
        channels: spec.channels,
    })
}

pub(crate) struct Voice<'a> {
    clip: &'a Clip,
    track: &'a Track,
    pub source_in: u64,
    pub start: u64,
    pub samples: u64,
    fade_in: u64,
    fade_out: u64,
    curve: Option<crate::animation::Sampler>,
}
impl<'a> Voice<'a> {
    pub fn new(clip: &'a Clip, track: &'a Track, source: &Source) -> Result<Self> {
        let source_in = clip.source_in.units(Time::new(source.rate as u64, 1)?)?;
        if clip
            .source_in
            .plus(clip.duration)?
            .compare(Time::new(source.frames(), source.rate as u64)?)?
            == std::cmp::Ordering::Greater
        {
            return Err(error("INVALID_RANGE", "Audio clip exceeds source samples"));
        }
        Ok(Self {
            clip,
            track,
            source_in,
            start: clip.start.units(RATE)?,
            samples: clip.duration.units(RATE)?,
            fade_in: clip.fade_in.units(RATE)?,
            fade_out: clip.fade_out.units(RATE)?,
            curve: clip
                .gain_curve
                .as_ref()
                .map(|c| c.prepare(clip.duration, 0, 4000))
                .transpose()?,
        })
    }
    pub fn weight(&self, i: u64) -> Result<(i128, i128)> {
        let gain = self
            .curve
            .as_ref()
            .map(|c| c.sample(Time::new(i, 48000)?))
            .transpose()?
            .unwrap_or(self.clip.gain_milli as i32) as i128;
        let (fi, di) = if self.fade_in == 0 {
            (1, 1)
        } else {
            (i.min(self.fade_in) as i128, self.fade_in as i128)
        };
        let (fo, do_) = if self.fade_out == 0 {
            (1, 1)
        } else {
            (
                (self.samples - i).min(self.fade_out) as i128,
                self.fade_out as i128,
            )
        };
        let weight = if self.clip.mute || self.track.mute {
            0
        } else {
            gain * self.track.gain_milli as i128 * fi * fo
        };
        Ok((weight, 1_000_000 * di * do_))
    }
    pub fn report(&self, source: &Source) -> Value {
        json!({"track_id":self.track.id,"clip_id":self.clip.id,"start_sample":self.start,"samples":self.samples,"source_in_sample":self.source_in,"source_rate":source.rate,"source_channels":source.channels,"channel_mapping":self.clip.channels,"mute":self.clip.mute||self.track.mute,"fade_in_samples":self.fade_in,"fade_out_samples":self.fade_out,"gain_automated":self.clip.gain_curve.is_some()})
    }
}

pub(crate) struct Prepared {
    pub pcm: Vec<i16>,
    pub layout: crate::pcm_wave::Layout,
    pub sources: Vec<(PathBuf, Identity)>,
    pub report: Value,
}

pub(crate) fn validate(mix: &Mix, root: &Path) -> Result<u64> {
    crate::audio_processing::validate(&mix.effects)?;
    let count = mix.duration.units(RATE)?;
    if mix.schema_version != 1
        || !id_ok(&mix.id)
        || count == 0
        || count > 60 * 48000
        || mix.tracks.len() > 16
    {
        return Err(invalid(
            "Mix v1 requires 1 sample to 60 seconds at 48 kHz, and at most 16 tracks",
        ));
    }
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Input root must be an existing absolute directory",
        ));
    }
    let mut track_ids = HashSet::new();
    let mut clip_ids = HashSet::new();
    let mut work = 0u64;
    for track in &mix.tracks {
        if !id_ok(&track.id) || !track_ids.insert(&track.id) || track.gain_milli > 4000 {
            return Err(invalid("Unique track IDs and gain_milli 0..4000 required"));
        }
        for clip in &track.clips {
            let n = clip.duration.units(RATE)?;
            let start = clip.start.units(RATE)?;
            work = work
                .checked_add(n)
                .ok_or_else(|| invalid("Mix work overflow"))?;
            if !id_ok(&clip.id)
                || !clip_ids.insert(&clip.id)
                || clip_ids.len() > 128
                || n == 0
                || start as u128 + n as u128 > count as u128
                || clip.gain_milli > 4000
                || clip.fade_in.units(RATE)? > n
                || clip.fade_out.units(RATE)? > n
                || work > 8_000_000
            {
                return Err(invalid(
                    "Clips require unique IDs, positive in-mix ranges, bounded fades/gain, <=128 clips and <=8M summed clip samples",
                ));
            }
            clip.source_in.validate()?;
        }
    }
    Ok(count)
}

pub(crate) fn prepare(mix: &Mix, root: &Path) -> Result<Prepared> {
    if let Some(routing) = &mix.routing {
        return crate::audio_routing::prepare(mix, routing, root);
    }
    let count = validate(mix, root)?;
    let mut decoded = HashMap::<PathBuf, Source>::new();
    let mut identities = HashMap::<PathBuf, Identity>::new();
    let mut sources = Vec::new();
    let mut bytes_total = 0u64;
    let mut wide = vec![0i64; count as usize * 2];
    let mut clips = Vec::new();
    for track in &mix.tracks {
        for clip in &track.clips {
            if let Some(previous) = identities.get(&clip.file.path) {
                if previous.sha256 != clip.file.sha256 || previous.bytes != clip.file.bytes {
                    return Err(error(
                        "INVALID_IDENTITY",
                        "One audio path cannot have conflicting identities",
                    ));
                }
            } else {
                bytes_total = bytes_total
                    .checked_add(clip.file.bytes)
                    .ok_or_else(|| invalid("Source size overflow"))?;
                if bytes_total > 64 * 1024 * 1024 {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Audio source budget exceeds 64 MiB",
                    ));
                }
                let (path, bytes) = scene::identity_bytes(&clip.file, root)?;
                decoded.insert(clip.file.path.clone(), decode(bytes)?);
                identities.insert(clip.file.path.clone(), clip.file.clone());
                sources.push((path, clip.file.clone()));
            }
            let source = &decoded[&clip.file.path];
            if !matches!(
                (&clip.channels, source.channels),
                (Channels::DuplicateMono, 1) | (Channels::PreserveStereo, 2)
            ) {
                return Err(error(
                    "UNSUPPORTED_AUDIO",
                    "Channel mapping must match the decoded WAV",
                ));
            }
            let voice = Voice::new(clip, track, source)?;
            for i in 0..voice.samples {
                let (weight, denominator) = voice.weight(i)?;
                for ch in 0..2 {
                    let sample = source.sample(voice.source_in, i, ch) as i128;
                    wide[((voice.start + i) * 2) as usize + ch] +=
                        rounded(sample * weight, denominator);
                }
            }
            clips.push(voice.report(source));
        }
    }
    crate::audio_processing::process(&mut wide, &mix.effects)?;
    let mut clipped = [0u64; 2];
    let mut peak = [0i32; 2];
    let mut hash = Sha256::new();
    let pcm: Vec<i16> = wide
        .into_iter()
        .enumerate()
        .map(|(i, x)| {
            if !(-32768..=32767).contains(&x) {
                clipped[i % 2] += 1;
            }
            let x = x.clamp(-32768, 32767) as i16;
            peak[i % 2] = peak[i % 2].max((x as i32).abs());
            hash.update(x.to_le_bytes());
            x
        })
        .collect();
    let report = json!({"profile":"pcm-mix-v1","mix_id":mix.id,"sample_rate":48000,"channels":2,"samples":count,"duration":mix.duration,"effects":mix.effects,"processing":if mix.effects.is_empty(){"bypass_exact_integer"}else{"master_f64_round_nearest_ties_away"},"meters":crate::audio_processing::meters(&pcm),"clips":clips,"peak_absolute":peak,"clipped_samples":clipped,"pcm_sha256":format!("{:x}",hash.finalize()),"resampling":"linear-nearest-ties-away","gain_unit":"linear_milli","rounding":"per_voice_nearest_ties_away_then_sum","clipping":"final_saturate_i16","sources":sources.iter().map(|(path,identity)| json!({"path":path,"identity":identity})).collect::<Vec<_>>()});
    Ok(Prepared {
        pcm,
        layout: crate::pcm_wave::Layout::Stereo,
        sources,
        report,
    })
}

pub fn inspect(mix: &Mix, root: &Path) -> Result<Value> {
    Ok(prepare(mix, root)?.report)
}
pub fn run(mix: &Mix, root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let output = render::destination_extension(output, output_root, "wav")?;
    let prepared = prepare(mix, root)?;
    if mix.routing.is_some() {
        return crate::audio_routing::publish(mix, prepared, root, &output);
    }
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let temp = scratch.0.join("output.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(BufWriter::new(File::create_new(&temp)?), spec)
        .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    for sample in &prepared.pcm {
        writer
            .write_sample(*sample)
            .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    let mut reader = hound::WavReader::open(&temp)
        .map_err(|e| error("RENDER_VALIDATION_FAILED", e.to_string()))?;
    if reader.spec() != spec {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Output WAV format changed",
        ));
    }
    let mut observed = 0;
    for value in reader.samples::<i16>() {
        let value = value.map_err(|e| error("RENDER_VALIDATION_FAILED", e.to_string()))?;
        if prepared.pcm.get(observed) != Some(&value) {
            return Err(error("RENDER_VALIDATION_FAILED", "Output PCM differs"));
        }
        observed += 1;
    }
    if observed != prepared.pcm.len() {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Output PCM count differs",
        ));
    }
    drop(reader);
    for (_, identity) in &prepared.sources {
        scene::identity_bytes(identity, root)?;
    }
    let mut report = prepared.report;
    report["output"] = json!(output);
    report["sha256"] = json!(media::file_hash(&temp)?);
    report["mix_sha256"] = json!(format!("{:x}", Sha256::digest(serde_json::to_vec(mix)?)));
    media::publish(&temp, &output)?;
    Ok(report)
}

pub fn capabilities() -> Value {
    json!({"profile":"pcm-mix-v1","output_rate":48000,"output_channels":2,"source_rates":[24000,44100,48000],"source_channels":[1,2],"maximum_seconds":60,"maximum_tracks":16,"maximum_clips":128,"maximum_clip_sample_frames":8000000,"maximum_source_bytes":67108864,"resampling":"linear","gain_milli_range":[0,4000],"fade":"linear","gain_automation":true,"clipping":"final_saturate_i16","scene_soundtrack":true,"effects":{"scope":"master_before_clipping","maximum":8,"types":["low_pass","high_pass","peaking","compressor"],"precision":"f64","frequency_hz":[20,20000],"q":[0.1,20],"eq_gain_db":[-24,24],"threshold_db":[-60,0],"ratio":[1,20],"attack_ms":[0,1000],"release_ms":[0,5000],"makeup_db":[0,24]},"meters":{"profile":"stereo-48k-meters-v1","signal":"final_pcm16","sample_peak":true,"rms":true,"integrated_loudness":"BS.1770-5_Annex_1_stereo","true_peak":false},"routing":crate::audio_routing::capabilities(),"render_mode":"blocking CLI/library"})
}
