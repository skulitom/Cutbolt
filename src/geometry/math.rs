//! Original bounded affine geometry, using column vectors and right-handed axes.
use crate::{Result, error};
pub(super) type V = [f64; 3];
pub(super) fn add(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(super) fn sub(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(super) fn mul(a: V, b: f64) -> V {
    a.map(|v| v * b)
}
pub(super) fn dot(a: V, b: V) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(super) fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(super) fn unit(a: V) -> Result<V> {
    let largest = a.iter().map(|v| v.abs()).fold(0.0, f64::max);
    if !a.iter().all(|v| v.is_finite()) || largest == 0.0 {
        return Err(error(
            "INVALID_GEOMETRY",
            "A camera basis, plane normal or light direction is degenerate",
        ));
    }
    let scaled = mul(a, 1.0 / largest);
    Ok(mul(scaled, 1.0 / dot(scaled, scaled).sqrt()))
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(super) struct Matrix(pub [[f64; 4]; 3]);
impl Matrix {
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);
    pub fn vector(self, v: V) -> V {
        std::array::from_fn(|i| self.0[i][0] * v[0] + self.0[i][1] * v[1] + self.0[i][2] * v[2])
    }
    pub fn point(self, v: V) -> V {
        add(self.vector(v), std::array::from_fn(|i| self.0[i][3]))
    }
    pub fn then(self, rhs: Self) -> Self {
        let mut result = [[0.0; 4]; 3];
        for (i, row) in result.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| self.0[i][k] * rhs.0[k][j]).sum::<f64>()
                    + if j == 3 { self.0[i][3] } else { 0.0 };
            }
        }
        Self(result)
    }
    pub fn bounded(self) -> Result<Self> {
        if self
            .0
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1e9)
        {
            Err(error(
                "GEOMETRY_PRECISION",
                "World and inverse affine coefficients must be finite and at most 1e9 in magnitude",
            ))
        } else {
            Ok(self)
        }
    }
    pub fn inverse(self) -> Result<Self> {
        let a = std::array::from_fn(|i| self.0[i][0]);
        let b = std::array::from_fn(|i| self.0[i][1]);
        let c = std::array::from_fn(|i| self.0[i][2]);
        let det = dot(a, cross(b, c));
        if !det.is_finite() || det.abs() < 1e-18 {
            return Err(error(
                "GEOMETRY_PRECISION",
                "Affine transform is singular or below the supported determinant bound",
            ));
        }
        let rows = [
            mul(cross(b, c), 1.0 / det),
            mul(cross(c, a), 1.0 / det),
            mul(cross(a, b), 1.0 / det),
        ];
        let translation = std::array::from_fn(|i| self.0[i][3]);
        Self(std::array::from_fn(|i| {
            [
                rows[i][0],
                rows[i][1],
                rows[i][2],
                -dot(rows[i], translation),
            ]
        }))
        .bounded()
    }
}
fn trig(angle: i32) -> (f64, f64) {
    let angle = angle.rem_euclid(360000);
    if angle % 90000 == 0 {
        [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)][(angle / 90000) as usize]
    } else {
        (angle as f64 * std::f64::consts::PI / 180000.0).sin_cos()
    }
}
pub(super) fn transform(position: [i32; 3], rotation: [i32; 3], scale: [i32; 3]) -> Matrix {
    let [x, y, z] = rotation.map(trig);
    let rx = Matrix([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, x.1, -x.0, 0.0],
        [0.0, x.0, x.1, 0.0],
    ]);
    let ry = Matrix([
        [y.1, 0.0, y.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [-y.0, 0.0, y.1, 0.0],
    ]);
    let rz = Matrix([
        [z.1, -z.0, 0.0, 0.0],
        [z.0, z.1, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);
    let mut s = Matrix::IDENTITY;
    for (i, v) in scale.into_iter().enumerate() {
        s.0[i][i] = v as f64 / 1000.0;
    }
    let mut result = rz.then(ry).then(rx).then(s);
    for (i, v) in position.into_iter().enumerate() {
        result.0[i][3] = v as f64 / 1000.0;
    }
    result
}
