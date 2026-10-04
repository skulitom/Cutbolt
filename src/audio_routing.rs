//! Original deterministic PCM routing graph, panning and named-layout publication.
use crate::{
    Result,
    animation::{Curve, Sampler},
    audio::{self, Mix, Voice},
    audio_processing::{Effect, Processor},
    error, media,
    pcm_wave::{self, Layout},
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

const HEADROOM: i64 = 1_000_000_000_000;
const WORK: u64 = 512_000_000;
fn unity() -> u32 {
    1000
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_AUDIO_ROUTING", message)
}
fn bounded(values: &[i64]) -> Result<()> {
    if values.iter().any(|v| !(-HEADROOM..=HEADROOM).contains(v)) {
        Err(error(
            "AUDIO_ROUTING_OVERFLOW",
            "Routing node exceeds declared intermediate headroom",
        ))
    } else {
        Ok(())
    }
}
/// Routing graph node, tagged by `kind`: a mix track, a bus or the final output.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeRef {
    /// A track of the enclosing mix.
    Track {
        /// ID of a track in the enclosing mix.
        id: String,
    },
    /// A bus declared in `routing.buses`.
    Bus {
        /// ID of a bus in `routing.buses`.
        id: String,
    },
    /// The final output node; it runs the mix's master `effects`.
    Output,
}
/// Channel layout declaration for one mix track.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackLayout {
    /// ID of a track in the enclosing mix; declare each track exactly once.
    pub id: String,
    /// Layout that every clip on the track must produce through its channel policy.
    pub layout: Layout,
}
/// Summing bus: incoming routes add, then gain, ordered effects and mute apply.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    /// Bus ID unique among buses; 1..128 bytes, not blank.
    pub id: String,
    /// Channel layout of the bus signal.
    pub layout: Layout,
    /// Linear gain, 1000 = unity, 0..4000; default 1000. `gain_curve` replaces it when set.
    #[serde(default = "unity")]
    pub gain_milli: u32,
    /// Silence the bus output after its effects; default false.
    #[serde(default)]
    pub mute: bool,
    /// Optional gain automation in milli-units 0..4000 on the mix clock (sample index / 48000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_curve: Option<Curve>,
    /// Ordered effects run after bus gain, at most 8; default empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}
/// Pan law: `linear` weights (1000-p)/2000 and (1000+p)/2000, so center is half amplitude per side; `equal_power` uses cosine/sine weights, about 0.707 per side at center.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PanLaw {
    Linear,
    EqualPower,
}
/// How a route maps source channels to destination channels, tagged by `type`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mapping {
    /// Copy channels unchanged; source and destination layouts must match exactly.
    Identity,
    /// Explicit weighted channel matrix, rounded once per destination channel.
    Matrix {
        /// One row per destination channel, one column per source channel; milli-unit weights -4000..4000 (1000 = unity, negative inverts).
        coefficients_milli: Vec<Vec<i32>>,
    },
    /// Pan a mono source into a stereo destination.
    Pan {
        /// Pan law used for the left/right weights.
        law: PanLaw,
        /// Position from -1000 (left) to 1000 (right); `curve` replaces it when set.
        position_milli: i32,
        /// Optional position automation, -1000..1000, on the mix clock (sample index / 48000).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
    },
    /// Stereo-to-stereo balance; center keeps both channels at unity, movement attenuates the opposite side.
    Balance {
        /// Position from -1000 (left) to 1000 (right); `curve` replaces it when set.
        position_milli: i32,
        /// Optional position automation, -1000..1000, on the mix clock (sample index / 48000).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        curve: Option<Curve>,
    },
}
/// Directed connection from a track or bus to a bus or the output; parallel routes add.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// Route ID unique within the routing; 1..128 bytes, not blank.
    pub id: String,
    /// Track or bus that feeds the route.
    pub source: NodeRef,
    /// Bus or output that receives the route.
    pub destination: NodeRef,
    /// Channel mapping applied to the signal.
    pub mapping: Mapping,
}
/// Optional mix routing graph with named layouts, buses and channel routes.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Routing {
    /// Layout of the final rendered WAV.
    pub output: Layout,
    /// Exactly one layout declaration for every mix track.
    pub tracks: Vec<TrackLayout>,
    /// Up to 16 buses.
    pub buses: Vec<Bus>,
    /// Up to 64 routes; every track and bus must reach the output without cycles, through at most 16 buses.
    pub routes: Vec<Route>,
}
enum Weights {
    Matrix(Vec<(usize, usize, i32)>),
    Pan {
        law: PanLaw,
        position: i32,
        curve: Option<Sampler>,
    },
    Balance {
        position: i32,
        curve: Option<Sampler>,
    },
}
struct Connection {
    from: usize,
    to: usize,
    weights: Weights,
    work: u64,
}
impl Connection {
    fn apply(&self, source: &[i64], destination: &mut [i64], time: Time) -> Result<()> {
        let mut contribution = [0i64; 8];
        match &self.weights {
            Weights::Matrix(entries) => {
                let mut sums = [0i128; 8];
                for (row, col, value) in entries {
                    sums[*row] += source[*col] as i128 * *value as i128;
                }
                for (d, s) in contribution.iter_mut().zip(sums) {
                    *d = audio::rounded(s, 1000);
                }
            }
            Weights::Pan {
                law,
                position,
                curve,
            } => {
                let p = curve
                    .as_ref()
                    .map(|c| c.sample(time))
                    .transpose()?
                    .unwrap_or(*position);
                match law {
                    PanLaw::Linear => {
                        contribution[0] =
                            audio::rounded(source[0] as i128 * (1000 - p) as i128, 2000);
                        contribution[1] =
                            audio::rounded(source[0] as i128 * (1000 + p) as i128, 2000);
                    }
                    PanLaw::EqualPower => {
                        let weights = match p {
                            -1000 => [1.0, 0.0],
                            1000 => [0.0, 1.0],
                            _ => {
                                let theta = std::f64::consts::PI * (p + 1000) as f64 / 4000.0;
                                [theta.cos(), theta.sin()]
                            }
                        };
                        for c in 0..2 {
                            contribution[c] = (source[0] as f64 * weights[c]).round() as i64;
                        }
                    }
                }
            }
            Weights::Balance { position, curve } => {
                let p = curve
                    .as_ref()
                    .map(|c| c.sample(time))
                    .transpose()?
                    .unwrap_or(*position);
                contribution[0] =
                    audio::rounded(source[0] as i128 * (1000 - p.max(0)) as i128, 1000);
                contribution[1] =
                    audio::rounded(source[1] as i128 * (1000 + p.min(0)) as i128, 1000);
            }
        }
        for (d, v) in destination.iter_mut().zip(contribution) {
            *d = d
                .checked_add(v)
                .ok_or_else(|| error("AUDIO_ROUTING_OVERFLOW", "Routing accumulator overflow"))?;
        }
        Ok(())
    }
}
struct Node {
    key: NodeRef,
    layout: Layout,
    gain: u32,
    mute: bool,
    curve: Option<Sampler>,
    processor: Processor,
    effect_count: usize,
}
struct Graph {
    nodes: Vec<Node>,
    routes: Vec<Connection>,
    order: Vec<usize>,
    outgoing: Vec<Vec<usize>>,
    tracks: HashMap<String, usize>,
    output: usize,
    work: u64,
}
impl Graph {
    fn new(mix: &Mix, routing: &Routing, samples: u64) -> Result<Self> {
        if routing.tracks.len() != mix.tracks.len()
            || routing.buses.len() > 16
            || routing.routes.len() > 64
        {
            return Err(invalid(
                "Declare every track layout, at most 16 buses and 64 routes",
            ));
        }
        let mut definitions = BTreeMap::new();
        let track_ids = mix
            .tracks
            .iter()
            .map(|t| t.id.as_str())
            .collect::<HashSet<_>>();
        for track in &routing.tracks {
            let key = NodeRef::Track {
                id: track.id.clone(),
            };
            if !track_ids.contains(track.id.as_str())
                || definitions
                    .insert(key.clone(), (track.layout, 1000, false, None, Vec::new()))
                    .is_some()
            {
                return Err(invalid(
                    "Track layout declarations must exactly match unique mix track IDs",
                ));
            }
        }
        for bus in &routing.buses {
            if !audio::id_ok(&bus.id)
                || bus.gain_milli > 4000
                || definitions
                    .insert(
                        NodeRef::Bus { id: bus.id.clone() },
                        (
                            bus.layout,
                            bus.gain_milli,
                            bus.mute,
                            bus.gain_curve.clone(),
                            bus.effects.clone(),
                        ),
                    )
                    .is_some()
            {
                return Err(invalid("Bus IDs must be unique and gains must be 0..4000"));
            }
        }
        definitions.insert(
            NodeRef::Output,
            (routing.output, 1000, false, None, mix.effects.clone()),
        );
        let mut lookup = BTreeMap::new();
        let mut nodes = Vec::new();
        let mut tracks = HashMap::new();
        for (key, (layout, gain, mute, curve, effects)) in definitions {
            let index = nodes.len();
            if let NodeRef::Track { id } = &key {
                tracks.insert(id.clone(), index);
            }
            lookup.insert(key.clone(), index);
            nodes.push(Node {
                key,
                layout,
                gain,
                mute,
                curve: curve
                    .as_ref()
                    .map(|c| c.prepare(mix.duration, 0, 4000))
                    .transpose()?,
                processor: Processor::new(&effects, layout.channels())?,
                effect_count: effects.len(),
            });
        }
        let output = lookup[&NodeRef::Output];
        let mut authored = routing.routes.iter().collect::<Vec<_>>();
        authored.sort_by(|a, b| a.id.cmp(&b.id));
        let mut ids = HashSet::new();
        let mut routes = Vec::new();
        let mut incoming = vec![0usize; nodes.len()];
        let mut outgoing = vec![0usize; nodes.len()];
        for route in authored {
            if !audio::id_ok(&route.id)
                || !ids.insert(&route.id)
                || matches!(route.source, NodeRef::Output)
                || matches!(route.destination, NodeRef::Track { .. })
            {
                return Err(invalid(
                    "Routes need unique IDs and must flow from tracks/buses to buses/output",
                ));
            }
            let from = *lookup
                .get(&route.source)
                .ok_or_else(|| invalid("Route source does not exist"))?;
            let to = *lookup
                .get(&route.destination)
                .ok_or_else(|| invalid("Route destination does not exist"))?;
            let (a, b) = (nodes[from].layout, nodes[to].layout);
            let (weights, work) = match &route.mapping {
                Mapping::Identity if a == b => (
                    Weights::Matrix((0..a.channels()).map(|c| (c, c, 1000)).collect()),
                    a.channels() as u64,
                ),
                Mapping::Matrix { coefficients_milli }
                    if coefficients_milli.len() == b.channels()
                        && coefficients_milli.iter().all(|r| {
                            r.len() == a.channels() && r.iter().all(|v| (-4000..=4000).contains(v))
                        }) =>
                {
                    let entries = coefficients_milli
                        .iter()
                        .enumerate()
                        .flat_map(|(r, row)| {
                            row.iter()
                                .enumerate()
                                .filter(|(_, v)| **v != 0)
                                .map(move |(c, v)| (r, c, *v))
                        })
                        .collect::<Vec<_>>();
                    let work = entries.len() as u64;
                    (Weights::Matrix(entries), work)
                }
                Mapping::Pan {
                    law,
                    position_milli,
                    curve,
                } if a == Layout::Mono
                    && b == Layout::Stereo
                    && (-1000..=1000).contains(position_milli) =>
                {
                    (
                        Weights::Pan {
                            law: *law,
                            position: *position_milli,
                            curve: curve
                                .as_ref()
                                .map(|c| c.prepare(mix.duration, -1000, 1000))
                                .transpose()?,
                        },
                        64,
                    )
                }
                Mapping::Balance {
                    position_milli,
                    curve,
                } if a == Layout::Stereo
                    && b == Layout::Stereo
                    && (-1000..=1000).contains(position_milli) =>
                {
                    (
                        Weights::Balance {
                            position: *position_milli,
                            curve: curve
                                .as_ref()
                                .map(|c| c.prepare(mix.duration, -1000, 1000))
                                .transpose()?,
                        },
                        2,
                    )
                }
                _ => {
                    return Err(invalid(
                        "Mapping layout/dimensions, coefficients or pan position are unsupported",
                    ));
                }
            };
            incoming[to] += 1;
            outgoing[from] += 1;
            routes.push(Connection {
                from,
                to,
                weights,
                work: work + b.channels() as u64 + 8,
            });
        }
        for (i, node) in nodes.iter().enumerate() {
            if (i != output && outgoing[i] == 0)
                || (matches!(node.key, NodeRef::Bus { .. }) && incoming[i] == 0)
            {
                return Err(invalid(
                    "Every track and bus must participate in a route to output",
                ));
            }
        }
        let mut order = Vec::new();
        let mut used = vec![false; nodes.len()];
        while let Some(i) = (0..nodes.len()).find(|i| !used[*i] && incoming[*i] == 0) {
            used[i] = true;
            order.push(i);
            for route in routes.iter().filter(|r| r.from == i) {
                incoming[route.to] -= 1;
            }
        }
        if order.len() != nodes.len() {
            return Err(invalid("Routing cycles are unsupported"));
        }
        let mut depth = vec![0usize; nodes.len()];
        for i in &order {
            for r in routes.iter().filter(|r| r.from == *i) {
                depth[r.to] = depth[r.to].max(depth[*i] + 1);
            }
        }
        if depth[output] > 17 {
            return Err(invalid("Routing depth exceeds 16 buses"));
        }
        let work = samples
            * (routes.iter().map(|r| r.work).sum::<u64>()
                + nodes
                    .iter()
                    .map(|n| n.layout.channels() as u64 * (1 + n.effect_count as u64 * 32))
                    .sum::<u64>())
            + mix
                .tracks
                .iter()
                .map(|t| {
                    t.clips
                        .iter()
                        .map(|c| {
                            c.duration
                                .units(Time { num: 48000, den: 1 })
                                .expect("validated clip")
                                * nodes[tracks[&t.id]].layout.channels() as u64
                        })
                        .sum::<u64>()
                })
                .sum::<u64>();
        if work > WORK {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Audio routing exceeds 512M weighted sample operations",
            ));
        }
        let mut outgoing = vec![Vec::new(); nodes.len()];
        for (index, route) in routes.iter().enumerate() {
            outgoing[route.from].push(index);
        }
        Ok(Self {
            nodes,
            routes,
            order,
            outgoing,
            tracks,
            output,
            work,
        })
    }
}

pub(crate) fn prepare(mix: &Mix, routing: &Routing, root: &Path) -> Result<audio::Prepared> {
    let count = audio::validate(mix, root)?;
    let mut graph = Graph::new(mix, routing, count)?;
    let mut identities = HashMap::<PathBuf, Identity>::new();
    let mut decoded = Vec::<(audio::Source, Layout)>::new();
    let mut lookup = HashMap::new();
    let mut sources = Vec::new();
    let mut bytes_total = 0u64;
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
                let source = pcm_wave::decode(&bytes)?;
                lookup.insert(clip.file.path.clone(), decoded.len());
                decoded.push((
                    audio::Source {
                        data: source.data,
                        rate: source.rate,
                        channels: source.layout.channels() as u16,
                    },
                    source.layout,
                ));
                identities.insert(clip.file.path.clone(), clip.file.clone());
                sources.push((path, clip.file.clone()));
            }
        }
    }
    let mut voices = Vec::new();
    let mut reports = Vec::new();
    for track in &mix.tracks {
        let node = graph.tracks[&track.id];
        for clip in &track.clips {
            let index = lookup[&clip.file.path];
            let (source, layout) = &decoded[index];
            let output_layout = match clip.channels {
                audio::Channels::DuplicateMono if *layout == Layout::Mono => Layout::Stereo,
                audio::Channels::PreserveStereo if *layout == Layout::Stereo => Layout::Stereo,
                audio::Channels::PreserveMono if *layout == Layout::Mono => Layout::Mono,
                audio::Channels::PreserveLayout => *layout,
                _ => {
                    return Err(error(
                        "UNSUPPORTED_AUDIO",
                        "Channel policy differs from the declared source layout",
                    ));
                }
            };
            if output_layout != graph.nodes[node].layout {
                return Err(invalid("Clip channel policy must match its track layout"));
            }
            let voice = Voice::new(clip, track, source)?;
            let mut report = voice.report(source);
            report["source_layout"] = json!(layout);
            report["track_layout"] = json!(output_layout);
            reports.push(report);
            voices.push((node, index, voice));
        }
    }
    let channels = routing.output.channels();
    let mut pcm = Vec::with_capacity(count as usize * channels);
    let mut clipped = vec![0u64; channels];
    let mut peak = vec![0i32; channels];
    let mut hash = Sha256::new();
    // One sample frame per graph node keeps live graph memory independent of duration.
    let mut values = vec![[0i64; 8]; graph.nodes.len()];
    let mut starts = (0..voices.len()).collect::<Vec<_>>();
    starts.sort_by_key(|i| (voices[*i].2.start, *i));
    let mut next = 0usize;
    let mut active = Vec::new();
    for sample in 0..count {
        while next < starts.len() && voices[starts[next]].2.start == sample {
            active.push(starts[next]);
            next += 1;
        }
        active.retain(|i| voices[*i].2.start + voices[*i].2.samples > sample);
        values.fill([0; 8]);
        let time = Time::new(sample, 48000)?;
        for voice_index in &active {
            let (node, index, voice) = &voices[*voice_index];
            let local = sample - voice.start;
            let (weight, denominator) = voice.weight(local)?;
            let source = &decoded[*index].0;
            for (c, value) in values[*node][..graph.nodes[*node].layout.channels()]
                .iter_mut()
                .enumerate()
            {
                *value += audio::rounded(
                    source.sample(voice.source_in, local, c) as i128 * weight,
                    denominator,
                );
            }
        }
        for index in &graph.order {
            let node = &mut graph.nodes[*index];
            let samples = &mut values[*index][..node.layout.channels()];
            bounded(samples)?;
            let gain = node
                .curve
                .as_ref()
                .map(|c| c.sample(time))
                .transpose()?
                .unwrap_or(node.gain as i32);
            for value in samples.iter_mut() {
                *value = audio::rounded(*value as i128 * gain as i128, 1000);
            }
            bounded(samples)?;
            node.processor.frame(samples)?;
            if node.mute {
                samples.fill(0);
            }
            bounded(samples)?;
            let source = values[*index];
            for route_index in &graph.outgoing[*index] {
                let route = &graph.routes[*route_index];
                let channels = graph.nodes[route.to].layout.channels();
                route.apply(&source, &mut values[route.to][..channels], time)?;
            }
        }
        for (c, v) in values[graph.output][..channels].iter().enumerate() {
            if !(-32768..=32767).contains(v) {
                clipped[c] += 1;
            }
            let v = (*v).clamp(-32768, 32767) as i16;
            peak[c] = peak[c].max((v as i32).abs());
            hash.update(v.to_le_bytes());
            pcm.push(v);
        }
    }
    for (_, identity) in &sources {
        scene::identity_bytes(identity, root)?;
    }
    let report = json!({"profile":"pcm-routing-v1","mix_id":mix.id,"sample_rate":48000,"channels":channels,"layout":routing.output,"speakers":routing.output.speakers(),"channel_mask":routing.output.mask(),"samples":count,"duration":mix.duration,"effects":mix.effects,"routing":routing,"node_order":graph.order.iter().map(|i|&graph.nodes[*i].key).collect::<Vec<_>>(),"weighted_sample_operations":graph.work,"graph_live_values":values.len()*8,"meters":crate::audio_processing::channel_meters(&pcm,channels),"clips":reports,"peak_absolute":peak,"clipped_samples":clipped,"pcm_sha256":format!("{:x}",hash.finalize()),"resampling":"linear-nearest-ties-away","gain_unit":"linear_milli","rounding":"voice_then_route_or_bus_nearest_ties_away","clipping":"final_saturate_i16","sources":sources.iter().map(|(path,identity)|json!({"path":path,"identity":identity})).collect::<Vec<_>>()});
    Ok(audio::Prepared {
        pcm,
        layout: routing.output,
        sources,
        report,
    })
}
pub(crate) fn publish(
    mix: &Mix,
    prepared: audio::Prepared,
    root: &Path,
    output: &Path,
) -> Result<Value> {
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let temp = scratch.0.join("output.wav");
    pcm_wave::write(&temp, prepared.layout, &prepared.pcm)?;
    let bytes = fs::read(&temp)?;
    let observed = pcm_wave::decode(&bytes)?;
    if observed.layout != prepared.layout || observed.rate != 48000 || observed.data != prepared.pcm
    {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Routed WAV layout or samples differ",
        ));
    }
    for (_, identity) in &prepared.sources {
        scene::identity_bytes(identity, root)?;
    }
    let mut report = prepared.report;
    report["output"] = json!(output);
    report["sha256"] = json!(media::file_hash(&temp)?);
    report["mix_sha256"] = json!(format!("{:x}", Sha256::digest(serde_json::to_vec(mix)?)));
    media::publish(&temp, output)?;
    Ok(report)
}
pub fn capabilities() -> Value {
    json!({"profile":"pcm-routing-v1","layouts":["mono","stereo","quad","5.1","5.1(side)","7.1"],"maximum_buses":16,"maximum_routes":64,"maximum_weighted_sample_operations":WORK,"intermediate_absolute_pcm_limit":HEADROOM,"matrix_milli_range":[-4000,4000],"pan_position_milli":[-1000,1000],"pan_laws":["linear","equal_power"],"stereo_balance":true,"pan_and_bus_gain_automation":true,"bus_effects":true,"master_effects":true,"extensible_pcm16":true,"scene_output":"explicit_stereo","surround_loudness":false})
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
            std::env::temp_dir().join(format!("cutbolt-routing-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let input = root.join("source.wav");
        pcm_wave::write(&input, Layout::Stereo, &[1000, -1000, 2000, -2000]).unwrap();
        let original = fs::read(&input).unwrap();
        let mix: Mix = serde_json::from_value(json!({"schema_version":1,"id":"publication-fixture","duration":{"num":1,"den":24000},
            "tracks":[{"id":"voice","clips":[{"id":"clip","file":{"path":"source.wav","bytes":original.len(),"sha256":media::file_hash(&input).unwrap()},"channels":"preserve_layout","start":{"num":0,"den":1},"source_in":{"num":0,"den":1},"duration":{"num":1,"den":24000}}]}],
            "routing":{"output":"stereo","tracks":[{"id":"voice","layout":"stereo"}],"buses":[],"routes":[{"id":"direct","source":{"kind":"track","id":"voice"},"destination":{"kind":"output"},"mapping":{"type":"identity"}}]}})).unwrap();
        let prepared = prepare(&mix, mix.routing.as_ref().unwrap(), &root).unwrap();
        fs::write(&input, b"changed original fixture").unwrap();
        let destination = root.join("result.wav");
        assert_eq!(
            publish(&mix, prepared, &root, &destination)
                .unwrap_err()
                .code,
            "MEDIA_CHANGED"
        );
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::write(&input, &original).unwrap();
        let sentinel = root.join("sentinel.wav");
        fs::write(&sentinel, b"existing output").unwrap();
        assert_eq!(
            audio::run(&mix, &root, &root, &sentinel).unwrap_err().code,
            "OUTPUT_EXISTS"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"existing output");
        assert_eq!(fs::read(&input).unwrap(), original);
        fs::remove_file(input).unwrap();
        fs::remove_file(sentinel).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
