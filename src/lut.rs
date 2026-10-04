//! Original bounded .cube reader and numerical LUT interpolation.
use crate::{
    Result, error,
    scene::{self, Identity},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

/// LUT sampling: `nearest` (1D or 3D), `linear` (1D only), `trilinear` or `tetrahedral` (3D only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Nearest,
    Linear,
    Trilinear,
    Tetrahedral,
}
/// Identity-bound .cube LUT (one 1D or 3D table) applied to encoded working-transfer RGB.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Table file identity; `.cube` path relative to `input_root`, at most 8 MiB.
    pub file: Identity,
    /// Sampling method; must suit the table's dimensionality.
    pub interpolation: Interpolation,
}
pub struct Loaded {
    spec: Transform,
    title: Option<String>,
    dimensions: usize,
    size: usize,
    low: [f64; 3],
    high: [f64; 3],
    values: Vec<[f64; 3]>,
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_LUT", message)
}
fn number(text: &str) -> Result<f64> {
    let v: f64 = text
        .parse()
        .map_err(|_| invalid("LUT entries and domain bounds must be numbers"))?;
    if !v.is_finite() || !(-16.0..=16.0).contains(&v) {
        return Err(invalid("LUT numbers must be finite and within -16..16"));
    }
    Ok(v)
}
fn triple(words: &[&str]) -> Result<[f64; 3]> {
    if words.len() != 3 {
        return Err(invalid("Expected exactly three LUT channel values"));
    }
    Ok([number(words[0])?, number(words[1])?, number(words[2])?])
}
impl Transform {
    pub fn load(&self, root: &Path) -> Result<Loaded> {
        if self.file.bytes > 8 * 1024 * 1024
            || !self
                .file
                .path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("cube"))
        {
            return Err(invalid("Use an identity-bound .cube file of at most 8 MiB"));
        }
        let (_, bytes) = scene::identity_bytes(&self.file, root)?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("LUT must contain ASCII text"))?;
        if !text.is_ascii()
            || text
                .bytes()
                .any(|c| c.is_ascii_control() && !b"\n\r\t".contains(&c))
        {
            return Err(invalid(
                "LUT must contain printable ASCII, tabs and line endings",
            ));
        }
        let mut title = None;
        let mut low = [0.0; 3];
        let mut high = [1.0; 3];
        let mut dimensions = 0;
        let mut size = 0;
        let mut values = Vec::new();
        let mut headers = HashSet::new();
        for raw in text.lines() {
            if raw.len() > 4096 {
                return Err(invalid("LUT line exceeds 4096 bytes"));
            }
            let mut quoted = false;
            let end = raw
                .char_indices()
                .find_map(|(i, c)| {
                    if c == '"' {
                        quoted = !quoted;
                    }
                    if c == '#' && !quoted { Some(i) } else { None }
                })
                .unwrap_or(raw.len());
            let line = raw[..end].trim();
            if line.is_empty() {
                continue;
            }
            let words: Vec<_> = line.split_ascii_whitespace().collect();
            let key = words[0];
            if [
                "TITLE",
                "LUT_1D_SIZE",
                "LUT_3D_SIZE",
                "DOMAIN_MIN",
                "DOMAIN_MAX",
            ]
            .contains(&key)
            {
                if !values.is_empty() || !headers.insert(key) {
                    return Err(invalid("LUT headers must be unique and precede data"));
                }
                match key {
                    "TITLE" => {
                        let value = line[5..].trim();
                        if value.len() < 2
                            || !value.starts_with('"')
                            || !value.ends_with('"')
                            || value[1..value.len() - 1].contains('"')
                            || value.len() > 258
                        {
                            return Err(invalid(
                                "TITLE must be a quoted ASCII string of at most 256 bytes",
                            ));
                        }
                        title = Some(value[1..value.len() - 1].to_owned());
                    }
                    "DOMAIN_MIN" => low = triple(&words[1..])?,
                    "DOMAIN_MAX" => high = triple(&words[1..])?,
                    _ => {
                        if dimensions != 0
                            || words.len() != 2
                            || !words[1].bytes().all(|c| c.is_ascii_digit())
                        {
                            return Err(invalid(
                                "Exactly one 1D or 3D size declaration is required",
                            ));
                        }
                        dimensions = if key == "LUT_1D_SIZE" { 1 } else { 3 };
                        size = words[1]
                            .parse::<usize>()
                            .map_err(|_| invalid("Invalid LUT size"))?;
                        if size < 2 || size > if dimensions == 1 { 65536 } else { 65 } {
                            return Err(invalid(
                                "1D LUTs require 2..65536 entries; 3D LUTs require size 2..65",
                            ));
                        }
                    }
                }
            } else {
                if dimensions == 0 {
                    return Err(invalid(
                        "LUT size must precede data; unsupported headers are rejected",
                    ));
                }
                if values.len() >= size.pow(dimensions as u32) {
                    return Err(invalid("LUT contains too many entries"));
                }
                values.push(triple(&words)?);
            }
        }
        if dimensions == 0
            || values.len() != size.pow(dimensions as u32)
            || (0..3).any(|c| high[c] - low[c] < 0.000001)
        {
            return Err(invalid(
                "LUT requires a complete table and increasing domains of width at least 0.000001",
            ));
        }
        if !matches!(
            (dimensions, self.interpolation),
            (_, Interpolation::Nearest)
                | (1, Interpolation::Linear)
                | (3, Interpolation::Trilinear | Interpolation::Tetrahedral)
        ) {
            return Err(invalid("Interpolation must match LUT dimensionality"));
        }
        Ok(Loaded {
            spec: self.clone(),
            title,
            dimensions,
            size,
            low,
            high,
            values,
        })
    }
}
impl Loaded {
    fn at(&self, p: [usize; 3]) -> [f64; 3] {
        self.values[p[0] + self.size * (p[1] + self.size * p[2])]
    }
    pub fn sample(&self, input: [f64; 3]) -> Result<[f64; 3]> {
        if input
            .iter()
            .any(|v| !v.is_finite() || !(-16.0..=16.0).contains(v))
        {
            return Err(invalid("Sample inputs must be finite and within -16..16"));
        }
        let x = std::array::from_fn::<_, 3, _>(|c| {
            ((input[c] - self.low[c]) / (self.high[c] - self.low[c])).clamp(0.0, 1.0)
                * (self.size - 1) as f64
        });
        if self.dimensions == 1 {
            return Ok(std::array::from_fn(|c| {
                if self.spec.interpolation == Interpolation::Nearest {
                    self.values[x[c].round() as usize][c]
                } else {
                    let lo = x[c].floor() as usize;
                    let hi = (lo + 1).min(self.size - 1);
                    let t = x[c] - lo as f64;
                    self.values[lo][c] * (1.0 - t) + self.values[hi][c] * t
                }
            }));
        }
        if self.spec.interpolation == Interpolation::Nearest {
            return Ok(self.at(x.map(|v| v.round() as usize)));
        }
        let lo = x.map(|v| (v.floor() as usize).min(self.size - 2));
        let f = std::array::from_fn::<_, 3, _>(|c| x[c] - lo[c] as f64);
        let mut result = [0.0; 3];
        if self.spec.interpolation == Interpolation::Trilinear {
            for corner in 0..8 {
                let mut position = lo;
                let mut weight = 1.0;
                for c in 0..3 {
                    let high = (corner >> c) & 1;
                    position[c] += high;
                    weight *= if high == 1 { f[c] } else { 1.0 - f[c] };
                }
                let value = self.at(position);
                for c in 0..3 {
                    result[c] += weight * value[c];
                }
            }
        } else {
            // The fractional-coordinate ordering selects one simplex; ties preserve R/G/B order.
            let mut axes = [0, 1, 2];
            axes.sort_by(|a, b| f[*b].total_cmp(&f[*a]));
            let mut position = lo;
            let mut previous = self.at(position);
            result = previous;
            for axis in axes {
                position[axis] += 1;
                let value = self.at(position);
                for c in 0..3 {
                    result[c] += f[axis] * (value[c] - previous[c]);
                }
                previous = value;
            }
        }
        Ok(result)
    }
    pub(crate) fn apply_rgb(&self, pixels: &mut [u8]) -> Result<()> {
        for pixel in pixels.as_chunks_mut::<3>().0 {
            let output = self.sample([
                pixel[0] as f64 / 255.0,
                pixel[1] as f64 / 255.0,
                pixel[2] as f64 / 255.0,
            ])?;
            for c in 0..3 {
                pixel[c] = (output[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        Ok(())
    }
    pub fn report(&self) -> Value {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in &self.values {
            for c in 0..3 {
                min[c] = min[c].min(v[c]);
                max[c] = max[c].max(v[c]);
            }
        }
        json!({"profile":"cube-lut-v1","file":self.spec.file,"title":self.title,"dimensions":self.dimensions,"size":self.size,"entries":self.values.len(),"domain_min":self.low,"domain_max":self.high,"table_min":min,"table_max":max,"interpolation":self.spec.interpolation,"ordering":"red_fastest_then_green_then_blue","outside_domain":"clamp_input","render_output":"clip_0_1_then_nearest_8bit","color_interpretation":"caller_declared_encoded_working_transfer_unchanged"})
    }
}
pub fn inspect(spec: &Transform, root: &Path, samples: &[[f64; 3]]) -> Result<Value> {
    if samples.len() > 256 {
        return Err(invalid("Inspect at most 256 sample colors"));
    }
    let loaded = spec.load(root)?;
    let mut report = loaded.report();
    report["samples"]=json!(samples.iter().map(|v|loaded.sample(*v).map(|out|json!({"input":v,"output_unclipped":out,"output_rgb8":out.map(|v|(v.clamp(0.0,1.0)*255.0).round() as u8)}))).collect::<Result<Vec<_>>>()?);
    Ok(report)
}
pub fn capabilities() -> Value {
    json!({"format":"bounded_ascii_cube_1d_or_3d","one_d_size":[2,65536],"three_d_size":[2,65],"maximum_bytes":8388608,"one_d_interpolation":["nearest","linear"],"three_d_interpolation":["nearest","trilinear","tetrahedral"],"application":"media.conform_after_explicit_sdr_normalization","source_identity_required":true})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simplex_and_cube_have_distinct_known_cross_terms() {
        let mut lut = Loaded {
            spec: Transform {
                file: Identity {
                    path: "test.cube".into(),
                    sha256: "0".repeat(64),
                    bytes: 1,
                },
                interpolation: Interpolation::Trilinear,
            },
            title: None,
            dimensions: 3,
            size: 2,
            low: [0.0; 3],
            high: [1.0; 3],
            values: Vec::new(),
        };
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    lut.values
                        .push([(r * g) as f64, (g * b) as f64, (r * b) as f64]);
                }
            }
        }
        assert_eq!(
            lut.sample([0.75, 0.5, 0.25]).unwrap(),
            [0.375, 0.125, 0.1875]
        );
        lut.spec.interpolation = Interpolation::Tetrahedral;
        assert_eq!(lut.sample([0.75, 0.5, 0.25]).unwrap(), [0.5, 0.25, 0.25]);
        assert_eq!(lut.sample([-1.0, 2.0, 2.0]).unwrap(), [0.0, 1.0, 0.0]);
        assert!(lut.sample([f64::NAN, 0.0, 0.0]).is_err());
    }
}
