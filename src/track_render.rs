//! Compile placed tracks and transitions using exact frame/sample interval boundaries.
use crate::{
    Result, error, media,
    model::Project,
    render::{self, Plan, Source},
    time::Time,
    tracks::{Kind, Track, TrackClip, Transition, TransitionKind},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};
const FPS: Time = Time { num: 25, den: 1 };
const SAMPLES: Time = Time { num: 48000, den: 1 };

struct Graph<'a> {
    project: &'a Project,
    root: &'a Path,
    control: &'a dyn media::Control,
    sources: BTreeMap<String, (usize, Source)>,
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
}
impl<'a> Graph<'a> {
    fn new(project: &'a Project, root: &'a Path, control: &'a dyn media::Control) -> Self {
        Self {
            project,
            root,
            control,
            sources: BTreeMap::new(),
            args: ["-hide_banner", "-v", "error", "-nostdin", "-n"]
                .map(str::to_owned)
                .to_vec(),
            filters: vec![],
            serial: 0,
            inspected: 0,
            overlay_assets: BTreeSet::new(),
            alpha_assets: BTreeSet::new(),
            audio_only: false,
        }
    }
    fn input(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        kind: Kind,
    ) -> Result<(usize, u64, u64)> {
        self.input_with(clip, at, duration, kind, false)
    }
    fn input_with(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        kind: Kind,
        overlay: bool,
    ) -> Result<(usize, u64, u64)> {
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
        if !self.sources.contains_key(&clip.asset_id) {
            let asset = self
                .project
                .assets
                .iter()
                .find(|a| a.id == clip.asset_id)
                .expect("validated asset");
            let path = media::project_file(Path::new(&asset.path), self.root)?;
            let source = if kind == Kind::Audio
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            {
                let wave = crate::pcm_stream::inspect(&path, self.control)?;
                Source {
                    path: path.clone(),
                    sha256: wave.sha256,
                    frames: 0,
                    samples: wave.frames,
                }
            } else if overlay {
                let (source, alpha) = render::inspect_overlay(
                    &path,
                    self.project.width,
                    self.project.height,
                    self.control,
                )?;
                self.overlay_assets.insert(clip.asset_id.clone());
                if alpha {
                    self.alpha_assets.insert(clip.asset_id.clone());
                }
                source
            } else if self.audio_only {
                render::inspect_reference_audio(
                    &path,
                    self.project.width,
                    self.project.height,
                    FPS,
                    self.control,
                )?
            } else {
                render::inspect_reference(
                    &path,
                    self.project.width,
                    self.project.height,
                    self.control,
                )?
            };
            crate::registry::verify_source(asset, &source)?;
            self.args.extend([
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-i".into(),
                path.to_string_lossy().into_owned(),
            ]);
            self.sources
                .insert(asset.id.clone(), (self.sources.len(), source));
        }
        let (input, source) = &self.sources[&clip.asset_id];
        if kind == Kind::Video && source.frames == 0 {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                "Audio-only WAV cannot supply video",
            ));
        }
        let first = clip
            .source_in
            .plus(at)?
            .minus(clip.start)?
            .units(kind.clock(FPS))?;
        let count = duration.units(kind.clock(FPS))?;
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
        Ok((*input, first, first + count))
    }
    fn video_source(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        label: &str,
    ) -> Result<()> {
        if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            return self.compose(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                Kind::Video,
                label,
            );
        }
        let (input, first, end) = self.input(clip, at, duration, Kind::Video)?;
        self.filters.push(format!("[{input}:v:0]trim=start_frame={first}:end_frame={end},settb=expr=1/25,setpts=N,format=pix_fmts=gbrp[{label}]"));
        Ok(())
    }
    /// A clip on an alpha_over track, keeping its straight alpha plane (opaque sources read as 255).
    fn overlay_source(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        label: &str,
    ) -> Result<()> {
        let (input, first, end) = self.input_with(clip, at, duration, Kind::Video, true)?;
        let source = format!(
            "[{input}:v:0]trim=start_frame={first}:end_frame={end},settb=expr=1/25,setpts=N"
        );
        if self.alpha_assets.contains(&clip.asset_id) {
            // Planar conversion of packed alpha through the scaler is not value-exact, so split the
            // decoded bgra planes and reassemble them as gbrap (G, B, R, A) without any arithmetic.
            self.filters.push(format!(
                "{source},extractplanes=r+g+b+a[{label}r][{label}g][{label}b][{label}a];[{label}r][{label}g][{label}b][{label}a]mergeplanes=map0s=1:map0p=0:map1s=2:map1p=0:map2s=0:map2p=0:map3s=3:map3p=0:format=gbrap[{label}]"
            ));
        } else {
            // Opaque bgr0 overlays: exact RGB conversion with a constant 255 alpha plane.
            self.filters
                .push(format!("{source},format=pix_fmts=gbrap[{label}]"));
        }
        Ok(())
    }
    /// Straight-alpha "over" in encoded RGB with exact integer rounding (ties up):
    /// out = floor((2*(s*a + d*(255-a)) + 255) / 510). Base and overlay are stacked side by side
    /// so the per-pixel expression can read both; values stay exact in double precision.
    fn over(&mut self, base: &str, overlay: &str, output: &str) -> Result<()> {
        let width = self.project.width;
        let channel = |c: &str| {
            format!(
                "floor((2*({c}(X+{width},Y)*alpha(X+{width},Y)+{c}(X,Y)*(255-alpha(X+{width},Y)))+255)/510)"
            )
        };
        let stacked = self.label()?;
        self.filters.push(format!(
            "[{base}]format=pix_fmts=gbrap[{stacked}b];[{stacked}b][{overlay}]hstack=inputs=2,geq=r='{}':g='{}':b='{}':a='255':interpolation=nearest,crop={width}:{}:0:0,format=pix_fmts=gbrp,settb=expr=1/25,setpts=N[{output}]",
            channel("r"),
            channel("g"),
            channel("b"),
            self.project.height
        ));
        Ok(())
    }
    fn audio_source(
        &mut self,
        clip: &TrackClip,
        at: Time,
        duration: Time,
        label: &str,
    ) -> Result<()> {
        if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            return self.compose(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                Kind::Audio,
                label,
            );
        }
        let (input, first, end) = self.input(clip, at, duration, Kind::Audio)?;
        self.filters.push(format!("[{input}:a:0]atrim=start_sample={first}:end_sample={end},asetpts=N/SR/TB,aformat=sample_fmts=dblp:channel_layouts=stereo[{label}]"));
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
        let units = track.kind.clock(FPS);
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
        if track.kind == Kind::Video {
            self.video_source(left, at, duration, &l)?;
            self.video_source(right, at, duration, &r)?;
            // Sample the transition at each frame center. Timestamp rounding recovers
            // the exact local frame index from the filter's rational clock.
            let k = format!("(2*(round(T*25)+{offset})+1)");
            let expr = match effect.kind {
                TransitionKind::Dissolve => format!("floor((A*({d}-{k})+B*{k}+{count})/{d})"),
                TransitionKind::DipBlack => {
                    format!("floor((A*max({d}-2*{k},0)+B*max(2*{k}-{d},0)+{count})/{d})")
                }
                TransitionKind::WipeLeft => format!("if(lte((2*X+1)*{count},W*{k}),B,A)"),
                TransitionKind::WipeRight => format!("if(lte((2*(W-X)-1)*{count},W*{k}),B,A)"),
            };
            self.filters.push(format!(
                "[{l}][{r}]blend=all_expr='{expr}':shortest=1[{label}]"
            ));
        } else {
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
        }
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
    fn check_clip(&mut self, clip: &TrackClip, at: Time, duration: Time, kind: Kind) -> Result<()> {
        self.control.check()?;
        self.inspected += 1;
        if self.inspected > 256 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Nested render exceeds 256 expanded clip/handle inspections",
            ));
        }
        if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            self.check_arrangement(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                kind,
            )?;
        } else {
            self.input(clip, at, duration, kind)?;
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
                    if t.composite.is_opaque() {
                        self.check_clip(c, c.start, c.duration, kind)?;
                    } else {
                        // Validated alpha_over tracks hold asset clips only, inspected with alpha.
                        self.input_with(c, c.start, c.duration, kind, true)?;
                    }
                }
            }
            // Even covered effects must have valid decoded handles and identities.
            for fx in &t.transitions {
                let (x, y) = t.interval(fx)?;
                if x.compare(end)?.is_lt() && y.compare(start)?.is_gt() {
                    y.minus(x)?
                        .units(kind.clock(FPS))?
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
    fn compose(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        kind: Kind,
        output: &str,
    ) -> Result<()> {
        self.control.check()?;
        let end = start.plus(duration)?;
        let clock = kind.clock(FPS);
        start.units(clock)?;
        let count = duration.units(clock)?;
        if count == 0 || end.compare(a.duration)?.is_gt() {
            return Err(error(
                "INVALID_RANGE",
                "Composition requires a positive in-bounds child window",
            ));
        }
        if kind == Kind::Video {
            let mut edges = BTreeSet::from([start.units(FPS)?, end.units(FPS)?]);
            for t in a
                .tracks
                .iter()
                .filter(|t| t.enabled && t.kind == Kind::Video)
            {
                edges.extend(boundaries(t, start, end, FPS)?);
            }
            let edges: Vec<_> = edges.into_iter().collect();
            let mut labels = String::new();
            for part in edges.windows(2) {
                let n = part[1] - part[0];
                let at = Time::new(part[0], 25)?;
                let length = Time::new(n, 25)?;
                let label = self.label()?;
                labels.push_str(&format!("[{label}]"));
                let visible = a.visible(at)?;
                let base_index = visible.map(|(track, _)| {
                    a.tracks
                        .iter()
                        .position(|t| std::ptr::eq(t, track))
                        .expect("visible track")
                });
                let mut overlays = Vec::new();
                for (index, track) in a.tracks.iter().enumerate() {
                    if track.kind == Kind::Video
                        && track.enabled
                        && !track.composite.is_opaque()
                        && base_index.is_none_or(|b| index > b)
                        && let Some(clip) = track.clips.iter().find(|c| {
                            !at.compare(c.start).expect("validated").is_lt()
                                && at
                                    .compare(c.end().expect("validated"))
                                    .expect("validated")
                                    .is_lt()
                        })
                    {
                        overlays.push(clip);
                    }
                }
                let base = if overlays.is_empty() {
                    label.clone()
                } else {
                    self.label()?
                };
                if let Some((track, clip)) = visible {
                    if let Some(fx) = track.transition_at(at)? {
                        self.effect(track, fx, at, length, &base)?;
                    } else {
                        self.video_source(clip, at, length, &base)?;
                    }
                } else {
                    self.filters.push(format!("color=c=black:s={}x{}:r=25,format=pix_fmts=gbrp,trim=end_frame={n},settb=expr=1/25,setpts=N[{base}]", self.project.width, self.project.height));
                }
                let mut below = base;
                for (i, clip) in overlays.iter().enumerate() {
                    let source = self.label()?;
                    self.overlay_source(clip, at, length, &source)?;
                    let output = if i + 1 == overlays.len() {
                        label.clone()
                    } else {
                        self.label()?
                    };
                    self.over(&below, &source, &output)?;
                    below = output;
                }
            }
            self.filters.push(format!(
                "{labels}concat=n={}:v=1:a=0,settb=expr=1/25,setpts=N[{output}]",
                edges.len() - 1
            ));
        } else {
            let begin = start.units(SAMPLES)?;
            let mut labels = String::new();
            let mut voices = 0;
            for track in a
                .tracks
                .iter()
                .filter(|t| t.enabled && t.kind == Kind::Audio)
            {
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
                // A child is a reusable PCM16 sequence, so saturate at its own mix boundary.
                self.filters.push(format!("{labels}amix=inputs={voices}:duration=longest:dropout_transition=0:normalize=0,apad=whole_len={count},atrim=end_sample={count},aformat=sample_fmts=s16:sample_rates=48000:channel_layouts=stereo,aformat=sample_fmts=dblp[{output}]"));
            }
        }
        Ok(())
    }
    fn sources(&self) -> Vec<Source> {
        let mut values: Vec<_> = self.sources.values().map(|(_, s)| s.clone()).collect();
        values.sort_by(|a, b| a.path.cmp(&b.path));
        values
    }
    fn finish(mut self) -> (Vec<String>, Vec<Source>) {
        let sources = self.sources();
        self.args.extend([
            "-filter_complex_threads".into(),
            "1".into(),
            "-filter_complex".into(),
            self.filters.join(";"),
        ]);
        (self.args, sources)
    }
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
fn compile<'a>(
    project: &'a Project,
    root: &'a Path,
    start: Time,
    duration: Time,
    (video, audio): (bool, bool),
    control: &'a dyn media::Control,
) -> Result<Graph<'a>> {
    project.validate()?;
    media::input_root(root)?;
    let a = project.tracks.as_ref().expect("track project");
    let frames = duration.units(FPS)?;
    let end = start.plus(duration)?;
    start.units(FPS)?;
    if project.frame_rate.compare(FPS)?.is_ne()
        || !(1..=180000).contains(&frames)
        || end.compare(a.duration)?.is_gt()
        || a.tracks.iter().map(|t| t.clips.len()).sum::<usize>() > 64
        || project.width as u64 * project.height as u64 > 8_000_000
    {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Track rendering requires an in-bounds 25 fps range of 1..180000 frames, at most 64 clips and 8M pixels",
        ));
    }
    let mut graph = Graph::new(project, root, control);
    graph.audio_only = !video;
    if video {
        graph.check_arrangement(a, start, duration, Kind::Video)?;
        graph.compose(a, start, duration, Kind::Video, "vout")?;
    }
    if audio {
        graph.check_arrangement(a, start, duration, Kind::Audio)?;
        graph.compose(a, start, duration, Kind::Audio, "aout")?;
    }
    Ok(graph)
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
    )
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
    let (mut args, sources) = compile(
        project,
        input_root,
        start,
        duration,
        (false, true),
        &media::Uncontrolled,
    )?
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
        frames: duration.units(FPS)?,
        samples: duration.units(SAMPLES)?,
        output,
        sources,
        arguments: args,
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
) -> Result<Plan> {
    let output = render::destination(output, output_root)?;
    let (mut args, sources) =
        compile(project, input_root, start, duration, (true, true), control)?.finish();
    let (level, slices) = media::ffv1_encoding(project.width, project.height);
    args.extend(
        [
            "-map",
            "[vout]",
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
            "-r",
            "25",
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
    args.push(output.to_string_lossy().into_owned());
    if args.iter().map(|s| s.len() + 3).sum::<usize>() > 24000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Track render arguments exceed supported size",
        ));
    }
    Ok(Plan {
        profile: "reference-tracks-ffv1-pcm-v1",
        source_quality: "original",
        project_revision: project.revision,
        frames: duration.units(FPS)?,
        samples: duration.units(SAMPLES)?,
        output,
        sources,
        arguments: args,
    })
}
pub(crate) fn read_frame(
    project: &Project,
    root: &Path,
    time: Time,
) -> Result<(Vec<u8>, Vec<Source>)> {
    let (mut args, sources) = compile(
        project,
        root,
        time,
        Time::new(1, 25)?,
        (true, false),
        &media::Uncontrolled,
    )?
    .finish();
    args.extend(
        [
            "-map",
            "[vout]",
            "-frames:v",
            "1",
            "-an",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ]
        .map(str::to_owned),
    );
    if args.iter().map(|s| s.len() + 3).sum::<usize>() > 24000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Transition preview arguments exceed supported size",
        ));
    }
    let pixels = media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(120))?;
    if pixels.len() != project.width as usize * project.height as usize * 3 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Transition preview frame size differs",
        ));
    }
    for source in &sources {
        if media::file_hash(&source.path)? != source.sha256 {
            return Err(error(
                "MEDIA_CHANGED",
                "Source changed during transition preview",
            ));
        }
    }
    Ok((pixels, sources))
}
