use crate::{
    Result,
    animation::{Curve, Sampler},
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Curves {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Curve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Curve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<Curve>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Vector {
    pub value: [i32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Curves>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scalar {
    pub value: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Curve>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    pub position_milli: Vector,
    pub rotation_mdeg: Vector,
    pub scale_milli: Vector,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plane {
    pub layer: String,
    pub size_milli: [u32; 2],
    pub material: String,
    pub double_sided: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<Plane>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Projection {
    Perspective { vertical_fov_mdeg: Scalar },
    Orthographic { vertical_size_milli: Scalar },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub position_milli: Vector,
    pub target_milli: Vector,
    pub up_milli: Vector,
    pub projection: Projection,
    pub near_milli: u32,
    pub far_milli: u32,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    Constant,
    InverseSquare,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Light {
    Ambient {
        color: [u8; 3],
        intensity_milli: Scalar,
    },
    Directional {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        color: [u8; 3],
        intensity_milli: Scalar,
        toward_light_milli: Vector,
    },
    Point {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        color: [u8; 3],
        intensity_milli: Scalar,
        position_milli: Vector,
        falloff: Falloff,
        reference_distance_milli: u32,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub nodes: Vec<Node>,
    pub camera: Camera,
    pub lights: Vec<Light>,
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
        if self.value.iter().any(|v| !(min..=max).contains(v)) {
            return Err(error(
                "INVALID_GEOMETRY",
                "Vector value exceeds its declared geometric bounds",
            ));
        }
        let curves = if let Some(c) = &self.animation {
            if c.x.is_none() && c.y.is_none() && c.z.is_none() {
                return Err(error(
                    "INVALID_GEOMETRY",
                    "Vector animation must contain a curve",
                ));
            }
            [&c.x, &c.y, &c.z].map(|curve| {
                curve
                    .as_ref()
                    .map(|curve| curve.prepare(duration, min, max))
                    .transpose()
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
                "Scalar exceeds its declared geometric bounds",
            ));
        }
        Ok(ScalarSampler {
            value: self.value,
            curve: self
                .animation
                .as_ref()
                .map(|c| c.prepare(duration, min, max))
                .transpose()?,
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
