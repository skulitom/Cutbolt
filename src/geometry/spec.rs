use crate::{
    At, Result,
    animation::{Curve, Sampler},
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};

/// Per-component animation curves for a geometry vector; at least one curve is required.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Curves {
    /// Curve for the x component; omit to hold the static x value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Curve>,
    /// Curve for the y component; omit to hold the static y value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Curve>,
    /// Curve for the z component; omit to hold the static z value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<Curve>,
}
/// Optionally animated 3D integer vector; unit and bounds come from the owning field.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vector {
    /// Static `[x, y, z]` value in the owning field's unit and bounds.
    pub value: [i32; 3],
    /// Optional per-component curves in scene-global time; curve values obey the same bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Curves>,
}
/// Optionally animated integer scalar; unit and bounds come from the owning field.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scalar {
    /// Static value in the owning field's unit and bounds.
    pub value: i32,
    /// Optional curve in scene-global time; replaces the static value and obeys the same bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Curve>,
}
/// Local node transform: scale, then x/y/z rotations, then translation; parents multiply the result.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Translation in thousandths of a world unit, each component -1000000..=1000000.
    pub position_milli: Vector,
    /// Rotations about x, y and z in millidegrees, each component -360000..=360000.
    pub rotation_mdeg: Vector,
    /// Scale factors in thousandths (1000 = 1.0), each component 1..=100000.
    pub scale_milli: Vector,
}
/// Textured rectangle centered on its node origin with front normal +Z.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plane {
    /// ID of the scene layer used as texture; every scene layer must be bound to exactly one plane.
    pub layer: String,
    /// Plane `[width, height]` in thousandths of a world unit, each 1..=1000000.
    pub size_milli: [u32; 2],
    /// Shading: `unlit` (texture unchanged) or `lambert` (encoded-color diffuse lighting).
    pub material: String,
    /// Draw the back face with its lighting normal flipped toward the viewer; false hides it.
    pub double_sided: bool,
}
/// Scene-graph node with a local transform and an optional textured plane.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Unique node ID: non-empty, at most 128 bytes, no control characters; ascending ID breaks depth ties.
    pub id: String,
    /// ID of the parent node; omit for a root node. Cycles reject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Local transform relative to the parent.
    pub transform: Transform,
    /// Textured plane drawn at this node; omit for a grouping-only node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<Plane>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
/// Camera projection, tagged by `kind`.
pub enum Projection {
    /// Perspective projection.
    Perspective {
        /// Vertical field of view in millidegrees, 1000..=170000.
        vertical_fov_mdeg: Scalar,
    },
    /// Orthographic projection.
    Orthographic {
        /// Visible vertical extent in thousandths of a world unit, 1..=2000000.
        vertical_size_milli: Scalar,
    },
}
/// Look-at camera for the geometry scene.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Node ID whose world transform maps position/target as points and up as a direction; omit for world space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Eye position in thousandths of a world unit, each component -1000000..=1000000.
    pub position_milli: Vector,
    /// Look-at point in thousandths of a world unit, each component -1000000..=1000000.
    pub target_milli: Vector,
    /// Up direction in thousandths, each component -1000..=1000; must not be degenerate.
    pub up_milli: Vector,
    /// Perspective or orthographic projection; not scaled by the parent.
    pub projection: Projection,
    /// Inclusive near clip depth along camera forward, in thousandths; greater than 0.
    pub near_milli: u32,
    /// Exclusive far clip depth in thousandths; greater than near and at most 2000000.
    pub far_milli: u32,
}
/// Point-light falloff: `constant` (no attenuation) or `inverse_square` (reference distance squared over distance squared).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    Constant,
    InverseSquare,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
/// Light for `lambert` planes, tagged by `kind`; gains sum and may exceed 1.0.
pub enum Light {
    /// Adds an unmodulated color gain to every lit sample.
    Ambient {
        /// RGB light color as `[r, g, b]` bytes.
        color: [u8; 3],
        /// Gain in thousandths (1000 = 1.0), 0..=4000.
        intensity_milli: Scalar,
    },
    /// Distant light adding a Lambert diffuse gain from one direction.
    Directional {
        /// Node ID whose world transform rotates the direction; omit for world space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// RGB light color as `[r, g, b]` bytes.
        color: [u8; 3],
        /// Gain in thousandths (1000 = 1.0), 0..=4000.
        intensity_milli: Scalar,
        /// Direction from surface toward the light in thousandths, each component -1000..=1000; normalized.
        toward_light_milli: Vector,
    },
    /// Positional light adding a Lambert diffuse gain with optional distance falloff.
    Point {
        /// Node ID whose world transform maps the position; omit for world space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        /// RGB light color as `[r, g, b]` bytes.
        color: [u8; 3],
        /// Gain in thousandths (1000 = 1.0), 0..=4000.
        intensity_milli: Scalar,
        /// Light position in thousandths of a world unit, each component -1000000..=1000000.
        position_milli: Vector,
        /// Distance attenuation mode.
        falloff: Falloff,
        /// Distance of full gain for `inverse_square` falloff, in thousandths, 1..=1000000.
        reference_distance_milli: u32,
    },
}
/// Optional scene `geometry` (`textured-planes-v1`): layers drawn as 3D textured planes through a camera.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    /// Scene-graph nodes, 1..=32, parent depth at most 16; list order does not set depth order.
    pub nodes: Vec<Node>,
    /// The single scene camera.
    pub camera: Camera,
    /// Up to eight lights; only `lambert` planes use them.
    pub lights: Vec<Light>,
    /// Shadow mode; only `none` is supported.
    pub shadows: String,
}

pub(super) struct VectorSampler {
    value: [i32; 3],
    curves: [Option<Sampler>; 3],
}
pub(super) struct ScalarSampler {
    value: i32,
    curve: Option<Sampler>,
}
impl Vector {
    pub(super) fn prepare(&self, duration: Time, min: i32, max: i32) -> Result<VectorSampler> {
        if let Some((i, v)) = self
            .value
            .iter()
            .enumerate()
            .find(|(_, v)| !(min..=max).contains(*v))
        {
            return Err(error(
                "INVALID_GEOMETRY",
                format!(
                    "value[{i}]: {v} is outside {min}..{max}; vector value exceeds its declared geometric bounds"
                ),
            ));
        }
        let curves = if let Some(c) = &self.animation {
            if c.x.is_none() && c.y.is_none() && c.z.is_none() {
                return Err(error(
                    "INVALID_GEOMETRY",
                    "Vector animation must contain a curve",
                ));
            }
            [(&c.x, "x"), (&c.y, "y"), (&c.z, "z")].map(|(curve, axis)| {
                curve
                    .as_ref()
                    .map(|curve| curve.prepare(duration, min, max))
                    .transpose()
                    .under(|| format!("animation.{axis}"))
            })
        } else {
            [Ok(None), Ok(None), Ok(None)]
        };
        let [x, y, z] = curves;
        Ok(VectorSampler {
            value: self.value,
            curves: [x?, y?, z?],
        })
    }
}
impl VectorSampler {
    pub fn sample(&self, t: Time) -> Result<[i32; 3]> {
        let mut v = self.value;
        for (i, c) in self.curves.iter().enumerate() {
            if let Some(c) = c {
                v[i] = c.sample(t)?;
            }
        }
        Ok(v)
    }
}
impl Scalar {
    pub(super) fn prepare(&self, duration: Time, min: i32, max: i32) -> Result<ScalarSampler> {
        if !(min..=max).contains(&self.value) {
            return Err(error(
                "INVALID_GEOMETRY",
                format!(
                    "value: {} is outside {min}..{max}; scalar exceeds its declared geometric bounds",
                    self.value
                ),
            ));
        }
        Ok(ScalarSampler {
            value: self.value,
            curve: self
                .animation
                .as_ref()
                .map(|c| c.prepare(duration, min, max))
                .transpose()
                .under(|| "animation".into())?,
        })
    }
}
impl ScalarSampler {
    pub fn sample(&self, t: Time) -> Result<i32> {
        self.curve
            .as_ref()
            .map(|c| c.sample(t))
            .transpose()
            .map(|v| v.unwrap_or(self.value))
    }
}
