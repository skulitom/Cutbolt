//! Explicit display geometry for SDR normalization, using integer coordinates throughout.
use crate::{Result, error, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Canvas fitting after source display rotation and sample aspect have been interpreted.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Preserve display aspect and center the complete picture, padding uncovered pixels.
    Contain,
    /// Preserve display aspect and fill the canvas, cropping the center of the picture.
    Cover,
    /// Fill the canvas independently on both axes after rotation.
    Stretch,
}

/// Explicitly normalize canonical quarter-turn display metadata and positive sample aspect.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    /// Required canvas fit. Sampling is nearest top-left; centering leaves odd excess right/bottom.
    pub fit: Fit,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Display {
    turns: u32,
    sar: Time,
    assumed: bool,
}

fn invalid(message: &str, observed: &Value) -> crate::Error {
    error("UNSUPPORTED_MEDIA", message)
        .with_path("recipe.source.geometry")
        .with_hint("Use a canonical quarter-turn matrix and a fixed positive sample aspect; choose contain, cover or stretch explicitly.")
        .with_detail(json!({"observed":observed}))
}

fn sar(value: &Value) -> Result<(Time, bool)> {
    if value.is_null() || value.as_str() == Some("N/A") {
        return Ok((Time { num: 1, den: 1 }, true));
    }
    let parsed = || -> Option<Time> {
        let (n, d) = value.as_str()?.split_once(':')?;
        let (num, den) = (n.parse::<u32>().ok()?, d.parse::<u32>().ok()?);
        if num == 0 || den == 0 || num > i32::MAX as u32 || den > i32::MAX as u32 {
            return None;
        }
        Time::new(u64::from(num), u64::from(den)).ok()
    };
    parsed().map(|s| (s, false)).ok_or_else(|| {
        invalid(
            "Sample aspect components must be integers in 1..2147483647",
            value,
        )
    })
}

const MATRICES: [[i64; 9]; 4] = [
    [65536, 0, 0, 0, 65536, 0, 0, 0, 1073741824],
    [0, -65536, 0, 65536, 0, 0, 0, 0, 1073741824],
    [-65536, 0, 0, 0, -65536, 0, 0, 0, 1073741824],
    [0, 65536, 0, -65536, 0, 0, 0, 0, 1073741824],
];

fn matrix(value: &Value) -> Result<u32> {
    let parse = || -> Option<u32> {
        let lines: Vec<_> = value
            .as_str()?
            .lines()
            .filter(|s| !s.trim().is_empty())
            .collect();
        if lines.len() != 3 {
            return None;
        }
        let mut numbers = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let (label, values) = line.split_once(':')?;
            if label.trim() != format!("{row:08x}") {
                return None;
            }
            let values: Vec<i64> = values
                .split_whitespace()
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()
                .ok()?;
            if values.len() != 3 {
                return None;
            }
            numbers.extend(values);
        }
        MATRICES
            .iter()
            .position(|m| m.as_slice() == numbers)
            .map(|i| i as u32)
    };
    parse().ok_or_else(|| invalid("Only exact identity/90/180/270-degree display matrices are supported; reflection, shear, translation and scale are unsupported", value))
}

fn angle(value: &Value) -> Option<u32> {
    let degrees = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    if !degrees.is_finite() || degrees.abs() > 360.0 || degrees % 90.0 != 0.0 {
        return None;
    }
    Some((degrees as i32 / 90).rem_euclid(4) as u32)
}

pub(crate) fn frame_has_transform(frame: &Value) -> bool {
    frame.as_object().is_some_and(|object| {
        object.keys().any(|key| {
            key.ends_with("displaymatrix") || key.ends_with(":rotation") || key == "rotation"
        })
    })
}

impl Display {
    pub(crate) fn inspect(stream: &Value) -> Result<Self> {
        let (sar, assumed) = sar(&stream["sample_aspect_ratio"])?;
        let mut turns = 0;
        let mut seen = false;
        if let Some(data) = stream.get("side_data_list") {
            let data = data
                .as_array()
                .ok_or_else(|| invalid("Malformed source side data", data))?;
            for item in data {
                if seen || item["side_data_type"] != "Display Matrix" {
                    return Err(invalid(
                        "Only one display matrix and no other stream side data are supported",
                        item,
                    ));
                }
                turns = matrix(&item["displaymatrix"])?;
                if let Some(rotation) = item.get("rotation")
                    && angle(rotation) != Some(turns)
                {
                    return Err(invalid("Display rotation contradicts its matrix", item));
                }
                seen = true;
            }
        }
        if let Some(rotation) = stream["tags"].get("rotate")
            && (angle(rotation) != Some(turns) || (!seen && turns != 0))
        {
            return Err(invalid(
                "Legacy rotate tag requires a matching canonical display matrix",
                rotation,
            ));
        }
        Ok(Self {
            turns,
            sar,
            assumed,
        })
    }

    pub(crate) fn oriented_size(self, size: [u32; 2]) -> [u32; 2] {
        if self.turns.is_multiple_of(2) {
            size
        } else {
            [size[1], size[0]]
        }
    }

    pub(crate) fn check_frame(self, frame: &Value, index: usize) -> Result<()> {
        let entries = frame.as_object().expect("decoded frame metadata");
        let mut matrices = 0;
        for (key, value) in entries {
            if key.ends_with("displaymatrix") {
                matrices += 1;
                // ffprobe compact uses C-escaped newlines in its fixed-point matrix rows.
                let text = value.as_str().unwrap_or("").replace("\\n", "\n");
                if matrices > 1 || matrix(&json!(text))? != self.turns {
                    return Err(invalid(
                        "Changing decoded display matrix is unsupported",
                        &json!({"frame":index,"matrix":value}),
                    ));
                }
            }
            if (key.ends_with(":rotation") || key == "rotation") && angle(value) != Some(self.turns)
            {
                return Err(invalid(
                    "Decoded rotation contradicts the source display matrix",
                    &json!({"frame":index,"rotation":value}),
                ));
            }
        }
        // Container SAR remains authoritative when the decoder supplies no frame-level value.
        if frame["sample_aspect_ratio"].is_null() || frame["sample_aspect_ratio"] == "N/A" {
            return Ok(());
        }
        let (actual, _) = sar(&frame["sample_aspect_ratio"])?;
        if actual != self.sar {
            return Err(invalid(
                "Changing decoded sample aspect is unsupported",
                &json!({"frame":index,"stream":self.sar,"frame_sar":frame["sample_aspect_ratio"]}),
            ));
        }
        Ok(())
    }

    pub(crate) fn plan(self, source: [u32; 2], canvas: [u32; 2], fit: Fit) -> Result<Plan> {
        if [source, canvas].iter().any(|[w, h]| {
            *w == 0
                || *h == 0
                || *w > 4096
                || *h > 2160
                || u64::from(*w) * u64::from(*h) > 8_000_000
        }) {
            return Err(invalid(
                "Geometry requires positive source and canvas dimensions up to 4096x2160/8M pixels",
                &json!({"source":source,"canvas":canvas}),
            ));
        }
        let [sw, sh] = source.map(u64::from);
        let [w, h] = canvas.map(u64::from);
        let (num, den, oriented) = if self.turns.is_multiple_of(2) {
            (sw * self.sar.num, sh * self.sar.den, [sw, sh])
        } else {
            (sh * self.sar.den, sw * self.sar.num, [sh, sw])
        };
        let divide =
            |a: u128, b: u128, up: bool| -> u64 { (if up { a.div_ceil(b) } else { a / b }) as u64 };
        let wider = u128::from(num) * u128::from(h) > u128::from(den) * u128::from(w);
        let cover = matches!(fit, Fit::Cover);
        let scaled = match fit {
            Fit::Stretch => [w, h],
            _ if wider != cover => [
                w,
                divide(u128::from(w) * u128::from(den), u128::from(num), cover),
            ],
            _ => [
                divide(u128::from(h) * u128::from(num), u128::from(den), cover),
                h,
            ],
        };
        if scaled.contains(&0) {
            return Err(invalid(
                "Contain fit is smaller than one canvas pixel; choose another canvas or fit",
                &json!({"canvas":canvas,"sample_aspect":self.sar}),
            ));
        }
        let pad = [
            w.saturating_sub(scaled[0]) / 2,
            h.saturating_sub(scaled[1]) / 2,
        ];
        let crop = [
            scaled[0].saturating_sub(w) / 2,
            scaled[1].saturating_sub(h) / 2,
        ];
        Ok(Plan {
            display: self,
            source,
            canvas,
            oriented,
            scaled,
            pad,
            crop,
            fit,
        })
    }
}

pub(crate) struct Plan {
    display: Display,
    source: [u32; 2],
    canvas: [u32; 2],
    oriented: [u64; 2],
    scaled: [u64; 2],
    pad: [u64; 2],
    crop: [u64; 2],
    fit: Fit,
}

impl Plan {
    /// At most four bytes per output pixel, with no virtual resized-frame allocation.
    pub(crate) fn indices(&self) -> Vec<u32> {
        let [sw, sh] = self.source.map(u64::from);
        let mut map = Vec::with_capacity(self.canvas[0] as usize * self.canvas[1] as usize);
        for y in 0..u64::from(self.canvas[1]) {
            for x in 0..u64::from(self.canvas[0]) {
                if x < self.pad[0]
                    || y < self.pad[1]
                    || x - self.pad[0] >= self.scaled[0]
                    || y - self.pad[1] >= self.scaled[1]
                {
                    map.push(u32::MAX);
                    continue;
                }
                let rx = ((u128::from(x - self.pad[0] + self.crop[0])
                    * u128::from(self.oriented[0]))
                    / u128::from(self.scaled[0])) as u64;
                let ry = ((u128::from(y - self.pad[1] + self.crop[1])
                    * u128::from(self.oriented[1]))
                    / u128::from(self.scaled[1])) as u64;
                let (sx, sy) = match self.display.turns {
                    0 => (rx, ry),
                    1 => (sw - 1 - ry, rx),
                    2 => (sw - 1 - rx, sh - 1 - ry),
                    3 => (ry, sh - 1 - rx),
                    _ => unreachable!("validated quarter turn"),
                };
                map.push((sy * sw + sx) as u32);
            }
        }
        map
    }

    pub(crate) fn report(&self, alpha: bool) -> Value {
        json!({"rotation_counterclockwise":self.display.turns * 90,"sample_aspect":self.display.sar,
            "missing_sample_aspect_assumed_square":self.display.assumed,"source_size":self.source,
            "oriented_size":self.oriented,"canvas_size":self.canvas,"fit":self.fit,"scaled_size":self.scaled,
            "pad_left_top":self.pad,"crop_left_top":self.crop,"centering":"floor_left_top_excess_right_bottom",
            "fit_rounding":match self.fit {Fit::Cover=>"ceil",Fit::Contain=>"floor",Fit::Stretch=>"none"},
            "sampling":"nearest_top_left","output_sample_aspect":{"num":1,"den":1},
            "padding":if alpha {"transparent_black"} else {"black"}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(turn: usize) -> Value {
        let text = MATRICES[turn]
            .chunks(3)
            .enumerate()
            .map(|(i, r)| format!("{i:08x}: {} {} {}\n", r[0], r[1], r[2]))
            .collect::<String>();
        json!({"sample_aspect_ratio":"1:1","side_data_list":[{"side_data_type":"Display Matrix","displaymatrix":text,"rotation":turn * 90}]})
    }

    #[test]
    fn quarter_turns_follow_display_orientation() {
        for (turn, wanted) in [
            [0, 1, 2, 3, 4, 5],
            [2, 5, 1, 4, 0, 3],
            [5, 4, 3, 2, 1, 0],
            [3, 0, 4, 1, 5, 2],
        ]
        .iter()
        .enumerate()
        {
            let canvas = if turn % 2 == 0 { [3, 2] } else { [2, 3] };
            let plan = Display::inspect(&stream(turn))
                .unwrap()
                .plan([3, 2], canvas, Fit::Contain)
                .unwrap();
            assert_eq!(&plan.indices(), wanted);
        }
    }

    #[test]
    fn fits_keep_aspect_center_and_report_padding() {
        let display = Display::inspect(&json!({"sample_aspect_ratio":"2:1"})).unwrap();
        let contained = display.plan([2, 2], [5, 5], Fit::Contain).unwrap();
        let z = u32::MAX;
        assert_eq!(
            contained.indices(),
            vec![
                z, z, z, z, z, 0, 0, 0, 1, 1, 2, 2, 2, 3, 3, z, z, z, z, z, z, z, z, z, z
            ]
        );
        assert_eq!(contained.report(true)["padding"], "transparent_black");
        let covered = display.plan([2, 2], [3, 3], Fit::Cover).unwrap();
        assert_eq!(covered.indices(), vec![0, 0, 1, 0, 0, 1, 2, 2, 3]);
        let stretched = display.plan([2, 2], [3, 3], Fit::Stretch).unwrap();
        assert_eq!(stretched.indices(), vec![0, 0, 1, 0, 0, 1, 2, 2, 3]);
    }

    #[test]
    fn extreme_aspect_uses_bounded_canvas_mapping() {
        let display = Display::inspect(&json!({"sample_aspect_ratio":"2147483647:1"})).unwrap();
        assert!(display.plan([u32::MAX, 2], [3, 3], Fit::Cover).is_err());
        assert!(display.plan([2, 2], [u32::MAX, 3], Fit::Cover).is_err());
        assert!(display.plan([4096, 1920], [3, 3], Fit::Contain).is_err());
        let plan = display.plan([4096, 1920], [3, 3], Fit::Cover).unwrap();
        assert_eq!(plan.indices().len(), 9);
        assert!(plan.indices().iter().all(|p| *p < 4096 * 1920));
    }

    #[test]
    fn unsupported_transforms_and_changing_aspect_fail() {
        for text in ["0:1", "1:0", "-1:1", "1.5:1", "2147483648:1", "1:2:3"] {
            assert!(Display::inspect(&json!({"sample_aspect_ratio":text})).is_err());
        }
        let mut value = stream(1);
        value["side_data_list"][0]["rotation"] = json!(-90);
        assert!(Display::inspect(&value).is_err());
        value = stream(0);
        value["side_data_list"][0]["displaymatrix"] =
            json!("00000000: 65536 0 0\n00000001: 0 65536 0\n00000002: 1 0 1073741824\n");
        assert!(Display::inspect(&value).is_err());
        assert!(
            Display::inspect(
                &json!({"side_data_list":[{"side_data_type":"Mastering display metadata"}]})
            )
            .is_err()
        );
        assert!(Display::inspect(&json!({"tags":{"rotate":"90"}})).is_err());
        let display = Display::inspect(&json!({"sample_aspect_ratio":"4:3"})).unwrap();
        assert!(
            display
                .check_frame(&json!({"sample_aspect_ratio":"1:1"}), 7)
                .is_err()
        );
        display
            .check_frame(&json!({"sample_aspect_ratio":"8:6"}), 7)
            .unwrap();
        let source = Display::inspect(&stream(1)).unwrap();
        let mut frame = json!({"sample_aspect_ratio":"1:1", "side_datum/3x3_displaymatrix:displaymatrix":stream(1)["side_data_list"][0]["displaymatrix"],"side_datum/3x3_displaymatrix:rotation":90});
        source.check_frame(&frame, 1).unwrap();
        frame["side_datum/3x3_displaymatrix:rotation"] = json!(-90);
        assert!(source.check_frame(&frame, 2).is_err());
        frame["side_datum/3x3_displaymatrix:rotation"] = json!(90);
        frame["side_datum/3x3_displaymatrix:displaymatrix"] =
            stream(2)["side_data_list"][0]["displaymatrix"].clone();
        assert!(source.check_frame(&frame, 2).is_err());
    }
}
