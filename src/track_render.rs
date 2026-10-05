//! Compile placed tracks and transitions using exact frame/sample interval boundaries.
use crate::{
    Result,
    dynamics::{self, Limiter},
    error, media,
    model::Project,
    render::{self, Plan, Source},
    time::Time,
    track_composite::{self, Compositor},
    tracks::{Kind, OverlayTransform, Track, TrackClip, Transition, TransitionKind},
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
    /// Gain streams for clips with gain curves, written when the graph runs.
    generated: Vec<render::Generated>,
    /// Distinguishes this graph's generated file names.
    nonce: u128,
    /// When the top-level overlays are composited by the engine: the base picture's frame runs
    /// and the index of their visible opaque track.
    runs: Vec<(u64, u64, Option<usize>)>,
}
impl<'a> Graph<'a> {
    fn new(project: &'a Project, root: &'a Path, control: &'a dyn media::Control) -> Self {
        Self {
            project,
            root,
            control,
            sources: BTreeMap::new(),
            indices: BTreeMap::new(),
            runs: Vec::new(),
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
        let source = self.inspect(clip, kind, overlay)?;
        let (first, end) = self.span(clip, at, duration, kind, &source)?;
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
            let path = media::project_file(Path::new(&asset.path), self.root)?;
            let source = if let Some(check) = self.inspection(&path, kind, overlay) {
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
                    path: path.clone(),
                    sha256: wave.sha256,
                    frames: 0,
                    samples: wave.frames,
                }
            };
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
        let tb = timebase(self.project.frame_rate);
        self.filters.push(format!("[{input}:v:0]trim=start_frame={first}:end_frame={end},settb=expr={tb},setpts=N,format=pix_fmts=gbrp[{label}]"));
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
        let tb = timebase(self.project.frame_rate);
        let source = format!(
            "[{input}:v:0]trim=start_frame={first}:end_frame={end},settb=expr={tb},setpts=N"
        );
        if let Some(transform) = &clip.transform {
            return self.transformed_overlay(clip, transform, &source, label);
        }
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
    /// A picture-in-picture overlay on a transparent full canvas: crop, shrink by the floor of
    /// each block's mean (`pixelize`, then exact neighbor decimation of the constant blocks), scale
    /// alpha by the opacity through a lookup table, then crop to the visible part and pad.
    fn transformed_overlay(
        &mut self,
        clip: &TrackClip,
        transform: &OverlayTransform,
        source: &str,
        label: &str,
    ) -> Result<()> {
        let (width, height) = (self.project.width, self.project.height);
        let alpha = self.alpha_assets.contains(&clip.asset_id);
        let k = transform.divisor;
        if alpha && k > 1 {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                format!(
                    "Clip {:?}: only opaque overlay sources can be shrunk; straight-alpha sources may be cropped, faded and placed",
                    clip.id
                ),
            ));
        }
        let planes = format!("{label}m");
        if alpha {
            self.filters.push(format!(
                "{source},extractplanes=r+g+b+a[{label}r][{label}g][{label}b][{label}a];[{label}r][{label}g][{label}b][{label}a]mergeplanes=map0s=1:map0p=0:map1s=2:map1p=0:map2s=0:map2p=0:map3s=3:map3p=0:format=gbrap[{planes}]"
            ));
        } else {
            self.filters
                .push(format!("{source},format=pix_fmts=gbrp[{planes}]"));
        }
        let [cx, cy, cw, ch] = transform.crop.unwrap_or([0, 0, width, height]);
        let mut chain = Vec::new();
        if [cx, cy, cw, ch] != [0, 0, width, height] {
            chain.push(format!("crop={cw}:{ch}:{cx}:{cy}"));
        }
        if k > 1 {
            chain.push(format!(
                "pixelize=w={k}:h={k}:m=avg,scale={}:{}:flags=neighbor",
                cw / k,
                ch / k
            ));
        }
        if !alpha {
            chain.push("format=pix_fmts=gbrap".into());
        }
        if transform.opacity < 255 {
            chain.push(format!(
                "lutrgb=a='floor((val*{}+127)/255)'",
                transform.opacity
            ));
        }
        // The shrunk image occupies [x, x + w) x [y, y + h) on the canvas; keep what is visible.
        let (w, h) = (i64::from(cw / k), i64::from(ch / k));
        let [x, y] = transform.position.map(i64::from);
        let (left, top) = (x.max(0), y.max(0));
        let (right, bottom) = ((x + w).min(width.into()), (y + h).min(height.into()));
        if left >= right || top >= bottom {
            chain.push("crop=1:1:0:0,lutrgb=a=0".into());
            chain.push(format!("pad={width}:{height}:0:0:color=black@0"));
        } else {
            if (left, top, right, bottom) != (x, y, x + w, y + h) {
                chain.push(format!(
                    "crop={}:{}:{}:{}",
                    right - left,
                    bottom - top,
                    left - x,
                    top - y
                ));
            }
            if (left, top, right, bottom) != (0, 0, width.into(), height.into()) {
                chain.push(format!("pad={width}:{height}:{left}:{top}:color=black@0"));
            }
        }
        if chain.is_empty() {
            chain.push("null".into());
        }
        self.filters
            .push(format!("[{planes}]{}[{label}]", chain.join(",")));
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
        let raw = if clip.adjusts_audio() {
            format!("{label}g")
        } else {
            label.to_string()
        };
        if let Some(id) = &clip.sequence_id {
            let a = crate::sequences::get(self.project, id)?.arrangement.clone();
            self.compose(
                &a,
                clip.source_in.plus(at)?.minus(clip.start)?,
                duration,
                Kind::Audio,
                &raw,
            )?;
        } else {
            let (input, first, end) = self.input(clip, at, duration, Kind::Audio)?;
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
        let units = track.kind.clock(self.project.frame_rate);
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
            let rate = self.project.frame_rate;
            let k = format!("(2*(round(T*{}/{})+{offset})+1)", rate.num, rate.den);
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
    fn compose(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        kind: Kind,
        output: &str,
    ) -> Result<()> {
        self.compose_with(a, start, duration, kind, output, false)
    }
    /// `deferred` leaves this arrangement's `alpha_over` tracks to the engine compositor: the
    /// picture is cut only where the visible opaque content changes, and its runs are recorded.
    fn compose_with(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
        kind: Kind,
        output: &str,
        deferred: bool,
    ) -> Result<()> {
        self.control.check()?;
        let end = start.plus(duration)?;
        let rate = self.project.frame_rate;
        let clock = kind.clock(rate);
        start.units(clock)?;
        let count = duration.units(clock)?;
        if count == 0 || end.compare(a.duration)?.is_gt() {
            return Err(error(
                "INVALID_RANGE",
                "Composition requires a positive in-bounds child window",
            ));
        }
        if kind == Kind::Video {
            let first = start.units(rate)?;
            let mut edges = BTreeSet::from([first, end.units(rate)?]);
            for t in a
                .tracks
                .iter()
                .filter(|t| t.enabled && t.kind == Kind::Video)
                .filter(|t| !deferred || t.composite.is_opaque())
            {
                edges.extend(boundaries(t, start, end, rate)?);
            }
            let edges: Vec<_> = edges.into_iter().collect();
            let mut labels = String::new();
            for part in edges.windows(2) {
                let n = part[1] - part[0];
                let at = Time::new(part[0] * rate.den, rate.num)?;
                let length = Time::new(n * rate.den, rate.num)?;
                let label = self.label()?;
                labels.push_str(&format!("[{label}]"));
                let visible = a.visible(at)?;
                let base_index = visible.map(|(track, _)| {
                    a.tracks
                        .iter()
                        .position(|t| std::ptr::eq(t, track))
                        .expect("visible track")
                });
                if deferred {
                    self.runs
                        .push((part[0] - first, part[1] - first, base_index));
                }
                let mut overlays = Vec::new();
                for (index, track) in a.tracks.iter().enumerate() {
                    if !deferred
                        && track.kind == Kind::Video
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
                    self.filters.push(format!("color=c=black:s={}x{}:r={}/{},format=pix_fmts=gbrp,trim=end_frame={n},settb=expr={},setpts=N[{base}]", self.project.width, self.project.height, rate.num, rate.den, timebase(rate)));
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
                "{labels}concat=n={}:v=1:a=0,settb=expr={},setpts=N[{output}]",
                edges.len() - 1,
                timebase(rate)
            ));
        } else {
            let tracks: Vec<usize> = (0..a.tracks.len())
                .filter(|&i| a.tracks[i].enabled && a.tracks[i].kind == Kind::Audio)
                .collect();
            // A child is a reusable PCM16 sequence, so saturate at its own mix boundary.
            self.mix(a, &tracks, start, duration, output, true)?;
        }
        Ok(())
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
            return self.compose(a, start, duration, Kind::Audio, "aout");
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
        values.sort_by(|a, b| a.path.cmp(&b.path));
        values
    }
    /// A graph for the encoder of an engine-composited picture: input 0 is that picture on
    /// stdin, and everything already inspected and counted carries over.
    fn successor(&mut self) -> Graph<'a> {
        let mut next = Graph::new(self.project, self.root, self.control);
        next.args.extend(track_composite::raw_input(
            self.project.width,
            self.project.height,
            self.project.frame_rate,
        ));
        next.inputs = 1;
        next.sources = self.sources.clone();
        next.overlay_assets = self.overlay_assets.clone();
        next.alpha_assets = self.alpha_assets.clone();
        next.audio_only = self.audio_only;
        next.packet_timed = self.packet_timed.clone();
        next.prefetched = std::mem::take(&mut self.prefetched);
        next.serial = self.serial;
        next.inspected = self.inspected;
        next
    }
    /// The shown clips of `a`'s `alpha_over` tracks over `[start, start + duration)`, each with a
    /// decoder of its cropped frames, using the runs of a deferred composition of the same window.
    fn layers(
        &mut self,
        a: &crate::tracks::Arrangement,
        start: Time,
        duration: Time,
    ) -> Result<Vec<track_composite::Layer>> {
        let rate = self.project.frame_rate;
        let (width, height) = (self.project.width, self.project.height);
        let (window, frames) = (start.units(rate)?, duration.units(rate)?);
        let tb = timebase(rate);
        let mut layers = Vec::new();
        for (index, track) in a.tracks.iter().enumerate() {
            if track.kind != Kind::Video || !track.enabled || track.composite.is_opaque() {
                continue;
            }
            for clip in &track.clips {
                let (begin, finish) = (clip.start.units(rate)?, clip.end()?.units(rate)?);
                if finish <= window || begin >= window + frames {
                    continue;
                }
                let (from, to) = (
                    begin.max(window) - window,
                    finish.min(window + frames) - window,
                );
                // Frames where no opaque track above the clip hides it.
                let mut shown: Option<(u64, u64)> = None;
                for &(run_start, run_end, base) in &self.runs {
                    let (s, e) = (run_start.max(from), run_end.min(to));
                    if s < e && base.is_none_or(|b| index > b) {
                        shown = Some(shown.map_or((s, e), |(a, b)| (a.min(s), b.max(e))));
                    }
                }
                let Some((first, end)) = shown else {
                    continue;
                };
                let at = Time::new((window + first) * rate.den, rate.num)?;
                let length = Time::new((end - first) * rate.den, rate.num)?;
                let source = self.inspect(clip, Kind::Video, true)?;
                let (source_first, source_end) =
                    self.span(clip, at, length, Kind::Video, &source)?;
                let alpha = self.alpha_assets.contains(&clip.asset_id);
                let (crop, divisor, opacity, position) = match &clip.transform {
                    Some(t) => (t.crop, t.divisor, t.opacity, t.position),
                    None => (None, 1, 255, [0, 0]),
                };
                if alpha && divisor > 1 {
                    return Err(error(
                        "UNSUPPORTED_MEDIA",
                        format!(
                            "Clip {:?}: only opaque overlay sources can be shrunk; straight-alpha sources may be cropped, faded and placed",
                            clip.id
                        ),
                    ));
                }
                let [cx, cy, cw, ch] = crop.unwrap_or([0, 0, width, height]);
                // Planar conversion of packed alpha through the scaler is not value-exact, so the
                // decoded bgra planes are split and reassembled as gbrap without arithmetic.
                let convert = if alpha {
                    "extractplanes=r+g+b+a[r][g][b][a];[r][g][b][a]mergeplanes=map0s=1:map0p=0:map1s=2:map1p=0:map2s=0:map2p=0:map3s=3:map3p=0:format=gbrap"
                } else {
                    "format=pix_fmts=gbrp"
                };
                let cropped = if [cx, cy, cw, ch] == [0, 0, width, height] {
                    String::new()
                } else {
                    format!(",crop={cw}:{ch}:{cx}:{cy}")
                };
                let mut arguments: Vec<String> = [
                    "-hide_banner",
                    "-v",
                    "error",
                    "-nostdin",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-i",
                ]
                .map(str::to_owned)
                .to_vec();
                arguments.push(source.path.to_string_lossy().into_owned());
                arguments.extend([
                    "-filter_complex_threads".into(),
                    "1".into(),
                    "-filter_complex".into(),
                    format!("[0:v:0]trim=start_frame={source_first}:end_frame={source_end},settb=expr={tb},setpts=N,{convert}{cropped}[out]"),
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
                layers.push(track_composite::Layer {
                    track: index,
                    track_id: track.id.clone(),
                    clip_id: clip.id.clone(),
                    first,
                    end,
                    width: cw,
                    height: ch,
                    alpha,
                    divisor,
                    opacity,
                    position,
                    arguments,
                });
            }
        }
        Ok(layers)
    }
    fn finish(mut self) -> (Vec<String>, Vec<Source>, Vec<render::Generated>) {
        let sources = self.sources();
        self.args.extend([
            "-filter_complex_threads".into(),
            "1".into(),
            "-filter_complex".into(),
            self.filters.join(";"),
        ]);
        (self.args, sources, self.generated)
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
/// A compiled window: one FFmpeg graph, or, when top-level `alpha_over` clips show, the base
/// picture's graph and the overlay clips for the engine compositor beside the encoder's graph.
struct Compiled<'a> {
    /// The single graph, or the encoder's graph (audio, with the composited picture as input 0).
    graph: Graph<'a>,
    picture: Option<(Graph<'a>, Vec<track_composite::Layer>)>,
}
impl Compiled<'_> {
    /// The engine compositor of this window, with the base decoder writing raw planar RGB.
    fn compositor(&mut self, frames: u64) -> Result<Option<Compositor>> {
        let Some((picture, layers)) = self.picture.take() else {
            return Ok(None);
        };
        let project = picture.project;
        let runs = picture.runs.clone();
        let (mut base, _, _) = picture.finish();
        base.extend(
            [
                "-map",
                "[vout]",
                "-an",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gbrp",
                "pipe:1",
            ]
            .map(str::to_owned),
        );
        for arguments in std::iter::once(&base).chain(layers.iter().map(|l| &l.arguments)) {
            if arguments.iter().map(|s| s.len() + 3).sum::<usize>() > 24000 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Track render arguments exceed supported size",
                ));
            }
        }
        Ok(Some(Compositor {
            width: project.width,
            height: project.height,
            frames,
            base,
            runs,
            layers,
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
    graph.audio_only = !video;
    graph.prefetch(a, start, duration, (video, audio));
    if video {
        graph.check_arrangement(a, start, duration, Kind::Video)?;
        let overlays = a
            .tracks
            .iter()
            .filter(|t| t.kind == Kind::Video && t.enabled && !t.composite.is_opaque())
            .flat_map(|t| &t.clips)
            .map(|c| Ok(c.start.compare(end)?.is_lt() && c.end()?.compare(start)?.is_gt()))
            .collect::<Result<Vec<_>>>()?
            .contains(&true);
        if overlays {
            // Top-level overlays are composited by the engine; nested ones stay in the graph.
            graph.compose_with(a, start, duration, Kind::Video, "vout", true)?;
            let layers = graph.layers(a, start, duration)?;
            if !layers.is_empty() {
                let mut encoder = graph.successor();
                if audio {
                    encoder.audio(a, start, duration)?;
                }
                return Ok(Compiled {
                    graph: encoder,
                    picture: Some((graph, layers)),
                });
            }
        } else {
            graph.compose(a, start, duration, Kind::Video, "vout")?;
        }
    }
    if audio {
        graph.audio(a, start, duration)?;
    }
    Ok(Compiled {
        graph,
        picture: None,
    })
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
) -> Result<Plan> {
    let output = render::destination(output, output_root)?;
    let frames = duration.units(project.frame_rate)?;
    let mut compiled = compile(project, input_root, start, duration, (true, true), control)?;
    let compositor = compiled.compositor(frames)?;
    let (mut args, sources, generated) = compiled.graph.finish();
    let (level, slices) = media::ffv1_encoding(project.width, project.height);
    args.extend(
        [
            "-map",
            // The engine-composited picture arrives raw on stdin as input 0.
            if compositor.is_some() {
                "0:v:0"
            } else {
                "[vout]"
            },
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
        args.extend(["-r".into(), "25".into()]);
    } else {
        args.extend([
            "-r".into(),
            format!("{}/{}", rate.num, rate.den),
            "-fps_mode".into(),
            "cfr".into(),
            "-enc_time_base:v".into(),
            timebase(rate),
        ]);
    }
    args.push(output.to_string_lossy().into_owned());
    if args.iter().map(|s| s.len() + 3).sum::<usize>() > 24000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Track render arguments exceed supported size",
        ));
    }
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
    )?;
    let pixels = if let Some(compositor) = compiled.compositor(1)? {
        track_composite::frame_rgb(&compositor, Duration::from_secs(120), &media::Uncontrolled)?
    } else {
        let mut args = std::mem::take(&mut compiled.graph.args);
        args.extend([
            "-filter_complex_threads".into(),
            "1".into(),
            "-filter_complex".into(),
            compiled.graph.filters.join(";"),
        ]);
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
        media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(120))?
    };
    let sources = compiled.graph.sources();
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
