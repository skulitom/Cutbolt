//! Compile placed tracks and transitions using exact frame/sample interval boundaries.
use crate::{
    Result,
    dynamics::{self, Limiter},
    error, media,
    model::Project,
    render::{self, Plan, Source, clock},
    time::Time,
    track_composite::{self, Compositor},
    tracks::{Kind, Track, TrackClip, Transition, TransitionKind},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
const FPS: Time = Time { num: 25, den: 1 };
/// The filter time base of a native frame rate: one tick per frame.
fn timebase(rate: Time) -> String {
    format!("{}/{}", rate.den, rate.num)
}
const SAMPLES: Time = Time { num: 48000, den: 1 };
/// A premix group: its limited track's ID and limiter (none for the unlimited tracks) and the
/// indices of the tracks it sums.
type Group = (Option<(String, Limiter)>, Vec<usize>);
/// Graphs made by this process, to keep generated file names apart.
static GRAPHS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct Graph<'a> {
    project: &'a Project,
    root: &'a Path,
    control: &'a dyn media::Control,
    /// Inspected sources by asset ID.
    sources: BTreeMap<String, Source>,
    indexed: BTreeMap<String, std::sync::Arc<crate::direct::Video>>,
    converted: BTreeMap<String, Source>,
    effect_resources: crate::clip_effects::Resources,
    effect_sources: BTreeMap<PathBuf, Source>,
    direct_audio_bytes: u64,
    /// This graph's FFmpeg input index of each asset it reads.
    indices: BTreeMap<String, usize>,
    args: Vec<String>,
    filters: Vec<String>,
    serial: usize,
    inspected: usize,
    /// Assets inspected as straight-alpha overlay sources (bgra allowed).
    overlay_assets: BTreeSet<String>,
    /// Overlay assets whose decoded frames carry an alpha plane.
    alpha_assets: BTreeSet<String>,
    /// Audio-only graphs never decode pictures, so sources are timed from packets.
    audio_only: bool,
    /// Assets inspected for their audio alone, timed from packets; a picture use inspects again.
    packet_timed: BTreeSet<String>,
    /// Inspections made in parallel before compiling (see `prefetch`), taken by `inspect`.
    prefetched: Vec<(PathBuf, render::Check, render::Inspected)>,
    /// Inputs so far, media and generated gain streams alike.
    inputs: usize,
    /// Direct audio windows, gain streams and limited mixes written when the graph runs.
    generated: Vec<render::Generated>,
    /// Distinguishes this graph's generated file names.
    nonce: u128,
}
impl<'a> Graph<'a> {
    fn new(project: &'a Project, root: &'a Path, control: &'a dyn media::Control) -> Self {
        Self {
            project,
            root,
            control,
            sources: BTreeMap::new(),
            indexed: BTreeMap::new(),
            converted: BTreeMap::new(),
            effect_resources: Default::default(),
            effect_sources: BTreeMap::new(),
            direct_audio_bytes: 0,
            indices: BTreeMap::new(),
            args: ["-hide_banner", "-v", "error", "-nostdin", "-n"]
                .map(str::to_owned)
                .to_vec(),
            filters: vec![],
            serial: 0,
            inspected: 0,
            overlay_assets: BTreeSet::new(),
            alpha_assets: BTreeSet::new(),
            audio_only: false,
            packet_timed: BTreeSet::new(),
            prefetched: Vec::new(),
            inputs: 0,
            generated: Vec::new(),
            // Graphs made in the same clock tick (a premix beside its render) still differ.
            nonce: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
                ^ (u128::from(GRAPHS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) << 96),
        }
    }
    fn input(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        kind: Kind,
    ) -> Result<(usize, u64, u64)> {
        let source = self.inspect(clip, kind, false)?;
        let (first, end) = self.span(clip, at, duration, kind, &source)?;
        if self.indexed.contains_key(&clip.asset_id) {
            return Err(error(
                "UNSUPPORTED_TIMELINE",
                format!(
                    "Clip {:?}: direct {} must use the engine stream compositor; this arrangement is not supported yet",
                    clip.id,
                    if kind == Kind::Audio {
                        "audio"
                    } else {
                        "pictures"
                    }
                ),
            ));
        }
        if !self.indices.contains_key(&clip.asset_id) {
            self.args.extend([
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-i".into(),
                source.path.to_string_lossy().into_owned(),
            ]);
            self.indices.insert(clip.asset_id.clone(), self.inputs);
            self.inputs += 1;
        }
        Ok((self.indices[&clip.asset_id], first, end))
    }
    /// Inspect and verify a clip's asset once, as an alpha overlay or an opaque source.
    fn inspect(&mut self, clip: &TrackClip, kind: Kind, overlay: bool) -> Result<Source> {
        self.control.check()?;
        if self.overlay_assets.contains(&clip.asset_id) != overlay
            && self.sources.contains_key(&clip.asset_id)
        {
            return Err(error(
                "UNSUPPORTED_TIMELINE",
                format!(
                    "Asset {:?} is used both as an alpha overlay and as an opaque source",
                    clip.asset_id
                ),
            ));
        }
        let pictures = kind == Kind::Video && self.packet_timed.contains(&clip.asset_id);
        if !self.sources.contains_key(&clip.asset_id) || pictures {
            let asset = self
                .project
                .assets
                .iter()
                .find(|a| a.id == clip.asset_id)
                .expect("validated asset");
            let path = if let Some(source) = self.converted.get(&asset.id) {
                source.path.clone()
            } else {
                media::project_file(Path::new(&asset.path), self.root)?
            };
            let mut source = if asset.direct() {
                let (source, video) = render::inspect_indexed(
                    self.project,
                    asset,
                    self.root,
                    self.control,
                    render::IdentityCheck::Full,
                )?;
                self.indexed.insert(asset.id.clone(), video);
                if overlay {
                    self.overlay_assets.insert(asset.id.clone());
                }
                source
            } else if let Some(check) = self.inspection(&path, kind, overlay) {
                let (source, pix_fmt) = match self.take_prefetched(&path, check) {
                    Some(result) => result?,
                    None => render::inspect_checked(&path, check, self.control)?,
                };
                if overlay {
                    self.overlay_assets.insert(clip.asset_id.clone());
                    if pix_fmt == "bgra" {
                        self.alpha_assets.insert(clip.asset_id.clone());
                    }
                }
                if check.timing == render::Timing::Packets && !self.audio_only {
                    self.packet_timed.insert(clip.asset_id.clone());
                } else {
                    self.packet_timed.remove(&clip.asset_id);
                }
                source
            } else {
                let wave = crate::pcm_stream::inspect(&path, self.control)?;
                Source {
                    conversion: None,
                    path: path.clone(),
                    sha256: wave.sha256,
                    frames: 0,
                    samples: wave.frames,
                }
            };
            if let Some(prepared) = self.converted.get(&asset.id) {
                source.conversion = prepared.conversion.clone();
            }
            crate::registry::verify_source(asset, &source)?;
            self.sources.insert(asset.id.clone(), source);
        }
        Ok(self.sources[&clip.asset_id].clone())
    }
    /// How `inspect` checks an asset file that feeds `kind`, or None for a WAV, which the PCM
    /// reader inspects. Pictures are inspected strictly, with alpha on overlay tracks. A source
    /// read only for its audio is timed from packets, as in audio-only graphs: its pictures are
    /// never decoded, and its samples come from the same packets either way.
    fn inspection(&self, path: &Path, kind: Kind, overlay: bool) -> Option<render::Check> {
        if kind == Kind::Audio && crate::model::audio_only_path(path) {
            return None;
        }
        Some(render::Check {
            width: self.project.width,
            height: self.project.height,
            rate: self.project.frame_rate,
            alpha: overlay,
            timing: if overlay || (kind == Kind::Video && !self.audio_only) {
                render::Timing::Decoded
            } else {
                render::Timing::Packets
            },
        })
    }
    fn take_prefetched(&mut self, path: &Path, check: render::Check) -> Option<render::Inspected> {
        let at = (self.prefetched.iter()).position(|(p, c, _)| p == path && *c == check)?;
        Some(self.prefetched.swap_remove(at).2)
    }
    /// Make the inspections that checking this window will need at once, in parallel when the
    /// control allows (`render::inspect_many`), so a cold timeline waits for about its slowest
    /// source instead of all of them in turn. `inspect` then takes each result in its usual
    /// order, so errors and receipts are unchanged. Gathering stops quietly at anything invalid,
    /// which compiling reports.
    fn prefetch(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        (video, audio): (bool, bool),
    ) {
        if self.control.shared().is_none() {
            return;
        }
        let mut wanted = Vec::new();
        let mut budget = 256;
        for (kind, read) in [(Kind::Video, video), (Kind::Audio, audio)] {
            if read {
                let _ = self.wanted(a, start, duration, kind, &mut wanted, &mut budget);
            }
        }
        let mut requests: Vec<(PathBuf, render::Check)> = Vec::new();
        for (id, kind, overlay) in &wanted {
            // Video is checked first, so an asset that also shows pictures is inspected for them.
            if *kind == Kind::Audio && wanted.iter().any(|(o, k, _)| o == id && *k == Kind::Video) {
                continue;
            }
            let Some(asset) = self.project.assets.iter().find(|a| &a.id == id) else {
                continue;
            };
            if asset.decode.is_some() {
                continue;
            }
            let Ok(path) = media::project_file(Path::new(&asset.path), self.root) else {
                continue;
            };
            if let Some(check) = self.inspection(&path, *kind, *overlay)
                && !requests.iter().any(|(p, c)| *p == path && *c == check)
            {
                requests.push((path, check));
            }
        }
        if requests.len() > 1 {
            let results = render::inspect_many(&requests, self.control);
            self.prefetched = (requests.into_iter().zip(results))
                .map(|((path, check), result)| (path, check, result))
                .collect();
        }
    }
    /// The inspections `check_arrangement` makes over a window, as (asset, kind, overlay),
    /// without making them. `budget` bounds the expanded clips as `check_clip` does.
    fn wanted(
        &self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        kind: Kind,
        out: &mut Vec<(String, Kind, bool)>,
        budget: &mut usize,
    ) -> Result<()> {
        let end = start.plus(duration)?;
        for t in a.tracks.iter().filter(|t| t.enabled && t.kind == kind) {
            for c in &t.clips {
                if c.start.compare(end)?.is_lt() && c.end()?.compare(start)?.is_gt() {
                    if t.composite.is_opaque() {
                        self.wanted_clip(c, c.start, c.duration, kind, out, budget)?;
                    } else {
                        out.push((c.asset_id.clone(), kind, true));
                    }
                }
            }
            for fx in &t.transitions {
                let (x, y) = t.interval(fx)?;
                if x.compare(end)?.is_lt() && y.compare(start)?.is_gt() {
                    let (l, r) = t.endpoints(fx)?;
                    self.wanted_clip(l, x, y.minus(x)?, kind, out, budget)?;
                    self.wanted_clip(r, x, y.minus(x)?, kind, out, budget)?;
                }
            }
        }
        Ok(())
    }
    fn wanted_clip(
        &self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        kind: Kind,
        out: &mut Vec<(String, Kind, bool)>,
        budget: &mut usize,
    ) -> Result<()> {
        *budget = budget
            .checked_sub(1)
            .ok_or_else(|| error("LIMIT_EXCEEDED", "Too many clips to inspect ahead"))?;
        if let Some(id) = &clip.sequence_id {
            let a = &crate::sequences::get(self.project, id)?.arrangement;
            let start = clip.source_in.plus(at)?.minus(clip.start)?;
            return self.wanted(a, start, duration, kind, out, budget);
        }
        out.push((clip.asset_id.clone(), kind, false));
        Ok(())
    }
    /// The source frames or samples `[first, end)` a clip plays over `[at, at + duration)`.
    fn span(
        &self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        kind: Kind,
        source: &Source,
    ) -> Result<(u64, u64)> {
        if kind == Kind::Video && source.frames == 0 {
            return Err(crate::model::no_picture(
                &clip.asset_id,
                &format!("video clip {:?}", clip.id),
            ));
        }
        let first = clip
            .source_in
            .plus(at)?
            .minus(clip.start)?
            .units(kind.clock(self.project.frame_rate))?;
        let count = duration.units(kind.clock(self.project.frame_rate))?;
        let limit = if kind == Kind::Video {
            source.frames
        } else {
            source.samples
        };
        if first as u128 + count as u128 > limit as u128 {
            return Err(error(
                "INVALID_RANGE",
                format!(
                    "Clip {} or its transition handle exceeds decoded media",
                    clip.id
                ),
            ));
        }
        Ok((first, first + count))
    }
    fn direct_picture(
        &self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        crop: [u32; 4],
    ) -> Result<Option<crate::direct::Picture>> {
        let Some(video) = self.indexed.get(&clip.asset_id) else {
            return Ok(None);
        };
        let (first, end) = self.span(
            clip,
            at,
            duration,
            Kind::Video,
            &self.sources[&clip.asset_id],
        )?;
        Ok(Some(crate::direct::Picture::new(
            video.clone(),
            first,
            end - first,
            crop,
            self.project
                .transfer
                .unwrap_or(crate::color::Transfer::Bt709),
        )))
    }
    fn clip_effects(
        &mut self,
        clip: &TrackClip,
        at: Time,
        origin: [u32; 2],
    ) -> Result<Option<crate::clip_effects::Stack>> {
        if clip.effects.is_empty() && clip.transform.is_none() {
            return Ok(None);
        }
        let pixels = clip
            .effects
            .iter()
            .filter_map(crate::tracks::ClipEffect::pixel)
            .collect::<Vec<_>>();
        for source in self.effect_resources.add(&pixels, self.root)? {
            if self
                .effect_sources
                .get(&source.path)
                .is_some_and(|old| old.sha256 != source.sha256)
            {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Clip effects and generated scenes disagree on a shared source identity",
                ));
            }
            self.effect_sources.insert(source.path.clone(), source);
        }
        let scale = self.project.effect_coordinate_scale.unwrap_or(1);
        let properties = crate::clip_properties::normalize(
            &clip.effects,
            clip.transform.as_ref(),
            [self.project.width * scale, self.project.height * scale],
            clip.source_duration(self.project)?,
        )?;
        Ok(Some(crate::clip_effects::Stack {
            clip_id: clip.id.clone(),
            effects: pixels,
            properties,
            source_start: clip.source_in.plus(at)?.minus(clip.start)?,
            source_duration: clip.source_duration(self.project)?,
            rate: self.project.frame_rate,
            origin,
            coordinate_scale: self.project.effect_coordinate_scale.unwrap_or(1),
        }))
    }
    /// Audio graph inputs and filters; native video frames arrive separately on stdin.
    fn arguments(&self) -> Vec<String> {
        let mut args = self.args.clone();
        args.extend([
            "-filter_complex_threads".into(),
            "1".into(),
            "-filter_complex".into(),
            self.filters.join(";"),
        ]);
        args
    }
    fn arguments_with(&self, tail: &[String]) -> Result<Vec<String>> {
        let mut args = self.arguments();
        args.extend_from_slice(tail);
        if too_long(&args) {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Track render arguments exceed supported size",
            ));
        }
        Ok(args)
    }
    fn audio_source(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        label: &str,
    ) -> Result<()> {
        let raw = if clip.adjusts_audio() {
            format!("{label}g")
        } else {
            label.to_string()
        };
        if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            self.compose_audio(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                &raw,
            )?;
        } else if clip.scene.is_some() {
            let scene = self.scene(clip)?;
            let first = clip.source_in.plus(at)?.minus(clip.start)?.units(SAMPLES)?;
            let samples = duration.units(SAMPLES)?;
            self.direct_audio_bytes = self
                .direct_audio_bytes
                .checked_add(samples * 4)
                .filter(|&n| n <= 1_073_741_824)
                .ok_or_else(|| {
                    error(
                        "LIMIT_EXCEEDED",
                        "Generated audio windows exceed the 1 GiB graph bound",
                    )
                })?;
            let input = self.inputs;
            self.inputs += 1;
            let path = std::env::temp_dir().join(format!(
                ".cutbolt-scene-audio-{}-{}-{input}.wav",
                std::process::id(),
                self.nonce
            ));
            self.args.extend([
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-i".into(),
                path.to_string_lossy().into_owned(),
            ]);
            self.generated.push(render::Generated::SceneAudio {
                path,
                scene,
                first,
                samples,
            });
            self.filters.push(format!("[{input}:a:0]atrim=end_sample={samples},asetpts=N/SR/TB,aformat=sample_fmts=dblp:channel_layouts=stereo[{raw}]"));
        } else {
            let source = self.inspect(clip, Kind::Audio, false)?;
            let (input, first, end) = if let Some(video) = self.indexed.get(&clip.asset_id).cloned()
            {
                let (first, end) = self.span(clip, at, duration, Kind::Audio, &source)?;
                let samples = end - first;
                let bytes = samples
                    .checked_mul(4)
                    .and_then(|n| self.direct_audio_bytes.checked_add(n));
                if bytes.is_none_or(|n| n > 1_073_741_824) {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Direct audio windows exceed 1073741824 PCM bytes in one render graph",
                    )
                    .with_path("project.tracks")
                    .with_hint("Render a shorter range or fewer overlapping audio clips."));
                }
                self.direct_audio_bytes = bytes.expect("bounded audio size");
                let input = self.inputs;
                self.inputs += 1;
                let path = std::env::temp_dir().join(format!(
                    ".cutbolt-direct-audio-{}-{}-{input}.wav",
                    std::process::id(),
                    self.nonce
                ));
                self.args.extend([
                    "-protocol_whitelist".into(),
                    "file,pipe".into(),
                    "-i".into(),
                    path.to_string_lossy().into_owned(),
                ]);
                self.generated.push(render::Generated::DirectAudio {
                    path,
                    video,
                    first,
                    samples,
                });
                (input, 0, samples)
            } else {
                self.input(clip, at, duration, Kind::Audio)?
            };
            self.filters.push(format!("[{input}:a:0]atrim=start_sample={first}:end_sample={end},asetpts=N/SR/TB,aformat=sample_fmts=dblp:channel_layouts=stereo[{raw}]"));
        }
        if clip.adjusts_audio() {
            self.level(clip, at, duration, &raw, label)?;
        }
        Ok(())
    }
    /// Clip gain and linear fades, rounded once per sample to the nearest PCM16 value with ties
    /// away from zero, as in a pcm-mix-v1 voice. Fades never overlap and last at most 60 s, so
    /// every product stays an exact integer in doubles.
    fn level(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        input: &str,
        label: &str,
    ) -> Result<()> {
        let e = clip.envelope()?;
        // Transition handles read before a clip's start and after its end; validation keeps
        // fades off those edges, so the handles play at the plain gain.
        let offset =
            e.offset as i64 + at.units(SAMPLES)? as i64 - clip.start.units(SAMPLES)? as i64;
        let (fi, fo, samples) = (e.fade_in, e.fade_out, e.samples);
        // With a gain curve, a generated stream beside the audio carries each sample's gain in
        // milli-units (exact as 16-bit PCM); otherwise the gain is the constant `gain_milli`.
        let (g, input) = match &clip.gain_curve {
            Some(curve) => {
                let index = self.inputs;
                self.inputs += 1;
                let path = std::env::temp_dir().join(format!(
                    ".cutbolt-gain-{}-{}-{index}.wav",
                    std::process::id(),
                    self.nonce
                ));
                self.args
                    .extend(["-i".into(), path.to_string_lossy().into_owned()]);
                self.generated.push(render::Generated::Gain {
                    path,
                    curve: curve.clone(),
                    start: clip.source_in.plus(at)?.minus(clip.start)?,
                    samples: duration.units(SAMPLES)?,
                });
                self.filters.push(format!(
                    "[{index}:a:0]aformat=sample_fmts=dblp:channel_layouts=mono[{label}c];[{input}][{label}c]join=inputs=2:channel_layout=3.0:map=0.0-FL|0.1-FR|1.0-FC[{label}j]"
                ));
                ("ld(5)".to_string(), format!("{label}j"))
            }
            None => (e.gain_milli.to_string(), input.to_string()),
        };
        let gain = if clip.gain_curve.is_some() {
            "st(5,val(2)*32768);"
        } else {
            ""
        };
        let (mut weight, mut divisor) = (g.clone(), "1000".to_string());
        if fo != 0 {
            let tail = samples - fo;
            weight = format!("if(gte(ld(0),{tail}),{g}*({samples}-ld(0)),{weight})");
            divisor = format!("if(gte(ld(0),{tail}),{},{divisor})", 1000 * fo);
        }
        if fi != 0 {
            weight = format!("if(lt(ld(0),{fi}),{g}*ld(0),{weight})");
            divisor = format!("if(lt(ld(0),{fi}),{},{divisor})", 1000 * fi);
        }
        let channel = |c: usize| {
            format!(
                "st(0,n{offset:+});{gain}st(1,{weight});st(2,{divisor});st(3,val({c})*32768*ld(1));if(lt(ld(3),0),-floor((-ld(3)+ld(2)/2)/ld(2)),floor((ld(3)+ld(2)/2)/ld(2)))/32768"
            )
        };
        self.filters.push(format!(
            "[{input}]aeval=exprs='{}|{}':channel_layout=stereo,aformat=sample_fmts=dblp:channel_layouts=stereo[{label}]",
            channel(0),
            channel(1)
        ));
        Ok(())
    }
    fn effect(
        &mut self,
        track: &Track,
        effect: &Transition,
        at: Time,
        duration: Time,
        label: &str,
    ) -> Result<()> {
        let (left, right) = track.endpoints(effect)?;
        let (start, end) = track.interval(effect)?;
        let units = SAMPLES;
        let count = end.minus(start)?.units(units)?;
        let offset = at.minus(start)?.units(units)?;
        let d = count.checked_mul(2).ok_or_else(|| {
            error(
                "TIME_OVERFLOW",
                "Transition sample clock exceeds supported precision",
            )
        })?;
        let l = format!("{label}l");
        let r = format!("{label}r");
        debug_assert_eq!(track.kind, Kind::Audio);
        self.audio_source(left, at, duration, &l)?;
        self.audio_source(right, at, duration, &r)?;
        let k = format!("(2*(n+{offset})+1)");
        let (a, b) = if effect.kind == TransitionKind::DipBlack {
            (format!("max({d}-2*{k},0)"), format!("max(2*{k}-{d},0)"))
        } else {
            (format!("({d}-{k})"), k)
        };
        let channel = |c| {
            format!(
                "st(0,val({c})*32768*{a}+val({})*32768*{b});if(lt(ld(0),0),-floor((-ld(0)+{count})/{d}),floor((ld(0)+{count})/{d}))/32768",
                c + 2
            )
        };
        self.filters.push(format!("[{l}][{r}]join=inputs=2:channel_layout=quad:map=0.0-FL|0.1-FR|1.0-BL|1.1-BR,aeval=exprs='{}|{}':channel_layout=stereo[{label}]",channel(0),channel(1)));
        Ok(())
    }
    fn label(&mut self) -> Result<String> {
        self.serial += 1;
        if self.serial > 2048 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Expanded sequence graph exceeds 2048 segments",
            ));
        }
        Ok(format!("n{}", self.serial))
    }
    fn scene(&mut self, clip: &TrackClip) -> Result<std::sync::Arc<crate::scene::FrameSource>> {
        let scene = clip.scene.as_ref().expect("generated clip");
        scene.validate(self.project)?;
        let prepared = self
            .effect_resources
            .scenes
            .get(scene, self.root, self.control)?;
        for source in prepared.sources() {
            if self
                .effect_sources
                .get(&source.path)
                .is_some_and(|old| old.sha256 != source.sha256)
            {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Generated scenes disagree on a shared source identity",
                ));
            }
            self.effect_sources.insert(source.path.clone(), source);
        }
        Ok(prepared)
    }
    fn check_clip(&mut self, clip: &TrackClip, at: Time, duration: Time, kind: Kind) -> Result<()> {
        self.control.check()?;
        self.inspected += 1;
        if self.inspected > 256 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Nested render exceeds 256 expanded clip/handle inspections",
            ));
        }
        if clip.scene.is_some() {
            self.scene(clip)?;
            let first = clip.source_in.plus(at)?.minus(clip.start)?;
            first.units(kind.clock(self.project.frame_rate))?;
            duration.units(kind.clock(self.project.frame_rate))?;
            if first
                .plus(duration)?
                .compare(clip.source_duration(self.project)?)?
                .is_gt()
            {
                return Err(error(
                    "INVALID_RANGE",
                    "Generated clip window exceeds its scene",
                ));
            }
        } else if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            self.check_arrangement(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                kind,
            )?;
        } else {
            let source = self.inspect(clip, kind, false)?;
            self.span(clip, at, duration, kind, &source)?;
        }
        Ok(())
    }
    fn check_arrangement(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        kind: Kind,
    ) -> Result<()> {
        let end = start.plus(duration)?;
        if end.compare(a.duration)?.is_gt() {
            return Err(error(
                "INVALID_RANGE",
                "Nested window exceeds its definition",
            ));
        }
        for t in a.tracks.iter().filter(|t| t.enabled && t.kind == kind) {
            for c in &t.clips {
                if c.start.compare(end)?.is_lt() && c.end()?.compare(start)?.is_gt() {
                    if t.composite.is_opaque() || c.scene.is_some() {
                        self.check_clip(c, c.start, c.duration, kind)?;
                    } else {
                        // Validated alpha_over tracks hold asset clips only, inspected with alpha.
                        // Their inputs are added where they are composited.
                        let source = self.inspect(c, kind, true)?;
                        self.span(c, c.start, c.duration, kind, &source)?;
                    }
                }
            }
            // Even covered effects must have valid decoded handles and identities.
            for fx in &t.transitions {
                let (x, y) = t.interval(fx)?;
                if x.compare(end)?.is_lt() && y.compare(start)?.is_gt() {
                    y.minus(x)?
                        .units(kind.clock(self.project.frame_rate))?
                        .checked_mul(2)
                        .ok_or_else(|| {
                            error(
                                "TIME_OVERFLOW",
                                "Transition sample clock exceeds supported precision",
                            )
                        })?;
                    let (l, r) = t.endpoints(fx)?;
                    self.check_clip(l, x, y.minus(x)?, kind)?;
                    self.check_clip(r, x, y.minus(x)?, kind)?;
                }
            }
        }
        Ok(())
    }
    fn compose_audio(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        output: &str,
    ) -> Result<()> {
        self.control.check()?;
        let end = start.plus(duration)?;
        start.units(SAMPLES)?;
        let count = duration.units(SAMPLES)?;
        if count == 0 || end.compare(a.duration)?.is_gt() {
            return Err(error(
                "INVALID_RANGE",
                "Composition requires a positive in-bounds child window",
            ));
        }
        let tracks: Vec<usize> = (0..a.tracks.len())
            .filter(|&i| a.tracks[i].enabled && a.tracks[i].kind == Kind::Audio)
            .collect();
        // A child is a reusable PCM16 sequence, so saturate at its own mix boundary.
        self.mix(a, &tracks, start, duration, output, true)
    }
    /// The voices of the listed audio tracks of `a` over `[start, start + duration)`, summed into
    /// `output` (silence when none plays); `saturate` rounds the sum to PCM16, as at a mix
    /// boundary, and otherwise the sum keeps its full range.
    fn mix(
        &mut self,
        a: &crate::tracks::Arrangement,
        tracks: &[usize],
        start: Time,
        duration: Time,
        output: &str,
        saturate: bool,
    ) -> Result<()> {
        let end = start.plus(duration)?;
        let begin = start.units(SAMPLES)?;
        let count = duration.units(SAMPLES)?;
        let mut labels = String::new();
        let mut voices = 0;
        for track in tracks.iter().map(|&i| &a.tracks[i]) {
            let edges: Vec<_> = boundaries(track, start, end, SAMPLES)?
                .into_iter()
                .collect();
            for part in edges.windows(2) {
                let at = Time::new(part[0], 48000)?;
                let length = Time::new(part[1] - part[0], 48000)?;
                if let Some(clip) = track.clips.iter().find(|c| {
                    c.start.compare(at).expect("validated").is_le()
                        && c.end()
                            .expect("validated")
                            .compare(at)
                            .expect("validated")
                            .is_gt()
                }) {
                    let label = self.label()?;
                    if let Some(fx) = track.transition_at(at)? {
                        self.effect(track, fx, at, length, &label)?;
                    } else {
                        self.audio_source(clip, at, length, &label)?;
                    }
                    self.filters.push(format!(
                        "[{label}]adelay=delays={}S:all=1[{label}d]",
                        part[0] - begin
                    ));
                    labels.push_str(&format!("[{label}d]"));
                    voices += 1;
                }
            }
        }
        if voices == 0 {
            self.filters.push(format!("anullsrc=r=48000:cl=stereo,atrim=end_sample={count},asetpts=N/SR/TB,aformat=sample_fmts=dblp[{output}]"));
        } else {
            let rounding = if saturate {
                ",aformat=sample_fmts=s16:sample_rates=48000:channel_layouts=stereo,aformat=sample_fmts=dblp"
            } else {
                ""
            };
            self.filters.push(format!("{labels}amix=inputs={voices}:duration=longest:dropout_transition=0:normalize=0,apad=whole_len={count},atrim=end_sample={count}{rounding}[{output}]"));
        }
        Ok(())
    }
    /// The top-level audio of a window as `aout`: the plain mix, or with track or master limiters,
    /// the mix through them.
    fn audio(&mut self, a: &crate::tracks::Arrangement, start: Time, duration: Time) -> Result<()> {
        self.check_arrangement(a, start, duration, Kind::Audio)?;
        let limited = a.master.is_some()
            || a.tracks
                .iter()
                .any(|t| t.enabled && t.kind == Kind::Audio && t.dynamics.is_some());
        if !limited {
            return self.compose_audio(a, start, duration, "aout");
        }
        let index = self.inputs;
        self.inputs += 1;
        let path = std::env::temp_dir().join(format!(
            ".cutbolt-mix-{}-{}-{index}.wav",
            std::process::id(),
            self.nonce
        ));
        let premix = self.premix(a, start, duration, path.clone(), 1)?;
        self.args
            .extend(["-i".into(), path.to_string_lossy().into_owned()]);
        self.filters.push(format!(
            "[{index}:a:0]aformat=sample_fmts=dblp:channel_layouts=stereo[aout]"
        ));
        self.generated
            .push(render::Generated::Mix(Box::new(premix)));
        Ok(())
    }
    /// The premix of a window's audio, to be written to `path`: each limited track's clips summed
    /// as one group and the other enabled audio tracks as another, unsaturated, over the window and
    /// the limiters' reach around it. Its runs split that range into `parts` equal pieces, and
    /// further where more clips overlap than one graph takes; they give the same sums whether they
    /// run one after another or side by side.
    fn premix(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        path: PathBuf,
        parts: u64,
    ) -> Result<dynamics::Premix> {
        // Each limited track is a group, and the other enabled audio tracks one more.
        let mut groups: Vec<Group> = Vec::new();
        let mut rest = Vec::new();
        for (index, track) in a.tracks.iter().enumerate() {
            if !track.enabled || track.kind != Kind::Audio {
                continue;
            }
            match &track.dynamics {
                Some(d) => groups.push((Some((track.id.clone(), d.limiter.clone())), vec![index])),
                None => rest.push(index),
            }
        }
        if !rest.is_empty() || groups.is_empty() {
            groups.push((None, rest));
        }
        let master = a.master.as_ref().map(|d| d.limiter.clone());
        // Mix enough around the window that every limited sample in it is exact.
        let (before, after) = groups
            .iter()
            .filter_map(|(g, _)| g.as_ref().map(|(_, l)| l.reach()))
            .fold((0, 0), |(b, f), (x, y)| (b.max(x), f.max(y)));
        let (master_before, master_after) = master.as_ref().map_or((0, 0), Limiter::reach);
        let window = (start.units(SAMPLES)?, start.plus(duration)?.units(SAMPLES)?);
        let first = window.0.saturating_sub(before + master_before);
        let last = (window.1 + after + master_after).min(a.duration.units(SAMPLES)?);
        let mut runs = Vec::new();
        let parts = parts.clamp(1, (last - first).max(1));
        let mut windows = Vec::new();
        for part in 0..parts {
            let piece = (last - first) * part / parts + first;
            let end = (last - first) * (part + 1) / parts + first;
            windows.extend(audio_windows(a, piece, end)?);
        }
        for (from, to) in windows {
            let mut premix = Graph::new(self.project, self.root, self.control);
            premix.audio_only = true;
            premix.sources = self.sources.clone();
            premix.effect_resources = self.effect_resources.clone();
            premix.effect_sources = self.effect_sources.clone();
            premix.converted = self.converted.clone();
            premix.indexed = self.indexed.clone();
            premix.overlay_assets = self.overlay_assets.clone();
            premix.alpha_assets = self.alpha_assets.clone();
            premix.inspected = self.inspected;
            let (at, length) = (Time::new(from, 48000)?, Time::new(to - from, 48000)?);
            let mut labels = String::new();
            for (index, (_, tracks)) in groups.iter().enumerate() {
                premix.mix(a, tracks, at, length, &format!("p{index}"), false)?;
                labels.push_str(&format!("[p{index}]"));
            }
            // Scaling by 2^-16 is exact, so signed 32-bit output holds each sum's PCM16 code
            // unsaturated, rounded as the PCM16 conversion of a plain mix would round it.
            let merge = if groups.len() == 1 {
                String::new()
            } else {
                format!("amerge=inputs={},", groups.len())
            };
            premix.filters.push(format!(
                "{labels}{merge}volume=volume=0.0000152587890625:precision=double,aformat=sample_fmts=s32[premix]"
            ));
            self.inspected = premix.inspected;
            for (path, source) in &premix.effect_sources {
                self.effect_sources.insert(path.clone(), source.clone());
            }
            for (id, source) in &premix.sources {
                self.sources
                    .entry(id.clone())
                    .or_insert_with(|| source.clone());
            }
            let (mut arguments, _, generated) = premix.finish();
            arguments.extend(
                [
                    "-map",
                    "[premix]",
                    "-c:a",
                    "pcm_s32le",
                    "-f",
                    "s32le",
                    "pipe:1",
                ]
                .map(str::to_owned),
            );
            if arguments.iter().map(|s| s.len() + 3).sum::<usize>() > 24000 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Limited audio mix arguments exceed supported size",
                ));
            }
            runs.push(dynamics::Run {
                arguments,
                generated,
                frames: to - from,
            });
        }
        Ok(dynamics::Premix {
            path,
            runs,
            groups: groups.into_iter().map(|(g, _)| g).collect(),
            master,
            first,
            window,
            report: Default::default(),
        })
    }
    fn sources(&self) -> Vec<Source> {
        let mut values: Vec<_> = self.sources.values().cloned().collect();
        values.extend(self.effect_sources.values().cloned());
        values.sort_by(|a, b| a.path.cmp(&b.path));
        values
    }
    /// Another graph of this render, to which everything already inspected and counted carries
    /// over.
    fn branch(&self) -> Graph<'a> {
        let mut next = Graph::new(self.project, self.root, self.control);
        next.sources = self.sources.clone();
        next.converted = self.converted.clone();
        next.indexed = self.indexed.clone();
        next.effect_resources = self.effect_resources.clone();
        next.effect_sources = self.effect_sources.clone();
        next.overlay_assets = self.overlay_assets.clone();
        next.alpha_assets = self.alpha_assets.clone();
        next.audio_only = self.audio_only;
        next.packet_timed = self.packet_timed.clone();
        next.serial = self.serial;
        next.inspected = self.inspected;
        next
    }
    /// A graph for the encoder of an engine-composited picture: input 0 is that picture on
    /// stdin, and everything already inspected and counted carries over.
    fn successor(&mut self) -> Graph<'a> {
        let mut next = self.branch();
        next.args.extend(track_composite::raw_input(
            self.project.width,
            self.project.height,
            self.project.frame_rate,
        ));
        next.inputs = 1;
        next.prefetched = std::mem::take(&mut self.prefetched);
        next
    }
    /// Render all visible video tracks through ordered Rust layers, including child sequences.
    fn stack_plan<'c>(
        &mut self,
        a: &'c crate::tracks::Arrangement,
        start: Time,
        duration: Time,
    ) -> Result<crate::track_stack::Plan> {
        use crate::track_stack::{Clip, Input, Layer, Mix, Plan, Segment};
        struct Window<'c> {
            clip: &'c TrackClip,
            first: u64,
            end: u64,
        }
        let rate = self.project.frame_rate;
        let window = start.units(rate)?;
        let frames = duration.units(rate)?;
        let end = start.plus(duration)?;
        let mut edges = BTreeSet::from([window, window + frames]);
        for track in a
            .tracks
            .iter()
            .filter(|t| t.enabled && t.kind == Kind::Video)
        {
            edges.extend(boundaries(track, start, end, rate)?);
        }
        let mut windows: Vec<Window<'_>> = vec![];
        let mut latest: BTreeMap<&str, usize> = BTreeMap::new();
        let mut segments = vec![];
        let edges: Vec<_> = edges.into_iter().collect();
        for pair in edges.windows(2) {
            self.control.check()?;
            let at = Time::new(pair[0] * rate.den, rate.num)?;
            let (first, end) = (pair[0] - window, pair[1] - window);
            let mut visible = vec![];
            for (index, track) in a
                .tracks
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, t)| t.enabled && t.kind == Kind::Video)
            {
                let transition = track.transition_at(at)?;
                let (left, right) = if let Some(fx) = transition {
                    let (left, right) = track.endpoints(fx)?;
                    (left, Some(right))
                } else if let Some(clip) = track.clips.iter().find(|c| {
                    c.start.compare(at).expect("validated").is_le()
                        && c.end()
                            .expect("validated")
                            .compare(at)
                            .expect("validated")
                            .is_gt()
                }) {
                    (clip, None)
                } else {
                    continue;
                };
                if right.is_none() && left.transform.as_ref().is_some_and(|t| t.opacity == 0) {
                    continue;
                }
                let opaque = |clip: &TrackClip| -> Result<bool> {
                    let scale = self.project.effect_coordinate_scale.unwrap_or(1);
                    let canvas = [self.project.width * scale, self.project.height * scale];
                    let properties = crate::clip_properties::normalize(
                        &clip.effects,
                        clip.transform.as_ref(),
                        canvas,
                        clip.source_duration(self.project)?,
                    )?;
                    Ok(!self.alpha_assets.contains(&clip.asset_id)
                        && clip.scene.as_ref().is_none_or(|s| !s.0.transparent)
                        && !clip
                            .effects
                            .iter()
                            .any(|e| matches!(e, crate::tracks::ClipEffect::ChromaKey(_)))
                        && properties.as_ref().is_none_or(|p| p.opaque(canvas)))
                };
                let covers = opaque(left)? && right.map(opaque).transpose()?.unwrap_or(true);
                visible.push((index, left, right, transition));
                if covers {
                    break;
                }
            }
            let mut layers = vec![];
            for (track, left, right, transition) in visible.into_iter().rev() {
                let mut schedule = |clip: &'c TrackClip| {
                    if let Some(&index) = latest.get(clip.id.as_str())
                        && windows[index].end == first
                    {
                        windows[index].end = end;
                        return index;
                    }
                    let index = windows.len();
                    windows.push(Window { clip, first, end });
                    latest.insert(&clip.id, index);
                    index
                };
                let outgoing = schedule(left);
                let mix = match (right, transition) {
                    (Some(right), Some(fx)) => {
                        let (begin, finish) = a.tracks[track].interval(fx)?;
                        Some(Mix {
                            incoming: schedule(right),
                            kind: fx.kind,
                            count: finish.minus(begin)?.units(rate)?,
                            offset: at.minus(begin)?.units(rate)?,
                        })
                    }
                    _ => None,
                };
                layers.push(Layer {
                    track,
                    outgoing,
                    mix,
                });
            }
            segments.push(Segment { first, end, layers });
        }
        let mut clips = Vec::with_capacity(windows.len());
        for Window { clip, first, end } in windows {
            let at = Time::new((window + first) * rate.den, rate.num)?;
            let length = Time::new((end - first) * rate.den, rate.num)?;
            let alpha = self.alpha_assets.contains(&clip.asset_id)
                || clip.scene.as_ref().is_some_and(|s| s.0.transparent);
            if alpha && clip.transform.as_ref().is_some_and(|t| t.divisor > 1) {
                return Err(error(
                    "UNSUPPORTED_MEDIA",
                    format!(
                        "Clip {:?}: only opaque overlay sources can be shrunk; straight-alpha sources may be cropped, faded and placed",
                        clip.id
                    ),
                ));
            }
            let input = if let Some(scene) = &clip.scene {
                Input::Generated {
                    scene_id: scene.0.id.clone(),
                    recipe_sha256: scene.key()?,
                    first: clip.source_in.plus(at)?.minus(clip.start)?.units(rate)?,
                    divisor: self.project.effect_coordinate_scale.unwrap_or(1),
                    source: self.scene(clip)?,
                }
            } else if let Some(sequence_id) = &clip.sequence_id {
                let arrangement = crate::sequences::get(self.project, sequence_id)?
                    .arrangement
                    .clone();
                let source_start = clip.source_in.plus(at)?.minus(clip.start)?;
                Input::Sequence {
                    sequence_id: sequence_id.clone(),
                    plan: Box::new(self.stack_plan(&arrangement, source_start, length)?),
                }
            } else {
                let direct = self.direct_picture(
                    clip,
                    at,
                    length,
                    [0, 0, self.project.width, self.project.height],
                )?;
                let arguments = if direct.is_some() {
                    vec![]
                } else {
                    let source = &self.sources[&clip.asset_id];
                    let (from, to) = self.span(clip, at, length, Kind::Video, source)?;
                    let mut arguments: Vec<String> = ["-hide_banner", "-v", "error", "-nostdin"]
                        .map(str::to_owned)
                        .to_vec();
                    arguments.extend(clock::seek_input(rate, from));
                    arguments.extend(["-protocol_whitelist", "file,pipe", "-i"].map(str::to_owned));
                    arguments.push(source.path.to_string_lossy().into_owned());
                    let trim = clock::seek_trim(rate, from, from, to);
                    let convert = if alpha {
                        "extractplanes=r+g+b+a[r][g][b][a];[r][g][b][a]mergeplanes=map0s=1:map0p=0:map1s=2:map1p=0:map2s=0:map2p=0:map3s=3:map3p=0:format=gbrap"
                    } else {
                        "format=pix_fmts=gbrp"
                    };
                    arguments.extend([
                        "-filter_complex_threads".into(),
                        "1".into(),
                        "-filter_complex".into(),
                        format!(
                            "[0:v:0]{trim},settb=expr={},setpts=N,{convert}[out]",
                            timebase(rate)
                        ),
                    ]);
                    arguments.extend(
                        [
                            "-map",
                            "[out]",
                            "-an",
                            "-fps_mode",
                            "passthrough",
                            "-f",
                            "rawvideo",
                            "-pix_fmt",
                            if alpha { "gbrap" } else { "gbrp" },
                            "pipe:1",
                        ]
                        .map(str::to_owned),
                    );
                    if too_long(&arguments) {
                        return Err(error(
                            "LIMIT_EXCEEDED",
                            "Native layer decoder arguments exceed supported size",
                        ));
                    }
                    arguments
                };
                Input::Media { arguments, direct }
            };
            clips.push(Clip {
                clip_id: clip.id.clone(),
                first,
                end,
                alpha,
                input,
                transform: clip.transform.clone(),
                effects: self.clip_effects(clip, at, [0, 0])?,
            });
        }
        let mut plan = Plan {
            width: self.project.width,
            height: self.project.height,
            frames,
            clips,
            segments,
            resources: self.effect_resources.clone(),
            frame_buffer_bytes: 0,
        };
        plan.check_memory()?;
        Ok(plan)
    }
    /// Arguments, sources and generated inputs of a graph that reads no pictures.
    fn finish(self) -> (Vec<String>, Vec<Source>, Vec<render::Generated>) {
        (self.arguments(), self.sources(), self.generated)
    }
}
fn too_long(args: &[String]) -> bool {
    args.iter().map(|s| s.len() + 3).sum::<usize>() > 24000
}
fn boundaries(track: &Track, begin: Time, end: Time, clock: Time) -> Result<BTreeSet<u64>> {
    let first = begin.units(clock)?;
    let last = end.units(clock)?;
    let mut result = BTreeSet::from([first, last]);
    let mut add = |time: Time| -> Result<()> {
        let n = time.units(clock)?;
        if n > first && n < last {
            result.insert(n);
        }
        Ok(())
    };
    for clip in &track.clips {
        add(clip.start)?;
        add(clip.end()?)?;
    }
    for effect in &track.transitions {
        let (a, b) = track.interval(effect)?;
        add(a)?;
        add(b)?;
    }
    Ok(result)
}
/// Clips a render of `[start, end)` reads: those intersecting it, plus both endpoints of every
/// transition whose interval intersects it (a handle reads beyond its clip).
pub(crate) fn window_clips(
    a: &crate::tracks::Arrangement,
    start: Time,
    end: Time,
) -> Result<usize> {
    clips_where(a, start, end, |_| true)
}
/// `window_clips` over the tracks `include` selects.
fn clips_where(
    a: &crate::tracks::Arrangement,
    start: Time,
    end: Time,
    include: impl Fn(&Track) -> bool,
) -> Result<usize> {
    let mut ids = std::collections::BTreeSet::new();
    for track in a.tracks.iter().filter(|t| include(t)) {
        for clip in &track.clips {
            if clip.start.compare(end)?.is_lt() && clip.end()?.compare(start)?.is_gt() {
                ids.insert(&clip.id);
            }
        }
        for effect in &track.transitions {
            let (begin, finish) = track.interval(effect)?;
            if begin.compare(end)?.is_lt() && finish.compare(start)?.is_gt() {
                ids.insert(&effect.left_id);
                ids.insert(&effect.right_id);
            }
        }
    }
    Ok(ids.len())
}
/// Samples `[first, end)` split into consecutive windows, each one graph's worth of enabled
/// audio clips (at most MAX_GRAPH_CLIPS, transition endpoints included).
fn audio_windows(a: &crate::tracks::Arrangement, first: u64, end: u64) -> Result<Vec<(u64, u64)>> {
    let count = |from: u64, to: u64| -> Result<usize> {
        clips_where(a, Time::new(from, 48000)?, Time::new(to, 48000)?, |t| {
            t.enabled && t.kind == Kind::Audio
        })
    };
    let mut windows = Vec::new();
    let mut cursor = first;
    while cursor < end {
        if count(cursor, end)? <= render::MAX_GRAPH_CLIPS {
            windows.push((cursor, end));
            break;
        }
        // The longest window that still fits, by bisection over a monotone count.
        let (mut low, mut high) = (0, end - cursor);
        while low < high {
            let middle = (low + high).div_ceil(2);
            if count(cursor, cursor + middle)? <= render::MAX_GRAPH_CLIPS {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        if low == 0 {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "More than {} audio clips overlap at sample {cursor}",
                    render::MAX_GRAPH_CLIPS
                ),
            ));
        }
        windows.push((cursor, cursor + low));
        cursor += low;
    }
    Ok(windows)
}
/// A compiled window: native video streams plus the exact audio/encoding graph.
struct Compiled<'a> {
    stack: Option<crate::track_stack::Plan>,
    graph: Graph<'a>,
}
impl Compiled<'_> {
    fn compositor(&mut self, frames: u64) -> Result<Option<Compositor>> {
        let Some(stack) = self.stack.take() else {
            return Ok(None);
        };
        let resources = stack.resources.clone();
        Ok(Some(Compositor {
            width: stack.width,
            height: stack.height,
            frames,
            effect_report: resources.report()?,
            scene_resources: resources.scenes.report()?,
            effect_resources: resources,
            stack,
        }))
    }
}
fn compile<'a>(
    project: &'a Project,
    root: &'a Path,
    start: Time,
    duration: Time,
    (video, audio): (bool, bool),
    control: &'a dyn media::Control,
    effect_resources: &crate::clip_effects::Resources,
) -> Result<Compiled<'a>> {
    project.validate()?;
    media::input_root(root)?;
    let a = project.tracks.as_ref().expect("track project");
    let rate = render::clock::rate(project.frame_rate)?;
    let frames = duration.units(rate)?;
    let end = start.plus(duration)?;
    start.units(rate)?;
    if !(1..=180000).contains(&frames)
        || end.compare(a.duration)?.is_gt()
        || window_clips(a, start, end)? > render::MAX_GRAPH_CLIPS
        || project.width as u64 * project.height as u64 > 8_000_000
    {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Track rendering requires an in-bounds range of 1..180000 frames, at most 64 clips per graph and 8M pixels",
        ));
    }
    let mut graph = Graph::new(project, root, control);
    graph.effect_resources = effect_resources.clone();
    graph.converted =
        crate::converted::sources(project, root, media::source_preparation_allowed(), control)?;
    graph.audio_only = !video;
    graph.prefetch(a, start, duration, (video, audio));
    if video {
        graph.check_arrangement(a, start, duration, Kind::Video)?;
        let stack = graph.stack_plan(a, start, duration)?;
        let mut encoder = graph.successor();
        if audio {
            encoder.audio(a, start, duration)?;
        }
        return Ok(Compiled {
            graph: encoder,
            stack: Some(stack),
        });
    }

    if audio {
        graph.audio(a, start, duration)?;
    }
    Ok(Compiled { stack: None, graph })
}
pub(crate) fn plan(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    control: &dyn media::Control,
) -> Result<Plan> {
    plan_window(
        project,
        input_root,
        output_root,
        output,
        Time::ZERO,
        project.duration()?,
        control,
        &crate::clip_effects::Resources::default(),
    )
}
/// The premix of a whole timeline's audio, with or without limiters, in `parts` or more runs, and
/// the sources it reads: its group sums, captured once, replay the mix at other levels or through
/// another master limiter (`audio.normalize`).
pub(crate) fn premix(
    project: &Project,
    input_root: &Path,
    path: PathBuf,
    parts: u64,
) -> Result<(dynamics::Premix, Vec<Source>)> {
    project.validate()?;
    media::input_root(input_root)?;
    let a = project.tracks.as_ref().ok_or_else(|| {
        error(
            "UNSUPPORTED_TIMELINE",
            "Only placed-track timelines have a premix",
        )
    })?;
    let duration = project.duration()?;
    let mut graph = Graph::new(project, input_root, &media::Uncontrolled);
    graph.converted = crate::converted::sources(
        project,
        input_root,
        media::source_preparation_allowed(),
        &media::Uncontrolled,
    )?;
    graph.audio_only = true;
    graph.prefetch(a, Time::ZERO, duration, (false, true));
    graph.check_arrangement(a, Time::ZERO, duration, Kind::Audio)?;
    let premix = graph.premix(a, Time::ZERO, duration, path, parts)?;
    Ok((premix, graph.sources()))
}
/// Mix only the enabled audio tracks of a window into a lossless stereo PCM WAV, with the same
/// clip/sample placement, transitions and saturation as a full reference render, but no video work.
pub(crate) fn plan_audio_window(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Plan> {
    let output = render::destination_extension(output, output_root, "wav")?;
    let (mut args, sources, generated) = compile(
        project,
        input_root,
        start,
        duration,
        (false, true),
        &media::Uncontrolled,
        &crate::clip_effects::Resources::default(),
    )?
    .graph
    .finish();
    args.extend(
        [
            "-map",
            "[aout]",
            "-c:a",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-f",
            "wav",
        ]
        .map(str::to_owned),
    );
    args.push(output.to_string_lossy().into_owned());
    Ok(Plan {
        profile: "reference-tracks-pcm-audio-v1",
        source_quality: "original",
        project_revision: project.revision,
        frames: duration.units(project.frame_rate)?,
        samples: duration.units(SAMPLES)?,
        output,
        sources,
        arguments: args,
        chunks: Vec::new(),
        generated,
        compositor: None,
    })
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_window(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
    control: &dyn media::Control,
    effect_resources: &crate::clip_effects::Resources,
) -> Result<Plan> {
    let output = render::destination(output, output_root)?;
    let frames = duration.units(project.frame_rate)?;
    let mut compiled = compile(
        project,
        input_root,
        start,
        duration,
        (true, true),
        control,
        effect_resources,
    )?;
    let compositor = compiled.compositor(frames)?;
    let (level, slices) = media::ffv1_encoding(project.width, project.height);
    let mut tail = Vec::new();
    tail.extend(
        [
            "-map",
            // The engine-composited picture arrives raw on stdin as input 0.
            "0:v:0",
            "-map",
            "[aout]",
            "-c:v",
            "ffv1",
            "-level",
            level,
            "-slices",
            slices,
            "-pix_fmt",
            "bgr0",
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-f",
            "matroska",
        ]
        .map(str::to_owned),
    );
    let rate = project.frame_rate;
    if rate == FPS {
        tail.extend(["-r".into(), "25".into()]);
    } else {
        tail.extend([
            "-r".into(),
            format!("{}/{}", rate.num, rate.den),
            "-fps_mode".into(),
            "cfr".into(),
            "-enc_time_base:v".into(),
            timebase(rate),
        ]);
    }
    tail.push(output.to_string_lossy().into_owned());
    let args = compiled.graph.arguments_with(&tail)?;
    let (sources, generated) = (compiled.graph.sources(), compiled.graph.generated);
    Ok(Plan {
        profile: if rate == FPS {
            "reference-tracks-ffv1-pcm-v1"
        } else {
            "reference-tracks-ffv1-pcm-rational-v2"
        },
        source_quality: "original",
        project_revision: project.revision,
        frames,
        samples: duration.units(SAMPLES)?,
        output,
        sources,
        arguments: args,
        chunks: Vec::new(),
        generated,
        compositor,
    })
}
pub(crate) fn read_frame(
    project: &Project,
    root: &Path,
    time: Time,
) -> Result<(Vec<u8>, Vec<Source>)> {
    let mut compiled = compile(
        project,
        root,
        time,
        Time::new(project.frame_rate.den, project.frame_rate.num)?,
        (true, false),
        &media::Uncontrolled,
        &crate::clip_effects::Resources::default(),
    )?;
    let compositor = compiled.compositor(1)?.expect("compiled video stream");
    let pixels =
        track_composite::frame_rgb(&compositor, Duration::from_secs(120), &media::Uncontrolled)?;
    let sources = compiled.graph.sources();
    if pixels.len() != project.width as usize * project.height as usize * 3 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Transition preview frame size differs",
        ));
    }
    for source in &sources {
        source.verify(&media::Uncontrolled)?;
    }
    Ok((pixels, sources))
}
