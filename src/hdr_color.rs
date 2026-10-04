//! Original absolute-light conversion, public transfer equations and declared tone mapping.
use crate::{Result, error};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    Pq,
    Hlg,
    Srgb,
    Bt709,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Primaries {
    Bt709,
    Bt2020,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub peak_nits: u32,
    pub black_millinits: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Encoding {
    pub transfer: Transfer,
    pub primaries: Primaries,
    pub display: Display,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tone {
    Preserve,
    Clip {
        reference_white_nits: u32,
    },
    Reinhard {
        reference_white_nits: u32,
        source_peak_nits: u32,
    },
}
pub(crate) fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_HDR", message)
}
impl Transfer {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Pq => "smpte2084",
            Self::Hlg => "arib-std-b67",
            Self::Srgb => "iec61966-2-1",
            Self::Bt709 => "bt709",
        }
    }
    pub(crate) fn hdr(self) -> bool {
        matches!(self, Self::Pq | Self::Hlg)
    }
}
impl Primaries {
    pub(crate) fn tag(self) -> &'static str {
        if self == Self::Bt709 {
            "bt709"
        } else {
            "bt2020"
        }
    }
    pub(crate) fn xy(self) -> [[f64; 2]; 3] {
        match self {
            Self::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            Self::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        }
    }
    fn luma(self, rgb: [f64; 3]) -> f64 {
        let w = if self == Self::Bt709 {
            [0.2126, 0.7152, 0.0722]
        } else {
            [0.2627, 0.6780, 0.0593]
        };
        dot(w, rgb)
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
type Matrix = [[f64; 3]; 3];
fn inverse(m: Matrix) -> Matrix {
    let mut cof = [[0.0; 3]; 3];
    for (r, row) in cof.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            let rr: Vec<_> = (0..3).filter(|i| *i != r).collect();
            let cc: Vec<_> = (0..3).filter(|i| *i != c).collect();
            *value = (m[rr[0]][cc[0]] * m[rr[1]][cc[1]] - m[rr[0]][cc[1]] * m[rr[1]][cc[0]])
                * if (r + c) % 2 == 0 { 1.0 } else { -1.0 };
        }
    }
    let d = dot(m[0], cof[0]);
    std::array::from_fn(|r| std::array::from_fn(|c| cof[c][r] / d))
}
fn matrix(p: Primaries) -> Matrix {
    let xy = p.xy();
    let columns: Matrix = xy.map(|[x, y]| [x / y, 1.0, (1.0 - x - y) / y]);
    let basis: Matrix = std::array::from_fn(|r| std::array::from_fn(|c| columns[c][r]));
    let white = [0.3127 / 0.3290, 1.0, (1.0 - 0.3127 - 0.3290) / 0.3290];
    let scales = inverse(basis).map(|row| dot(row, white));
    std::array::from_fn(|r| std::array::from_fn(|c| basis[r][c] * scales[c]))
}
pub(crate) fn gamut(source: Primaries, target: Primaries) -> Matrix {
    if source == target {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let a = matrix(source);
    let b = inverse(matrix(target));
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|i| b[r][i] * a[i][c]).sum()))
}
impl Encoding {
    pub(crate) fn validate(self) -> Result<()> {
        let peak = self.display.peak_nits;
        if !(48..=10000).contains(&peak)
            || self.display.black_millinits > 1000
            || self.display.black_millinits as f64 / 1000.0 >= peak as f64
        {
            return Err(invalid(
                "Display peak must be 48..10000 nits and black 0..1000 millinits below peak",
            ));
        }
        if self.transfer.hdr() && self.primaries != Primaries::Bt2020 {
            return Err(invalid(
                "PQ and HLG require BT.2020 primaries in this profile",
            ));
        }
        if self.transfer == Transfer::Hlg && !(400..=2000).contains(&peak) {
            return Err(invalid("HLG display peak must be 400..2000 nits"));
        }
        if !self.transfer.hdr() && peak > 400 {
            return Err(invalid("SDR display peak must be 48..400 nits"));
        }
        Ok(())
    }
    fn gamma(self) -> f64 {
        1.2 + 0.42 * (self.display.peak_nits as f64 / 1000.0).log10()
    }
    fn beta(self) -> f64 {
        (3.0 * (self.display.black_millinits as f64 / 1000.0 / self.display.peak_nits as f64)
            .powf(1.0 / self.gamma()))
        .sqrt()
    }
    pub(crate) fn decode(self, encoded: [f64; 3]) -> [f64; 3] {
        match self.transfer {
            Transfer::Pq => encoded.map(|v| {
                let t = v.clamp(0.0, 1.0).powf(1.0 / 78.84375);
                10000.0
                    * ((t - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * t))
                        .powf(1.0 / 0.1593017578125)
            }),
            Transfer::Hlg => {
                let a = 0.17883277;
                let b = 1.0 - 4.0 * a;
                let c = 0.5 - a * (4.0_f64 * a).ln();
                let beta = self.beta();
                let scene = encoded.map(|v| {
                    let x = (1.0 - beta) * v.clamp(0.0, 1.0) + beta;
                    if x <= 0.5 {
                        x * x / 3.0
                    } else {
                        (((x - c) / a).exp() + b) / 12.0
                    }
                });
                let gain = self.display.peak_nits as f64
                    * self.primaries.luma(scene).powf(self.gamma() - 1.0);
                scene.map(|v| v * gain)
            }
            Transfer::Srgb | Transfer::Bt709 => encoded.map(|v| {
                let x = v.clamp(0.0, 1.0);
                self.display.peak_nits as f64
                    * if self.transfer == Transfer::Srgb {
                        if x <= 0.04045 {
                            x / 12.92
                        } else {
                            ((x + 0.055) / 1.055).powf(2.4)
                        }
                    } else if x < 0.081 {
                        x / 4.5
                    } else {
                        ((x + 0.099) / 1.099).powf(1.0 / 0.45)
                    }
            }),
        }
    }
    pub(crate) fn encode(self, rgb: [f64; 3]) -> [f64; 3] {
        match self.transfer {
            Transfer::Pq => rgb.map(|v| {
                let t = (v.max(0.0) / 10000.0).powf(0.1593017578125);
                ((0.8359375 + 18.8515625 * t) / (1.0 + 18.6875 * t)).powf(78.84375)
            }),
            Transfer::Hlg => {
                let relative = rgb.map(|v| v.max(0.0) / self.display.peak_nits as f64);
                let y = self.primaries.luma(relative);
                let gain = if y == 0.0 {
                    0.0
                } else {
                    y.powf(1.0 / self.gamma() - 1.0)
                };
                let beta = self.beta();
                let a = 0.17883277;
                let b = 1.0 - 4.0 * a;
                let c = 0.5 - a * (4.0_f64 * a).ln();
                relative.map(|v| {
                    let x = v * gain;
                    let signal = if x <= 1.0 / 12.0 {
                        (3.0 * x).sqrt()
                    } else {
                        a * (12.0 * x - b).ln() + c
                    };
                    (signal - beta) / (1.0 - beta)
                })
            }
            Transfer::Srgb | Transfer::Bt709 => rgb.map(|v| {
                let x = v.max(0.0) / self.display.peak_nits as f64;
                if self.transfer == Transfer::Srgb {
                    if x <= 0.0031308 {
                        12.92 * x
                    } else {
                        1.055 * x.powf(1.0 / 2.4) - 0.055
                    }
                } else if x < 0.018 {
                    4.5 * x
                } else {
                    1.099 * x.powf(0.45) - 0.099
                }
            }),
        }
    }
}
pub(crate) struct Processor {
    source: Encoding,
    target: Encoding,
    matrix: Matrix,
    gain: f64,
    tone: Tone,
}
impl Processor {
    pub(crate) fn new(
        source: Encoding,
        target: Encoding,
        exposure_milliev: i32,
        tone: Tone,
    ) -> Result<Self> {
        source.validate()?;
        target.validate()?;
        if !(-8000..=8000).contains(&exposure_milliev) {
            return Err(invalid("Exposure must be within -8000..8000 milli-EV"));
        }
        match tone {
            Tone::Preserve => (),
            Tone::Clip {
                reference_white_nits,
            }
            | Tone::Reinhard {
                reference_white_nits,
                ..
            } => {
                if target.transfer.hdr() || !(48..=1000).contains(&reference_white_nits) {
                    return Err(invalid(
                        "Tone mapping requires SDR output and reference white 48..1000 nits",
                    ));
                }
            }
        }
        if let Tone::Reinhard {
            reference_white_nits,
            source_peak_nits,
        } = tone
            && (source_peak_nits < reference_white_nits || source_peak_nits > 10000)
        {
            return Err(invalid(
                "Tone-map source peak must be reference white..10000 nits",
            ));
        }
        if source.transfer.hdr() && !target.transfer.hdr() && tone == Tone::Preserve {
            return Err(invalid(
                "HDR to SDR requires an explicit clip or reinhard policy",
            ));
        }
        Ok(Self {
            source,
            target,
            matrix: gamut(source.primaries, target.primaries),
            gain: 2.0f64.powf(exposure_milliev as f64 / 1000.0),
            tone,
        })
    }
    pub(crate) fn sample(&self, encoded: [f64; 3]) -> [f64; 3] {
        let light = self.source.decode(encoded).map(|v| v * self.gain);
        let mut rgb = self.matrix.map(|row| dot(row, light).max(0.0));
        match self.tone {
            Tone::Preserve => (),
            Tone::Clip {
                reference_white_nits,
            } => {
                rgb = rgb.map(|v| {
                    (v / reference_white_nits as f64).min(1.0)
                        * self.target.display.peak_nits as f64
                });
            }
            Tone::Reinhard {
                reference_white_nits,
                source_peak_nits,
            } => {
                let x = rgb.into_iter().fold(0.0, f64::max) / reference_white_nits as f64;
                let peak = source_peak_nits as f64 / reference_white_nits as f64;
                let mapped = (x * (1.0 + x / (peak * peak)) / (1.0 + x)).min(1.0);
                let scale = if x == 0.0 { 0.0 } else { mapped / x }
                    * self.target.display.peak_nits as f64
                    / reference_white_nits as f64;
                rgb = rgb.map(|v| v * scale);
            }
        }
        self.target.encode(rgb)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_transfer_anchors_and_round_trips() {
        for t in [Transfer::Pq, Transfer::Hlg, Transfer::Srgb, Transfer::Bt709] {
            let e = Encoding {
                transfer: t,
                primaries: if t.hdr() {
                    Primaries::Bt2020
                } else {
                    Primaries::Bt709
                },
                display: Display {
                    peak_nits: if t.hdr() { 1000 } else { 100 },
                    black_millinits: 0,
                },
            };
            for code in [0, 1, 4096, 16384, 32768, 49152, 65535] {
                let v = code as f64 / 65535.0;
                let out = e.encode(e.decode([v; 3]));
                assert!((out[0] - v).abs() < 0.000002);
            }
        }
        let pq = Encoding {
            transfer: Transfer::Pq,
            primaries: Primaries::Bt2020,
            display: Display {
                peak_nits: 1000,
                black_millinits: 0,
            },
        };
        assert!((pq.decode([1.0; 3])[0] - 10000.0).abs() < 0.001);
        assert!((pq.encode([100.0; 3])[0] - 0.508078421517).abs() < 1e-10);
        let hlg = Encoding {
            transfer: Transfer::Hlg,
            ..pq
        };
        assert!((hlg.decode([0.75; 3])[0] - 203.15214594).abs() < 0.00001);
        let black = Encoding {
            display: Display {
                peak_nits: 1000,
                black_millinits: 5,
            },
            ..hlg
        };
        assert!((black.decode([0.0; 3])[0] - 0.005).abs() < 1e-9);
    }
    #[test]
    fn primary_matrix_white_and_inverse() {
        let forward = gamut(Primaries::Bt2020, Primaries::Bt709);
        let reverse = gamut(Primaries::Bt709, Primaries::Bt2020);
        for v in [[1.0; 3], [0.4, 0.2, 0.1], [1.0, 0.0, 0.0]] {
            let out = reverse.map(|row| dot(row, forward.map(|r| dot(r, v))));
            for i in 0..3 {
                assert!((out[i] - v[i]).abs() < 1e-12);
            }
        }
        for row in forward {
            assert!((dot(row, [1.0; 3]) - 1.0).abs() < 1e-12);
        }
    }
}
