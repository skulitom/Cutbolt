//! Original bounded textured-plane geometry and explicit encoded-color lighting.
use crate::{
    Result,
    composite::{self, AlphaMode, BlendMode},
    error,
    scene::Scene,
    time::Time,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
mod math;
mod spec;
use math::{Matrix, V, add, cross, dot, mul, sub, unit};
pub use spec::*;
use spec::{ScalarSampler, VectorSampler};

/// Camera rays tested against planes over the whole scene, pixels x planes x exposure samples:
/// the scene compositing budget. A visit measured about 40 ns on the development machine, half
/// an interpolated spatial pixel, so a full budget takes no longer than a full spatial scene.
pub(crate) const MAX_VISITS: u64 = crate::scene::MAX_COMPOSITED_PIXELS;
/// Node transforms sampled and reported, nodes x exposure samples. Each sample reports every
/// node's world matrix and every plane's state, about 0.8 KB per record, so the receipt stays
/// near 25 MB.
pub(crate) const MAX_NODE_RECORDS: u64 = 32_768;

fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_GEOMETRY", message)
}
fn unsupported(message: impl Into<String>) -> crate::Error {
    error("UNSUPPORTED_GEOMETRY", message)
}
fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn world_vector(value: [i32; 3]) -> V {
    value.map(|v| v as f64 / 1000.0)
}

struct TransformSampler {
    position: VectorSampler,
    rotation: VectorSampler,
    scale: VectorSampler,
}
impl TransformSampler {
    fn new(value: &Transform, duration: Time) -> Result<Self> {
        Ok(Self {
            position: value
                .position_milli
                .prepare(duration, -1_000_000, 1_000_000)?,
            rotation: value.rotation_mdeg.prepare(duration, -360000, 360000)?,
            scale: value.scale_milli.prepare(duration, 1, 100000)?,
        })
    }
    fn sample(&self, time: Time) -> Result<Matrix> {
        math::transform(
            self.position.sample(time)?,
            self.rotation.sample(time)?,
            self.scale.sample(time)?,
        )
        .bounded()
    }
}

#[derive(Serialize)]
struct CameraState {
    position: V,
    forward: V,
    right: V,
    up: V,
    perspective: bool,
    vertical_extent: f64,
    near: f64,
    far: f64,
}
struct CameraSampler {
    parent: Option<usize>,
    position: VectorSampler,
    target: VectorSampler,
    up: VectorSampler,
    perspective: bool,
    extent: ScalarSampler,
    near: f64,
    far: f64,
}
impl CameraSampler {
    fn sample(&self, time: Time, world: &[Matrix]) -> Result<CameraState> {
        let parent = self.parent.map(|i| world[i]).unwrap_or(Matrix::IDENTITY);
        let position = parent.point(world_vector(self.position.sample(time)?));
        let target = parent.point(world_vector(self.target.sample(time)?));
        let up = parent.vector(world_vector(self.up.sample(time)?));
        let forward = unit(sub(target, position))?;
        let right = unit(cross(forward, unit(up)?))?;
        let up = unit(cross(right, forward))?;
        let extent = self.extent.sample(time)? as f64;
        Ok(CameraState {
            position,
            forward,
            right,
            up,
            perspective: self.perspective,
            vertical_extent: if self.perspective {
                2.0 * (extent * std::f64::consts::PI / 360000.0).tan()
            } else {
                extent / 1000.0
            },
            near: self.near,
            far: self.far,
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LightState {
    Ambient {
        gain: V,
    },
    Directional {
        gain: V,
        toward_light: V,
    },
    Point {
        gain: V,
        position: V,
        falloff: Falloff,
        reference_distance: f64,
    },
}
enum LightSampler {
    Ambient {
        color: [u8; 3],
        intensity: ScalarSampler,
    },
    Directional {
        color: [u8; 3],
        intensity: ScalarSampler,
        parent: Option<usize>,
        direction: VectorSampler,
    },
    Point {
        color: [u8; 3],
        intensity: ScalarSampler,
        parent: Option<usize>,
        position: VectorSampler,
        falloff: Falloff,
        distance: f64,
    },
}
impl LightSampler {
    fn sample(&self, t: Time, world: &[Matrix]) -> Result<LightState> {
        let gain = |color: [u8; 3], intensity: &ScalarSampler| -> Result<V> {
            let value = intensity.sample(t)? as f64 / 1000.0;
            Ok(color.map(|c| value * c as f64 / 255.0))
        };
        let parent = |index: Option<usize>| index.map(|i| world[i]).unwrap_or(Matrix::IDENTITY);
        Ok(match self {
            Self::Ambient { color, intensity } => LightState::Ambient {
                gain: gain(*color, intensity)?,
            },
            Self::Directional {
                color,
                intensity,
                parent: p,
                direction,
            } => LightState::Directional {
                gain: gain(*color, intensity)?,
                toward_light: unit(parent(*p).vector(world_vector(direction.sample(t)?)))?,
            },
            Self::Point {
                color,
                intensity,
                parent: p,
                position,
                falloff,
                distance,
            } => LightState::Point {
                gain: gain(*color, intensity)?,
                position: parent(*p).point(world_vector(position.sample(t)?)),
                falloff: *falloff,
                reference_distance: *distance,
            },
        })
    }
}

#[derive(Serialize)]
struct PlaneState {
    id: String,
    layer: usize,
    inverse: Matrix,
    normal: V,
    size: [f64; 2],
    lambert: bool,
    double_sided: bool,
}
#[derive(Serialize)]
pub(crate) struct State {
    camera: CameraState,
    planes: Vec<PlaneState>,
    lights: Vec<LightState>,
}
pub(crate) struct Prepared {
    pub states: Vec<Option<State>>,
    pub report: Value,
}

pub(crate) fn prepare(scene: &Scene, times: &[Option<Time>]) -> Result<Option<Prepared>> {
    let Some(spec) = &scene.geometry else {
        return Ok(None);
    };
    if spec.shadows != "none" {
        return Err(unsupported(
            "Plane geometry supports shadows=none only; shadow maps and ray-traced shadows are not implemented",
        ));
    }
    if spec.nodes.is_empty() || spec.nodes.len() > 32 || spec.lights.len() > 8 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Geometry requires 1..32 nodes and at most eight lights",
        ));
    }
    let visits =
        scene.width as u64 * scene.height as u64 * scene.layers.len() as u64 * times.len() as u64;
    let records = times.len() as u64 * spec.nodes.len() as u64;
    if visits > MAX_VISITS || records > MAX_NODE_RECORDS {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!(
                "Geometry needs {visits} pixel-plane sample visits ({}x{} pixels x {} planes x {} samples) and {records} node sample records; the limits are {MAX_VISITS} and {MAX_NODE_RECORDS}. Use a smaller canvas, fewer planes, samples or nodes, a shorter scene, or split the scene",
                scene.width,
                scene.height,
                scene.layers.len(),
                times.len()
            ),
        ));
    }
    for layer in &scene.layers {
        let t = &layer.transform;
        if t.position != [0, 0]
            || t.scale != 1
            || t.quarter_turns != 0
            || t.spatial.is_some()
            || layer
                .animation
                .as_ref()
                .is_some_and(|a| a.position_x.is_some() || a.position_y.is_some())
            || layer.frames.iter().any(|f| f.anchor != [0, 0])
        {
            return Err(unsupported(
                "Plane geometry requires neutral 2D position/scale/rotation, no spatial transform or position curves, and zero frame anchors; use node transforms",
            ));
        }
    }
    if scene.expressions.as_ref().is_some_and(|p| {
        p.bindings
            .iter()
            .any(|b| b.property == crate::expressions::Property::Position)
    }) {
        return Err(unsupported(
            "Plane geometry accepts opacity expressions; position expressions are not a 3D transform",
        ));
    }
    let mut indices = BTreeMap::new();
    for (i, node) in spec.nodes.iter().enumerate() {
        if !label(&node.id) || indices.insert(node.id.clone(), i).is_some() {
            return Err(invalid("Geometry node IDs must be unique bounded labels"));
        }
    }
    let index = |name: &Option<String>| -> Result<Option<usize>> {
        name.as_ref()
            .map(|s| {
                indices
                    .get(s)
                    .copied()
                    .ok_or_else(|| invalid("Unknown geometry parent"))
            })
            .transpose()
    };
    let parents = spec
        .nodes
        .iter()
        .map(|n| index(&n.parent))
        .collect::<Result<Vec<_>>>()?;
    let mut order = Vec::new();
    let mut color = vec![0; spec.nodes.len()];
    fn visit(
        i: usize,
        parents: &[Option<usize>],
        color: &mut [u8],
        order: &mut Vec<usize>,
    ) -> Result<()> {
        if color[i] == 1 {
            return Err(error(
                "GEOMETRY_CYCLE",
                "Geometry parent links form a cycle",
            ));
        }
        if color[i] == 2 {
            return Ok(());
        }
        color[i] = 1;
        if let Some(p) = parents[i] {
            visit(p, parents, color, order)?;
        }
        color[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..spec.nodes.len() {
        visit(i, &parents, &mut color, &mut order)?;
    }
    let mut depths = vec![0; spec.nodes.len()];
    for &i in &order {
        depths[i] = 1 + parents[i].map(|p| depths[p]).unwrap_or(0);
        if depths[i] > 16 {
            return Err(error("LIMIT_EXCEEDED", "Geometry parent depth exceeds 16"));
        }
    }
    let layers: BTreeMap<_, _> = scene
        .layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id.clone(), i))
        .collect();
    let mut bound = BTreeSet::new();
    let mut planes = Vec::new();
    for (i, node) in spec.nodes.iter().enumerate() {
        if let Some(plane) = &node.plane {
            let layer = *layers
                .get(&plane.layer)
                .ok_or_else(|| invalid("Plane references an unknown scene layer"))?;
            if !bound.insert(layer) {
                return Err(invalid("A layer can bind to only one plane"));
            }
            if plane
                .size_milli
                .iter()
                .any(|v| !(1..=1_000_000).contains(v))
            {
                return Err(invalid("Plane dimensions must be 1..1000000 thousandths"));
            }
            if !matches!(plane.material.as_str(), "unlit" | "lambert") {
                return Err(unsupported(
                    "Only unlit and encoded-color Lambert plane materials are supported",
                ));
            }
            planes.push((i, layer, plane));
        }
    }
    if bound.len() != scene.layers.len() {
        return Err(invalid("Geometry must bind every scene layer exactly once"));
    }
    let transforms = spec
        .nodes
        .iter()
        .map(|n| TransformSampler::new(&n.transform, scene.duration))
        .collect::<Result<Vec<_>>>()?;
    let c = &spec.camera;
    if c.near_milli == 0 || c.far_milli <= c.near_milli || c.far_milli > 2_000_000 {
        return Err(invalid(
            "Camera requires 0 < near < far <=2000000 thousandths",
        ));
    }
    let (perspective, extent) = match &c.projection {
        Projection::Perspective { vertical_fov_mdeg } => (
            true,
            vertical_fov_mdeg.prepare(scene.duration, 1000, 170000)?,
        ),
        Projection::Orthographic {
            vertical_size_milli,
        } => (
            false,
            vertical_size_milli.prepare(scene.duration, 1, 2_000_000)?,
        ),
    };
    let camera = CameraSampler {
        parent: index(&c.parent)?,
        position: c
            .position_milli
            .prepare(scene.duration, -1_000_000, 1_000_000)?,
        target: c
            .target_milli
            .prepare(scene.duration, -1_000_000, 1_000_000)?,
        up: c.up_milli.prepare(scene.duration, -1000, 1000)?,
        perspective,
        extent,
        near: c.near_milli as f64 / 1000.0,
        far: c.far_milli as f64 / 1000.0,
    };
    let lights = spec
        .lights
        .iter()
        .map(|l| {
            Ok(match l {
                Light::Ambient {
                    color,
                    intensity_milli,
                } => LightSampler::Ambient {
                    color: *color,
                    intensity: intensity_milli.prepare(scene.duration, 0, 4000)?,
                },
                Light::Directional {
                    parent,
                    color,
                    intensity_milli,
                    toward_light_milli,
                } => LightSampler::Directional {
                    color: *color,
                    intensity: intensity_milli.prepare(scene.duration, 0, 4000)?,
                    parent: index(parent)?,
                    direction: toward_light_milli.prepare(scene.duration, -1000, 1000)?,
                },
                Light::Point {
                    parent,
                    color,
                    intensity_milli,
                    position_milli,
                    falloff,
                    reference_distance_milli,
                } => {
                    if !(1..=1_000_000).contains(reference_distance_milli) {
                        return Err(invalid(
                            "Point light reference distance must be 1..1000000 thousandths",
                        ));
                    }
                    LightSampler::Point {
                        color: *color,
                        intensity: intensity_milli.prepare(scene.duration, 0, 4000)?,
                        parent: index(parent)?,
                        position: position_milli.prepare(scene.duration, -1_000_000, 1_000_000)?,
                        falloff: *falloff,
                        distance: *reference_distance_milli as f64 / 1000.0,
                    }
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut states = Vec::new();
    let mut node_states = Vec::new();
    for time in times {
        let Some(time) = time else {
            states.push(None);
            node_states.push(None);
            continue;
        };
        let mut world = vec![Matrix::IDENTITY; spec.nodes.len()];
        for &i in &order {
            let parent = parents[i].map(|p| world[p]).unwrap_or(Matrix::IDENTITY);
            world[i] = parent.then(transforms[i].sample(*time)?).bounded()?;
            world[i].inverse()?;
        }
        let p = planes
            .iter()
            .map(|(node, layer, plane)| {
                Ok(PlaneState {
                    id: spec.nodes[*node].id.clone(),
                    layer: *layer,
                    inverse: world[*node].inverse()?,
                    normal: unit(cross(
                        world[*node].vector([1.0, 0.0, 0.0]),
                        world[*node].vector([0.0, 1.0, 0.0]),
                    ))?,
                    size: plane.size_milli.map(|v| v as f64 / 1000.0),
                    lambert: plane.material == "lambert",
                    double_sided: plane.double_sided,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let state = State {
            camera: camera.sample(*time, &world)?,
            planes: p,
            lights: lights
                .iter()
                .map(|l| l.sample(*time, &world))
                .collect::<Result<Vec<_>>>()?,
        };
        node_states.push(Some(
            spec.nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id.clone(), world[i]))
                .collect::<BTreeMap<_, _>>(),
        ));
        states.push(Some(state));
    }
    let report = json!({"profile":"textured-planes-v1","specification":spec,"sample_states":states,"node_world_matrices":node_states,
        "pixel_plane_sample_visits":visits,"node_sample_records":times.len()*spec.nodes.len(),
        "coordinates":"right_handed_x_right_y_up_z_toward_default_camera","sampling":"pixel_centres_nearest_held_texture",
        "clipping":"camera_forward_depth_near_inclusive_far_exclusive","depth":"per_pixel_far_to_near;equal_depth_ascending_node_id",
        "lighting":"encoded_rgb_lambert_gain_then_alpha_composite","transform_order":"parent * translation * Rz * Ry * Rx * scale",
        "shadows":"none","meshes":"planes_only","scene_parameters":"opacity_and_source_space_masks_effects;neutral_2d_geometry"});
    Ok(Some(Prepared { states, report }))
}

pub(crate) struct Texel {
    pub rgba: [u8; 4],
    pub alpha_mode: AlphaMode,
    pub opacity: u8,
    pub coverage: u32,
    pub blend: BlendMode,
}

fn lighting(lights: &[LightState], position: V, normal: V) -> V {
    let mut result = [0.0; 3];
    for light in lights {
        let (gain, factor) = match light {
            LightState::Ambient { gain } => (*gain, 1.0),
            LightState::Directional { gain, toward_light } => {
                (*gain, dot(normal, *toward_light).max(0.0))
            }
            LightState::Point {
                gain,
                position: source,
                falloff,
                reference_distance,
            } => {
                let delta = sub(*source, position);
                let distance_squared = dot(delta, delta);
                if distance_squared == 0.0 {
                    continue;
                }
                let angular = (dot(normal, delta) / distance_squared.sqrt()).max(0.0);
                (
                    *gain,
                    angular
                        * match falloff {
                            Falloff::Constant => 1.0,
                            Falloff::InverseSquare => {
                                reference_distance * reference_distance / distance_squared
                            }
                        },
                )
            }
        };
        for i in 0..3 {
            result[i] += gain[i] * factor;
        }
    }
    result
}

pub(crate) fn compose(
    scene: &Scene,
    state: &State,
    mut texture: impl FnMut(usize, u32, u32) -> Option<Texel>,
) -> Vec<u8> {
    let mut rgb = scene
        .background
        .repeat((scene.width * scene.height) as usize);
    let c = &state.camera;
    // A perspective camera casts every ray from its position, so each plane's local ray origin
    // is the same for the whole frame; the hit list is reused from pixel to pixel.
    let origins = state
        .planes
        .iter()
        .map(|plane| plane.inverse.point(c.position))
        .collect::<Vec<_>>();
    let mut hits = Vec::with_capacity(state.planes.len());
    for y in 0..scene.height {
        for x in 0..scene.width {
            let u = ((x as f64 + 0.5) / scene.width as f64 - 0.5)
                * c.vertical_extent
                * scene.width as f64
                / scene.height as f64;
            let v = (0.5 - (y as f64 + 0.5) / scene.height as f64) * c.vertical_extent;
            let offset = add(mul(c.right, u), mul(c.up, v));
            let origin = if c.perspective {
                c.position
            } else {
                add(c.position, offset)
            };
            let direction = if c.perspective {
                add(c.forward, offset)
            } else {
                c.forward
            };
            hits.clear();
            for (i, plane) in state.planes.iter().enumerate() {
                let facing = dot(plane.normal, direction);
                if !plane.double_sided && facing >= 0.0 {
                    continue;
                }
                let local_origin = if c.perspective {
                    origins[i]
                } else {
                    plane.inverse.point(origin)
                };
                let local_direction = plane.inverse.vector(direction);
                if local_direction[2] == 0.0 {
                    continue;
                }
                let depth = -local_origin[2] / local_direction[2];
                if !depth.is_finite() || depth < c.near || depth >= c.far {
                    continue;
                }
                let point = add(local_origin, mul(local_direction, depth));
                let u = point[0] / plane.size[0] + 0.5;
                let v = 0.5 - point[1] / plane.size[1];
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let canvas = scene.layers[plane.layer].canvas;
                let sx = (u * canvas[0] as f64).floor() as u32;
                let sy = (v * canvas[1] as f64).floor() as u32;
                if let Some(texel) =
                    texture(plane.layer, sx.min(canvas[0] - 1), sy.min(canvas[1] - 1))
                {
                    hits.push((i, depth, facing, texel));
                }
            }
            hits.sort_by(|a, b| {
                b.1.total_cmp(&a.1)
                    .then_with(|| state.planes[a.0].id.cmp(&state.planes[b.0].id))
            });
            for (i, depth, facing, mut texel) in hits.drain(..) {
                let plane = &state.planes[i];
                if plane.lambert && texel.rgba[3] > 0 {
                    let normal = if facing > 0.0 {
                        mul(plane.normal, -1.0)
                    } else {
                        plane.normal
                    };
                    let gain = lighting(&state.lights, add(origin, mul(direction, depth)), normal);
                    let alpha = if matches!(texel.alpha_mode, AlphaMode::Premultiplied) {
                        255.0 / texel.rgba[3] as f64
                    } else {
                        1.0
                    };
                    for (channel, gain) in gain.into_iter().enumerate() {
                        texel.rgba[channel] = (texel.rgba[channel] as f64 * alpha * gain)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                    texel.alpha_mode = AlphaMode::Straight;
                }
                let position = ((y * scene.width + x) * 3) as usize;
                for channel in 0..3 {
                    rgb[position + channel] = composite::masked_channel(
                        rgb[position + channel],
                        texel.rgba[channel],
                        texel.rgba[3],
                        texel.opacity,
                        texel.blend,
                        texel.alpha_mode,
                        texel.coverage,
                    );
                }
            }
        }
    }
    rgb
}

pub fn capabilities() -> Value {
    json!({"profile":"textured-planes-v1","maximum_nodes":32,"maximum_parent_depth":16,"maximum_planes":16,"maximum_lights":8,
        "maximum_pixel_plane_sample_visits":MAX_VISITS,"maximum_node_sample_records":MAX_NODE_RECORDS,"projections":["perspective","orthographic"],
        "materials":["unlit","lambert"],"lights":["ambient","directional","point"],"shadows":["none"],"time":"exact_rational",
        "geometry_numeric":"bounded_f64","textures":"nearest_held_images","depth":"per_pixel_back_to_front"})
}
