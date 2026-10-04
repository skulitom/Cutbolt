//! Local, explicitly selected audio capture and inspectable native-track placement.
use crate::{Result, error, media, pcm_stream, render, scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[cfg(windows)]
mod windows;
#[cfg(not(windows))]
mod windows {
    use super::*;
    pub(super) fn inputs() -> Result<Value> {
        Err(error(
            "UNSUPPORTED_PLATFORM",
            "Local recording currently requires Windows",
        ))
    }
    pub(super) fn describe(_: &Input) -> Result<Value> {
        inputs()
    }
    pub(super) fn open(_: &Input) -> Result<Box<dyn Capture>> {
        Err(error(
            "UNSUPPORTED_PLATFORM",
            "Local recording currently requires Windows",
        ))
    }
}

const RATE: Time = Time { num: 48000, den: 1 };
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Endpoint { id: String },
    Process { pid: u32 },
}
impl Input {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Endpoint { id } if id.is_empty() || id.len() > 1024 || id.contains('\0') => {
                Err(error(
                    "INVALID_CAPTURE_INPUT",
                    "A nonempty bounded endpoint ID is required",
                ))
            }
            Self::Process { pid: 0 } => Err(error(
                "INVALID_CAPTURE_INPUT",
                "A nonzero explicit process ID is required",
            )),
            _ => Ok(()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub input: Input,
    pub duration: Time,
    pub output_root: PathBuf,
    pub output: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    pub project: crate::model::Project,
    pub source: scene::Identity,
    pub input_root: PathBuf,
    pub asset_id: String,
    pub clip_id: String,
    pub track_id: String,
    pub sequence_id: Option<String>,
    pub start: Time,
    pub compensation: crate::tracks::Shift,
    pub collision: crate::tracks::Collision,
}
fn count(duration: Time) -> Result<u64> {
    let frames = duration.units(RATE)?;
    if frames == 0 || frames > pcm_stream::MAX_FRAMES {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Recording requires one sample to two hours",
        ));
    }
    Ok(frames)
}
pub fn inputs() -> Result<Value> {
    windows::inputs()
}
pub fn inspect(input: &Input, duration: Time) -> Result<Value> {
    input.validate()?;
    let frames = count(duration)?;
    let device = windows::describe(input)?;
    Ok(
        json!({"profile":"local-pcm-recording-v1","input":input,"device":device,"duration":Time::new(frames,48000)?,"samples":frames,"sample_rate":48000,"channels":2,"sample_format":"s16le","conversion":"windows_shared_mode_to_48k_stereo_pcm16","capture_started":false,"clock":"first_captured_sample","discontinuity_policy":"reject_after_first_packet","latency_compensation":"explicit_placement_shift"}),
    )
}

pub(crate) struct Packet {
    pub pcm: Vec<u8>,
    pub frames: u32,
    pub flags: u32,
    pub device_position: u64,
    pub qpc_100ns: u64,
}
pub(crate) trait Capture {
    fn next(&mut self) -> Result<Option<Packet>>;
    fn stop(&mut self) -> Result<()>;
    fn device_clock(&self) -> bool;
    fn description(&self) -> Value;
}
#[derive(Default)]
struct Timing {
    frames: u64,
    packets: u64,
    silent: u64,
    first: Option<(u64, u64)>,
    last: Option<(u64, u64)>,
    first_discontinuity: bool,
    max_qpc_deviation: u64,
    last_frames: u32,
}
impl Timing {
    fn accept(&mut self, packet: &Packet, device_clock: bool) -> Result<()> {
        if packet.frames == 0
            || packet.frames > 48_000
            || packet.pcm.len() != packet.frames as usize * 4
            || packet.flags & !7 != 0
        {
            return Err(error(
                "INVALID_CAPTURE_PACKET",
                "Invalid packet dimensions or flags",
            ));
        }
        if packet.flags & 4 != 0 {
            return Err(error(
                "CAPTURE_TIMESTAMP_ERROR",
                "Device marked a capture timestamp unreliable",
            ));
        }
        if packet.flags & 1 != 0 && self.packets > 0 {
            return Err(error(
                "CAPTURE_DISCONTINUITY",
                "Audio capture reported lost/discontinuous samples",
            ));
        }
        if let Some((first_position, first_qpc)) = self.first {
            if device_clock
                && first_position.checked_add(self.frames) != Some(packet.device_position)
            {
                return Err(error(
                    "CAPTURE_DISCONTINUITY",
                    "Capture device sample position is not continuous",
                ));
            }
            if packet.qpc_100ns <= self.last.expect("first packet exists").1 {
                return Err(error(
                    "CAPTURE_TIMESTAMP_ERROR",
                    "Capture clock did not advance",
                ));
            }
            let interval = packet.qpc_100ns - self.last.expect("first packet exists").1;
            let expected_interval = self.last_frames as u64 * 10_000_000 / 48000;
            if interval.abs_diff(expected_interval) > 10000 {
                return Err(error(
                    "CAPTURE_DISCONTINUITY",
                    format!(
                        "Packet timestamp gap differs from its samples by more than one millisecond: observed {interval}, expected {expected_interval}, deviation {} (100 ns units), received {} sample frames, packet {}",
                        interval.abs_diff(expected_interval),
                        self.frames,
                        self.packets
                    ),
                ));
            }
            let expected = self.frames as u128 * 10_000_000 / 48000;
            let elapsed = packet
                .qpc_100ns
                .checked_sub(first_qpc)
                .ok_or_else(|| error("CAPTURE_TIMESTAMP_ERROR", "Capture clock moved backward"))?;
            self.max_qpc_deviation = self
                .max_qpc_deviation
                .max((elapsed as u128).abs_diff(expected).min(u64::MAX as u128) as u64);
        } else {
            self.first = Some((packet.device_position, packet.qpc_100ns));
            self.first_discontinuity = packet.flags & 1 != 0;
        }
        self.last = Some((packet.device_position, packet.qpc_100ns));
        self.last_frames = packet.frames;
        self.frames += packet.frames as u64;
        self.packets += 1;
        if packet.flags & 2 != 0 {
            self.silent += packet.frames as u64;
        }
        Ok(())
    }
    fn report(&self) -> Value {
        json!({"received_sample_frames":self.frames,"packets":self.packets,"silent_frames":self.silent,"first_device_position":self.first.map(|x|x.0),"first_qpc_100ns":self.first.map(|x|x.1),"last_device_position":self.last.map(|x|x.0),"last_qpc_100ns":self.last.map(|x|x.1),"initial_discontinuity_flag":self.first_discontinuity,"maximum_qpc_sample_clock_deviation_100ns":self.max_qpc_deviation})
    }
}
fn record_packets(
    capture: &mut dyn Capture,
    frames: u64,
    temp: &Path,
    control: &dyn media::Control,
) -> Result<(String, Value)> {
    let mut writer = pcm_stream::Writer::create(temp, frames)?;
    let mut timing = Timing::default();
    let mut written = 0u64;
    let start = Instant::now();
    let mut last_packet = Instant::now();
    while written < frames {
        control.check()?;
        if start.elapsed() > Duration::from_secs(frames.div_ceil(48000) + 10)
            || last_packet.elapsed() > Duration::from_secs(5)
        {
            return Err(error(
                "CAPTURE_TIMEOUT",
                "Capture did not supply audio within its bounded deadline",
            ));
        }
        if let Some(packet) = capture.next()? {
            timing.accept(&packet, capture.device_clock())?;
            let take = (frames - written).min(packet.frames as u64) as usize;
            writer.push(&packet.pcm[..take * 4])?;
            written += take as u64;
            last_packet = Instant::now();
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    capture.stop()?;
    let digest = writer.finish_file()?;
    let mut report = timing.report();
    report["discarded_final_packet_frames"] = json!(timing.frames - frames);
    report["capture_wall_seconds"] = json!(start.elapsed().as_secs_f64());
    Ok((digest, report))
}
pub fn run(request: &Record) -> Result<Value> {
    run_controlled(request, &media::Uncontrolled)
}
pub fn run_controlled(request: &Record, control: &dyn media::Control) -> Result<Value> {
    capture_with(request, control, windows::open)
}
fn capture_with(
    request: &Record,
    control: &dyn media::Control,
    open: impl FnOnce(&Input) -> Result<Box<dyn Capture>>,
) -> Result<Value> {
    request.input.validate()?;
    let frames = count(request.duration)?;
    let output = render::destination_extension(&request.output, &request.output_root, "wav")?;
    control.check()?;
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let mut capture = open(&request.input)?;
    let temp = scratch.0.join("output.wav");
    let (digest, timing) = record_packets(capture.as_mut(), frames, &temp, control)?;
    let actual = pcm_stream::inspect(&temp, control)?;
    if actual.frames != frames || actual.pcm_sha256 != digest {
        return Err(error(
            "CAPTURE_VALIDATION_FAILED",
            "Captured WAV differs from written packets",
        ));
    }
    let receipt = json!({"profile":"local-pcm-recording-v1","input":request.input,"device":capture.description(),"output":output,"duration":Time::new(frames,48000)?,"samples":frames,"sample_rate":48000,"channels":2,"sample_format":"s16le","pcm_sha256":digest,"identity":{"path":output,"bytes":actual.bytes,"sha256":actual.sha256},"timing":timing,"conversion":"windows_shared_mode_to_48k_stereo_pcm16","clock":"first_captured_sample","latency_compensation":"none_in_file; explicit placement shift"});
    control.publish(&temp, &output, &receipt)?;
    Ok(receipt)
}
pub fn place(request: &Place) -> Result<Value> {
    request.project.validate()?;
    for id in [&request.asset_id, &request.clip_id, &request.track_id] {
        crate::tracks::id(id)?;
    }
    let path = media::allowed_file(&request.source.path, &request.input_root)?;
    let actual = pcm_stream::inspect(&path, &media::Uncontrolled)?;
    if actual.bytes != request.source.bytes || actual.sha256 != request.source.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            "Recorded source differs from its supplied identity",
        ));
    }
    request.start.units(RATE)?;
    request.compensation.amount.units(RATE)?;
    let start = if request.compensation.backward {
        request.start.minus(request.compensation.amount)?
    } else {
        request.start.plus(request.compensation.amount)?
    };
    let duration = Time::new(actual.frames, 48000)?;
    let asset = crate::model::Asset {
        id: request.asset_id.clone(),
        path: path.to_string_lossy().into_owned(),
        duration,
        metadata: Default::default(),
        identity: Some(crate::registry::Identity {
            sha256: actual.sha256,
            bytes: actual.bytes,
        }),
        proxy: None,
    };
    let arrangement = match &request.sequence_id {
        Some(id) => &crate::sequences::get(&request.project, id)?.arrangement,
        None => request.project.tracks.as_ref().ok_or_else(|| {
            error(
                "INVALID_TRACKS",
                "Recording placement requires existing native tracks",
            )
        })?,
    };
    let track = arrangement
        .tracks
        .iter()
        .find(|t| t.id == request.track_id)
        .ok_or_else(|| error("MISSING_TRACK", "Recording target track is missing"))?;
    if track.kind != crate::tracks::Kind::Audio {
        return Err(error(
            "INVALID_CAPTURE_PLACEMENT",
            "Recording placement requires an audio track",
        ));
    }
    let edit = crate::tracks::Edit::Place {
        track_id: request.track_id.clone(),
        clip: crate::tracks::TrackClip {
            id: request.clip_id.clone(),
            asset_id: request.asset_id.clone(),
            sequence_id: None,
            start,
            source_in: Time::ZERO,
            duration,
        },
        collision: request.collision,
    };
    let operation = match &request.sequence_id {
        Some(id) => crate::model::Operation::SequenceEdit {
            id: id.clone(),
            edit,
        },
        None => crate::model::Operation::Tracks { edit },
    };
    let operations = vec![crate::model::Operation::AddMedia { asset }, operation];
    let candidate = request
        .project
        .apply(request.project.revision, operations.clone())?;
    if media::file_hash(&path)? != request.source.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            "Recorded source changed during placement inspection",
        ));
    }
    Ok(
        json!({"operations":operations,"project":candidate,"uncompensated_start":request.start,"compensated_start":start,"compensation":request.compensation,"duration":duration,"source_samples":actual.frames,"pcm_sha256":actual.pcm_sha256,"writes_files":false,"updates_session":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};
    struct Fake {
        packets: VecDeque<Result<Packet>>,
    }
    impl Capture for Fake {
        fn next(&mut self) -> Result<Option<Packet>> {
            self.packets.pop_front().transpose()
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
        fn device_clock(&self) -> bool {
            true
        }
        fn description(&self) -> Value {
            json!({"original_injected_packet_fixture":true})
        }
    }
    #[test]
    fn capture_packets_publication_cancellation_and_device_faults() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("cutbolt-recording-{}-{unique}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let request = Record {
            input: Input::Endpoint {
                id: "original-test-endpoint".into(),
            },
            duration: Time::new(481, 48000).unwrap(),
            output_root: root.clone(),
            output: root.join("take.wav"),
        };
        let fake = || -> Box<dyn Capture> {
            let mut first = packet(100, 1000000, 0);
            for v in first.pcm.as_chunks_mut::<4>().0 {
                v.copy_from_slice(&[255, 127, 0, 128]);
            }
            Box::new(Fake {
                packets: VecDeque::from([Ok(first), Ok(packet(580, 1100000, 2))]),
            })
        };
        let receipt = capture_with(&request, &media::Uncontrolled, |_| Ok(fake())).unwrap();
        assert_eq!(receipt["samples"], 481);
        assert_eq!(receipt["timing"]["discarded_final_packet_frames"], 479);
        let contents = std::fs::read(&request.output).unwrap();
        let decoded = crate::pcm_wave::decode(&contents).unwrap();
        assert_eq!(decoded.data.len(), 962);
        assert_eq!(&decoded.data[958..], [32767, -32768, 0, 0]);
        assert_eq!(
            capture_with(&request, &media::Uncontrolled, |_| panic!(
                "Must reject before opening input"
            ))
            .unwrap_err()
            .code,
            "OUTPUT_EXISTS"
        );
        for (index, code) in [
            "CAPTURE_DEVICE_CHANGED",
            "CAPTURE_TIMESTAMP_ERROR",
            "CAPTURE_DISCONTINUITY",
        ]
        .iter()
        .enumerate()
        {
            let mut r = request.clone();
            r.output = root.join(format!("failure-{index}.wav"));
            let packets = VecDeque::from([
                Ok(packet(0, 1000000, 0)),
                Err(error(code, "original injected input failure")),
            ]);
            assert_eq!(
                capture_with(&r, &media::Uncontrolled, |_| Ok(Box::new(Fake { packets })))
                    .unwrap_err()
                    .code,
                *code
            );
            assert!(!r.output.exists());
        }
        struct Cancel(Cell<u32>);
        impl media::Control for Cancel {
            fn check(&self) -> Result<()> {
                let n = self.0.get();
                self.0.set(n + 1);
                if n >= 2 {
                    Err(error("CANCELLED", "original cancellation fixture"))
                } else {
                    Ok(())
                }
            }
        }
        let mut r = request.clone();
        r.output = root.join("cancel.wav");
        assert_eq!(
            capture_with(&r, &Cancel(Cell::new(0)), |_| Ok(fake()))
                .unwrap_err()
                .code,
            "CANCELLED"
        );
        assert!(!r.output.exists());
        struct Race;
        impl media::Control for Race {
            fn publish(&self, temp: &Path, output: &Path, _: &Value) -> Result<()> {
                std::fs::write(output, b"other owner")?;
                media::publish(temp, output)
            }
        }
        r.output = root.join("race.wav");
        assert_eq!(
            capture_with(&r, &Race, |_| Ok(fake())).unwrap_err().code,
            "PUBLISH_FAILED"
        );
        assert_eq!(std::fs::read(&r.output).unwrap(), b"other owner");
        assert_eq!(std::fs::read(&request.output).unwrap(), contents);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_file(&request.output).unwrap();
        std::fs::remove_file(&r.output).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
    fn packet(position: u64, qpc: u64, flags: u32) -> Packet {
        Packet {
            pcm: vec![0; 1920],
            frames: 480,
            flags,
            device_position: position,
            qpc_100ns: qpc,
        }
    }
    #[test]
    fn packet_clocks_silence_and_discontinuities() {
        let mut timing = Timing::default();
        timing.accept(&packet(123, 1_000_000, 1), true).unwrap();
        timing.accept(&packet(603, 1_100_000, 2), true).unwrap();
        assert_eq!(timing.frames, 960);
        assert_eq!(timing.silent, 480);
        assert_eq!(timing.max_qpc_deviation, 0);
        assert_eq!(
            timing
                .accept(&packet(1084, 1_200_000, 0), true)
                .unwrap_err()
                .code,
            "CAPTURE_DISCONTINUITY"
        );
        assert_eq!(
            timing
                .accept(&packet(1083, 1_200_000, 1), true)
                .unwrap_err()
                .code,
            "CAPTURE_DISCONTINUITY"
        );
        assert_eq!(
            timing
                .accept(&packet(1083, 1_200_000, 4), true)
                .unwrap_err()
                .code,
            "CAPTURE_TIMESTAMP_ERROR"
        );
        assert_eq!(
            timing
                .accept(&packet(1083, 1_100_000, 0), true)
                .unwrap_err()
                .code,
            "CAPTURE_TIMESTAMP_ERROR"
        );
        let mut process = Timing::default();
        process.accept(&packet(0, 1_000_000, 0), false).unwrap();
        process.accept(&packet(0, 1_100_000, 0), false).unwrap();
        assert_eq!(process.max_qpc_deviation, 0);
        assert_eq!(
            process
                .accept(&packet(0, 1_300_000, 0), false)
                .unwrap_err()
                .code,
            "CAPTURE_DISCONTINUITY"
        );
    }
}
