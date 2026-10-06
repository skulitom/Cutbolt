//! H.264 delivery straight from the timeline, without a lossless intermediate.
//!
//! The reference graph, or the encoder that the engine compositor feeds, converts and encodes the
//! picture and mix itself. It also returns exactly what it encoded: the packed RGB24 frames on
//! stdout, which the engine counts and hashes as they arrive, and the PCM samples in a scratch
//! file. The encoder's input therefore keeps an exact frame and sample count and a digest, equal
//! to a reference export's decoded digests of the same range, while nothing is written to disk,
//! checked and decoded again before encoding.
use super::*;
use crate::digest;
use std::time::Instant;

/// What the encoder received: SHA-256 of its packed RGB24 frames and of its stereo PCM s16le.
pub(super) struct Timeline {
    pub video: String,
    pub audio: Option<String>,
}

/// Encode H.264 video exports into `encoded`, or return None when the export keeps the lossless
/// intermediate: other profiles, audio-only delivery, and plans of joined chunks.
pub(super) fn encode(
    request: &Export,
    c: &Checked,
    scratch: &Path,
    encoded: &Path,
    control: &dyn media::Control,
) -> Result<Option<Timeline>> {
    if request.profile != Profile::H264Aac || !request.streams.video() {
        return Ok(None);
    }
    let plan = &c.reference;
    let Some((inputs, graph, picture)) = layout(plan) else {
        return Ok(None);
    };
    let pixels = u64::from(c.project.width) * u64::from(c.project.height);
    let frame = pixels * 3;
    // The allowances of the render and the encode it replaces: at least 2 Mpx per second beyond
    // the fixed base, and 20 Mpx per second more for engine composition.
    let mut timeout =
        Duration::from_secs(600) + Duration::from_millis(plan.frames * pixels / 2_000);
    if plan.compositor.is_some() {
        timeout += Duration::from_millis(plan.frames * pixels / 20_000);
    }
    let pcm = scratch.join("timeline.pcm");
    // Gain streams and limited mixes the graph reads, as a render writes them.
    let _generated = render::write_generated(&plan.generated, control, timeout)?;
    let passes: &[Option<u8>] = if request.h264.unwrap_or_default().rate_control.two_pass() {
        &[Some(1), Some(2)]
    } else {
        &[None]
    };
    let mut video: Option<String> = None;
    for &pass in passes {
        control.phase(if pass == Some(1) {
            "encoding pass 1"
        } else {
            "encoding"
        })?;
        control.total(plan.frames);
        let args = arguments(
            request,
            c,
            Outputs {
                inputs: &inputs,
                graph: &graph,
                picture,
                pass,
                scratch,
                encoded,
                pcm: &pcm,
            },
        );
        let digest = run(plan, &args, plan.frames * frame, frame, timeout, control)?;
        if video.as_ref().is_some_and(|first| *first != digest) {
            return Err(verification(
                "The two encoding passes received different timeline frames",
            ));
        }
        video = Some(digest);
    }
    let audio = if request.streams.audio() {
        let bytes = plan.samples * 4;
        if fs::metadata(&pcm)?.len() != bytes {
            return Err(verification(
                "The encoder received a different number of timeline samples",
            ));
        }
        Some(prefix_hash(&pcm, bytes)?)
    } else {
        None
    };
    Ok(Some(Timeline {
        video: video.expect("at least one pass"),
        audio,
    }))
}

/// The plan's options and inputs, its graph, and the picture the encoder reads: `[vout]`, or
/// input 0 when the engine compositor writes it to stdin. None for joined chunks or a layout
/// this module does not know.
fn layout(plan: &render::Plan) -> Option<(Vec<String>, String, &'static str)> {
    if !plan.chunks.is_empty() {
        return None;
    }
    let args = &plan.arguments;
    let at = args.iter().position(|a| a == "-filter_complex")?;
    let maps: Vec<&str> = args
        .get(at + 2..at + 6)?
        .iter()
        .map(String::as_str)
        .collect();
    let picture = match maps[..] {
        ["-map", "[vout]", "-map", "[aout]"] if plan.compositor.is_none() => "[vout]",
        ["-map", "0:v:0", "-map", "[aout]"] if plan.compositor.is_some() => "[0:v:0]",
        _ => return None,
    };
    Some((args[..at].to_vec(), args[at + 1].clone(), picture))
}

struct Outputs<'a> {
    inputs: &'a [String],
    graph: &'a str,
    picture: &'a str,
    pass: Option<u8>,
    scratch: &'a Path,
    encoded: &'a Path,
    pcm: &'a Path,
}

/// One FFmpeg run: the reference graph extended to split the picture after packing it as RGB24
/// and the mix, then three outputs. The delivery comes first, so x264 is output stream 0 in both
/// passes and finds its pass-1 statistics by that index; the frames then go to stdout and the
/// samples to `pcm`, all at the timeline's exact clock.
fn arguments(request: &Export, c: &Checked, o: Outputs) -> Vec<String> {
    let rate = c.project.frame_rate;
    let audio = request.streams.audio() && o.pass != Some(1);
    let conversion = request.transfer().expect("validated transfer").conversion();
    let mut graph = format!(
        "{};{}format=rgb24,split=2[export_rgb][export_yuv];[export_yuv]{conversion}[export_video]",
        o.graph, o.picture
    );
    graph.push_str(if audio {
        ";[aout]asplit=2[export_pcm][export_audio]"
    } else {
        ";[aout]anullsink"
    });
    let clock = [
        "-r".to_owned(),
        format!("{}/{}", rate.num, rate.den),
        "-fps_mode".into(),
        "cfr".into(),
        "-enc_time_base:v".into(),
        format!("{}/{}", rate.den, rate.num),
    ];
    let mut args = vec!["-xerror".to_owned()];
    args.extend_from_slice(o.inputs);
    args.extend([
        "-filter_complex".into(),
        graph,
        "-map".into(),
        "[export_video]".into(),
    ]);
    if audio {
        args.extend(["-map", "[export_audio]"].map(str::to_owned));
    }
    args.extend(["-map_metadata", "-1", "-map_chapters", "-1"].map(str::to_owned));
    args.extend(clock.clone());
    args.extend(h264_options(request, o.pass, o.scratch, o.encoded));
    args.extend(["-map", "[export_rgb]"].map(str::to_owned));
    args.extend(clock);
    args.extend(["-c:v", "rawvideo", "-f", "rawvideo", "pipe:1"].map(str::to_owned));
    if audio {
        args.extend(
            ["-map", "[export_pcm]", "-c:a", "pcm_s16le", "-f", "s16le"].map(str::to_owned),
        );
        args.push(o.pcm.to_string_lossy().into_owned());
    }
    args
}

/// Run the encoder and return the SHA-256 of exactly `bytes` bytes of frames from its stdout;
/// fewer or more fail. With engine composition, the compositor feeds its stdin.
fn run(
    plan: &render::Plan,
    args: &[String],
    bytes: u64,
    frame: u64,
    timeout: Duration,
    control: &dyn media::Control,
) -> Result<String> {
    let program = control.tool("ffmpeg");
    if let Some(compositor) = &plan.compositor {
        // The compositor reports progress as it writes frames.
        return crate::track_composite::encode_consuming(
            compositor,
            args,
            timeout,
            0,
            control,
            move |stdout| returned(stdout, bytes),
        );
    }
    let mut encoder = media::StreamReader::spawn_with(
        control.command(&program, args)?,
        &program,
        timeout,
        media::Watch::default(),
    )?;
    let mut reported = Instant::now();
    let digest = digest::exact_sha256(
        bytes,
        |chunk| encoder.read_exact(chunk),
        |done| {
            if reported.elapsed() >= Duration::from_millis(250) {
                reported = Instant::now();
                control.frames(done / frame)
            } else {
                control.check()
            }
        },
    )?;
    encoder.finish()?;
    control.frames(bytes / frame)?;
    Ok(digest)
}

/// The digest of exactly `bytes` bytes of the encoder's returned frames, read to the end.
fn returned(stdout: std::process::ChildStdout, bytes: u64) -> Result<String> {
    let mut stdout = std::io::BufReader::with_capacity(4 * 1024 * 1024, stdout);
    let digest = digest::exact_sha256(
        bytes,
        |chunk| {
            stdout.read_exact(chunk).map_err(|_| {
                verification("The encoder returned fewer timeline frames than the range holds")
            })
        },
        |_| Ok(()),
    );
    // Read to the end whatever happened, so the encoder never blocks on a full pipe.
    let extra = std::io::copy(&mut stdout, &mut std::io::sink()).unwrap_or(0);
    let digest = digest?;
    if extra != 0 {
        return Err(verification(
            "The encoder returned more timeline frames than the range holds",
        ));
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(arguments: &[&str]) -> render::Plan {
        render::Plan {
            profile: "reference-tracks-ffv1-pcm-v1",
            source_quality: "original",
            project_revision: 3,
            frames: 50,
            samples: 96_000,
            output: PathBuf::from("plan.mkv"),
            sources: Vec::new(),
            arguments: arguments.iter().map(|a| a.to_string()).collect(),
            chunks: Vec::new(),
            generated: Vec::new(),
            compositor: None,
        }
    }
    const GRAPH: [&str; 16] = [
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-i",
        "a.mkv",
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        "[0:v:0]null[vout];[0:a:0]anull[aout]",
        "-map",
        "[vout]",
        "-map",
        "[aout]",
        "-c:v",
    ];

    fn checked(rate: Time) -> Checked {
        let project = Project {
            schema_version: 1,
            id: "p".into(),
            revision: 3,
            width: 64,
            height: 36,
            frame_rate: rate,
            assets: Vec::new(),
            clips: Vec::new(),
            tracks: None,
            sequences: Vec::new(),
            preview_scale: None,
            transfer: Some(Transfer::Bt709),
        };
        Checked {
            range: Range {
                start: Time::ZERO,
                duration: Time::new(2, 1).unwrap(),
            },
            output: PathBuf::from("out.mp4"),
            reference: plan(&GRAPH),
            project,
        }
    }
    fn request(c: &Checked, streams: Streams, h264: Option<H264>) -> Export {
        Export {
            project: c.project.clone(),
            input_root: PathBuf::from("in"),
            output_root: PathBuf::from("out"),
            output: c.output.clone(),
            profile: Profile::H264Aac,
            streams,
            range: None,
            input_transfer: None,
            h264,
            aac_bitrate: None,
            sequence_first: None,
        }
    }
    fn outputs(c: &Checked, request: &Export, pass: Option<u8>) -> Vec<String> {
        let (inputs, graph, picture) = layout(&c.reference).unwrap();
        arguments(
            request,
            c,
            Outputs {
                inputs: &inputs,
                graph: &graph,
                picture,
                pass,
                scratch: Path::new("scratch"),
                encoded: Path::new("scratch/encoded.mp4"),
                pcm: Path::new("scratch/timeline.pcm"),
            },
        )
    }
    fn position(args: &[String], value: &str) -> usize {
        args.iter().position(|a| a == value).unwrap()
    }

    #[test]
    fn layout_takes_the_graph_and_its_picture_or_keeps_the_intermediate() {
        let (inputs, graph, picture) = layout(&plan(&GRAPH)).unwrap();
        assert_eq!(inputs, GRAPH[..9]);
        assert_eq!(graph, GRAPH[10]);
        assert_eq!(picture, "[vout]");
        // The engine compositor feeds the encoder's input 0.
        let mut composited = plan(&GRAPH);
        composited.arguments[12] = "0:v:0".into();
        assert!(layout(&composited).is_none());
        composited.compositor = Some(crate::track_composite::Compositor {
            width: 64,
            height: 36,
            frames: 50,
            base: Vec::new(),
            runs: Vec::new(),
            transitions: Vec::new(),
            layers: Vec::new(),
        });
        assert_eq!(layout(&composited).unwrap().2, "[0:v:0]");
        // Joined chunks and unknown layouts keep the lossless intermediate.
        let mut chunked = plan(&GRAPH);
        chunked.chunks.push(plan(&GRAPH));
        assert!(layout(&chunked).is_none());
        assert!(layout(&plan(&GRAPH[..12])).is_none());
        let mut other = plan(&GRAPH);
        other.arguments[14] = "[other]".into();
        assert!(layout(&other).is_none());
    }

    #[test]
    fn one_run_delivers_first_and_returns_the_encoder_input() {
        let c = checked(Time::new(30000, 1001).unwrap());
        let args = outputs(&c, &request(&c, Streams::AudioVideo, None), None);
        assert_eq!(args[..2], ["-xerror", "-hide_banner"]);
        let at = position(&args, "-filter_complex");
        let graph = &args[at + 1];
        assert_eq!(
            graph,
            &format!(
                "{};[vout]format=rgb24,split=2[export_rgb][export_yuv];[export_yuv]{}[export_video];[aout]asplit=2[export_pcm][export_audio]",
                GRAPH[10],
                Transfer::Bt709.conversion()
            )
        );
        // The delivery is output 0 (x264 finds its pass statistics by stream index), then the
        // RGB24 frames on stdout at the exact clock, then the samples.
        assert_eq!(
            args[at + 2..at + 6],
            ["-map", "[export_video]", "-map", "[export_audio]"]
        );
        let delivery = position(&args, "scratch/encoded.mp4");
        let frames = position(&args, "[export_rgb]");
        assert_eq!(
            args[frames + 1..frames + 12],
            [
                "-r",
                "30000/1001",
                "-fps_mode",
                "cfr",
                "-enc_time_base:v",
                "1001/30000",
                "-c:v",
                "rawvideo",
                "-f",
                "rawvideo",
                "pipe:1"
            ]
        );
        let samples = position(&args, "[export_pcm]");
        assert_eq!(
            args[samples + 1..samples + 6],
            ["-c:a", "pcm_s16le", "-f", "s16le", "scratch/timeline.pcm"]
        );
        assert!(delivery < frames && frames < samples);
        assert_eq!(args.last().unwrap(), "scratch/timeline.pcm");
        let threads = args.iter().rposition(|a| a == "-threads").unwrap();
        assert!(threads < delivery);
        // Slice threads keep the output repeatable under the VBV cap.
        assert_eq!(
            args[threads + 1..threads + 4],
            [
                ENCODER_THREADS.to_string().as_str(),
                "-thread_type",
                "slice"
            ]
        );
        assert!(
            args.iter()
                .any(|a| a.contains(&format!(":threads={ENCODER_THREADS},")))
        );
    }

    #[test]
    fn video_only_and_first_passes_return_frames_but_no_samples() {
        let c = checked(Time::new(25, 1).unwrap());
        let video = outputs(&c, &request(&c, Streams::Video, None), None);
        assert!(video[position(&video, "-filter_complex") + 1].ends_with(";[aout]anullsink"));
        assert!(
            !video
                .iter()
                .any(|a| a == "[export_pcm]" || a == "[export_audio]")
        );
        assert!(position(&video, "scratch/encoded.mp4") < position(&video, "[export_rgb]"));
        assert_eq!(video.last().unwrap(), "pipe:1");
        let two_pass = H264 {
            rate_control: RateControl::TwoPass {
                bitrate: 1_000_000,
                maximum_bitrate: 2_000_000,
                buffer_size: 4_000_000,
            },
            ..H264::default()
        };
        let both = request(&c, Streams::AudioVideo, Some(two_pass));
        let first = outputs(&c, &both, Some(1));
        let at = position(&first, "-filter_complex");
        assert!(first[at + 1].ends_with(";[aout]anullsink"));
        assert!(!first.iter().any(|a| a == "[export_pcm]" || a == "-c:a"));
        assert_eq!(first[position(&first, "-pass") + 1], "1");
        let null = position(&first, "null");
        assert_eq!(first[null - 1..null + 2], ["-f", "null", "-"]);
        assert!(null < position(&first, "[export_rgb]"));
        assert_eq!(first.last().unwrap(), "pipe:1");
        // x264 is the first output stream in both passes.
        let second = outputs(&c, &both, Some(2));
        assert_eq!(first[at + 2..at + 4], ["-map", "[export_video]"]);
        assert_eq!(second[at + 2..at + 4], ["-map", "[export_video]"]);
        assert!(second.iter().any(|a| a == "[export_pcm]"));
        assert_eq!(second[position(&second, "-pass") + 1], "2");
    }
}
