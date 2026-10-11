//! Original, source-owned built-ins. Fitting changes only these authored recipes; inline templates
//! continue to use explicit pixel coordinates and ordinary binding semantics.
use super::*;
use crate::model::Project;

const CATALOG: &[(&str, &str)] = &[
    (
        "Lower third on a compact colored panel.",
        include_str!("../../examples/lower-third-template.json"),
    ),
    (
        "Centered title card with a rising label.",
        include_str!("../../examples/title-card-template.json"),
    ),
    (
        "Lower third with a thin accent rule and subtitle.",
        include_str!("designs/lower-third-rule.json"),
    ),
    (
        "Lower third with a circular name marker.",
        include_str!("designs/lower-third-marker.json"),
    ),
    (
        "Large centered title with a short subtitle.",
        include_str!("designs/title-center.json"),
    ),
    (
        "Left-aligned title beside a bold vertical panel.",
        include_str!("designs/title-split.json"),
    ),
    (
        "Closing message with two follow-up labels.",
        include_str!("designs/end-card.json"),
    ),
    (
        "Label and stepped arrow pointing right.",
        include_str!("designs/callout-arrow.json"),
    ),
    (
        "Outlined callout box with a caption below.",
        include_str!("designs/callout-box.json"),
    ),
    (
        "Chapter number and heading separated by an accent rule.",
        include_str!("designs/chapter-marker.json"),
    ),
    (
        "Progress bar with an editable fill rectangle and label.",
        include_str!("designs/progress-bar.json"),
    ),
    (
        "White caption with a dark glyph outline.",
        include_str!("designs/caption-outline.json"),
    ),
    (
        "Caption on a padded dark line box.",
        include_str!("designs/caption-box.json"),
    ),
    (
        "Accent-colored caption on a narrow bottom panel.",
        include_str!("designs/caption-accent.json"),
    ),
    (
        "Quotation and attribution beside an accent bar.",
        include_str!("designs/quote-card.json"),
    ),
    (
        "Original follow-along reminder without a platform logo.",
        include_str!("designs/reminder.json"),
    ),
    (
        "Three, two, one and a final message on successive beats.",
        include_str!("designs/countdown.json"),
    ),
    (
        "Speaker name and role inside a compact tag.",
        include_str!("designs/name-tag.json"),
    ),
    (
        "Three list items revealed on successive beats.",
        include_str!("designs/list-reveal.json"),
    ),
    (
        "Section heading between horizontal rules.",
        include_str!("designs/section-divider.json"),
    ),
];

fn output_parameters(template: &mut Template) {
    let scene = &template.scene;
    let parameters = [
        (
            "width",
            Definition::Integer {
                default: Some(scene.width as i32),
                min: 1,
                max: 4096,
            },
            Property::SceneWidth,
        ),
        (
            "height",
            Definition::Integer {
                default: Some(scene.height as i32),
                min: 1,
                max: 4096,
            },
            Property::SceneHeight,
        ),
        (
            "output_scale",
            Definition::Integer {
                default: Some(scene.output_scale as i32),
                min: 1,
                max: 8,
            },
            Property::SceneOutputScale,
        ),
        (
            "frame_rate",
            Definition::Time {
                default: Some(scene.frame_rate.unwrap_or(Time { num: 25, den: 1 })),
            },
            Property::SceneFrameRate,
        ),
        (
            "transparent",
            Definition::Boolean {
                default: Some(scene.transparent),
            },
            Property::SceneTransparent,
        ),
    ];
    for (name, definition, property) in parameters {
        template.parameters.push(Parameter {
            name: name.into(),
            definition,
            bindings: vec![Binding {
                property,
                layer: None,
            }],
        });
    }
}

fn load(source: &str) -> Result<Template> {
    let mut template: Template = serde_json::from_str(source)?;
    output_parameters(&mut template);
    Ok(template)
}

pub fn list(id: Option<&str>) -> Result<Value> {
    let mut entries = Vec::new();
    for (purpose, source) in CATALOG {
        let template = load(source)?;
        if id.is_some_and(|id| id != template.id) {
            continue;
        }
        let scene = &template.scene;
        entries.push(json!({"id":template.id,"purpose":purpose,
            "parameters":template.parameters.iter().map(|p| json!({"name":p.name,"definition":p.definition})).collect::<Vec<_>>(),
            "output":{"width":scene.width*scene.output_scale,"height":scene.height*scene.output_scale,
                "logical_width":scene.width,"logical_height":scene.height,"output_scale":scene.output_scale,
                "frame_rate":scene.clock()?,"duration":scene.duration,"transparent":scene.transparent}}));
    }
    if entries.is_empty()
        && let Some(id) = id
    {
        return Err(unknown(id));
    }
    Ok(
        json!({"templates":entries,"count":entries.len(),"font_policy":"Supply external fonts by identity or unambiguous installed name; no font is bundled",
        "target_project":"Built-in layouts fit its canvas; output_scale defaults to 1 and frame_rate to the project clock. Transparency keeps the template default."}),
    )
}

fn unknown(id: &str) -> crate::Error {
    error(
        "INVALID_TEMPLATE",
        format!("Unknown built-in template {id:?}"),
    )
    .with_path("template_id")
    .with_hint("Use templates.list for the built-in IDs and typed parameters")
}

/// Choose exactly one inline or built-in template. User values remain in output pixels; only
/// authored defaults are fitted. Every expanded scene is subsequently inspected as usual.
pub fn select(
    id: Option<&str>,
    inline: Option<&Template>,
    project: Option<&Project>,
    values: &BTreeMap<String, ParameterValue>,
) -> Result<Template> {
    match (id, inline) {
        (None, Some(template)) => Ok(template.clone()),
        (Some(id), None) => {
            if let Some(project) = project {
                project.validate()?;
            }
            for (_, source) in CATALOG {
                let template = load(source)?;
                if template.id == id {
                    return fit(template, project, values);
                }
            }
            Err(unknown(id))
        }
        _ => Err(error(
            "INVALID_TEMPLATE",
            "Provide exactly one of template_id or template",
        )
        .with_path("template_id")
        .with_hint("Use a built-in ID from templates.list or supply an inline template object")),
    }
}

fn integer(
    values: &BTreeMap<String, ParameterValue>,
    name: &str,
    default: u32,
    max: u32,
) -> Result<u32> {
    match values.get(name) {
        None if default > 0 && default <= max => Ok(default),
        Some(ParameterValue::Integer(n)) if *n > 0 && *n as u32 <= max => Ok(*n as u32),
        _ => Err(error(
            "INVALID_PARAMETER",
            format!("{name} requires an integer in 1..={max}"),
        )
        .with_path(format!("values.{name}"))),
    }
}
fn time_value(
    values: &BTreeMap<String, ParameterValue>,
    name: &str,
    default: Time,
) -> Result<Time> {
    match values.get(name) {
        None => Ok(default),
        Some(ParameterValue::Time(t)) => {
            t.validate()?;
            Ok(*t)
        }
        _ => Err(
            error("INVALID_PARAMETER", format!("{name} requires a time value"))
                .with_path(format!("values.{name}")),
        ),
    }
}
fn scale(value: i32, from: u32, to: u32) -> i32 {
    let n = i64::from(value) * i64::from(to);
    ((n.unsigned_abs() + u64::from(from) / 2) / u64::from(from)) as i32 * n.signum() as i32
}
fn rectangle(rect: &mut [i32; 4], from: [u32; 2], to: [u32; 2]) {
    for axis in 0..2 {
        let end = scale(rect[axis] + rect[axis + 2], from[axis], to[axis]);
        rect[axis] = scale(rect[axis], from[axis], to[axis]);
        rect[axis + 2] = (end - rect[axis]).max(1);
    }
}
fn ratio(after: Time, before: Time) -> Result<Time> {
    after.times(Time::new(before.den, before.num)?)
}
fn curve(curve: &mut Curve, clock: Time, axis: Option<(u32, u32)>) -> Result<()> {
    for key in &mut curve.keys {
        key.time = key.time.times(clock)?;
        if let Some((from, to)) = axis {
            key.value = scale(key.value, from, to);
        }
    }
    if let Some(retime) = &mut curve.retime {
        retime.start = retime.start.times(clock)?;
    }
    Ok(())
}

fn fit(
    mut template: Template,
    project: Option<&Project>,
    values: &BTreeMap<String, ParameterValue>,
) -> Result<Template> {
    let old = template.scene.clone();
    let width = integer(
        values,
        "width",
        project.map_or(old.width, |p| p.width),
        4096,
    )?;
    let height = integer(
        values,
        "height",
        project.map_or(old.height, |p| p.height),
        4096,
    )?;
    let output_scale = integer(
        values,
        "output_scale",
        if project.is_some() {
            1
        } else {
            old.output_scale
        },
        8,
    )?;
    let rate = crate::render::clock::rate(time_value(
        values,
        "frame_rate",
        project.map_or(old.clock()?, |p| p.frame_rate),
    )?)?;
    // The three fractional native rates run their integer-rate design beats at 1001/1000.
    // Whole-second authored layers therefore remain exact frames, samples and milliseconds.
    let duration = time_value(
        values,
        "duration",
        old.duration.times(if rate.den == 1001 {
            Time::new(1001, 1000)?
        } else {
            Time::new(1, 1)?
        })?,
    )?;
    let frames = duration.units(rate)?;
    duration.units(Time::new(48000, 1)?)?;
    if frames == 0 || duration.compare(Time::new(scene::MAX_SECONDS, 1)?)?.is_gt() {
        return Err(error(
            "INVALID_PARAMETER",
            "Built-in duration must be positive and within the scene duration limit",
        )
        .with_path("values.duration"));
    }
    let from = [old.width, old.height];
    let to = [width, height];
    let uniform =
        if u64::from(width) * u64::from(old.height) <= u64::from(height) * u64::from(old.width) {
            (old.width, width)
        } else {
            (old.height, height)
        };
    let phase = |time: Time| -> Result<Time> {
        let numerator = u128::from(time.num) * u128::from(frames) * u128::from(old.duration.den);
        let denominator = u128::from(time.den) * u128::from(old.duration.num);
        Time::new(
            ((numerator + denominator / 2) / denominator) as u64 * rate.den,
            rate.num,
        )
    };
    let scene = &mut template.scene;
    scene.width = width;
    scene.height = height;
    scene.output_scale = output_scale;
    scene.frame_rate = Some(rate);
    scene.duration = duration;
    for (layer, before) in scene.layers.iter_mut().zip(&old.layers) {
        layer.start = phase(before.start)?;
        layer.duration = phase(before.start.plus(before.duration)?)?.minus(layer.start)?;
        if layer.duration.num == 0 {
            return Err(error(
                "INVALID_PARAMETER",
                "Duration is too short to show every built-in layer for at least one frame",
            )
            .with_path("values.duration"));
        }
        layer.canvas = to;
        layer.transform.crop = [0, 0, width, height];
        for axis in 0..2 {
            layer.transform.position[axis] =
                scale(layer.transform.position[axis], from[axis], to[axis]);
        }
        if let Some(animation) = &mut layer.animation {
            let clock = ratio(layer.duration, before.duration)?;
            if let Some(c) = &mut animation.position_x {
                curve(c, clock, Some((from[0], to[0])))?;
            }
            if let Some(c) = &mut animation.position_y {
                curve(c, clock, Some((from[1], to[1])))?;
            }
            if let Some(c) = &mut animation.opacity {
                curve(c, clock, None)?;
            }
        }
        match layer
            .graphics
            .as_mut()
            .ok_or_else(|| invalid("Built-ins require graphics layers"))?
        {
            Graphic::Text {
                rect,
                size,
                line_height,
                letter_spacing,
                background,
                outline,
                ..
            } => {
                rectangle(rect, from, to);
                *size = scale(i32::from(*size), uniform.0, uniform.1).clamp(1, 512) as u16;
                *line_height =
                    scale(i32::from(*line_height), uniform.0, uniform.1).clamp(1, 2048) as u16;
                *letter_spacing =
                    scale(i32::from(*letter_spacing), uniform.0, uniform.1).clamp(0, 128) as u16;
                if let Some(background) = background {
                    background.padding = scale(i32::from(background.padding), uniform.0, uniform.1)
                        .clamp(0, 256) as u16;
                }
                if let Some(outline) = outline {
                    outline.width =
                        scale(i32::from(outline.width), uniform.0, uniform.1).clamp(1, 8) as u8;
                }
            }
            Graphic::Shape { rect, stroke, .. } => {
                rectangle(rect, from, to);
                if let Some(stroke) = stroke {
                    stroke.width =
                        scale(stroke.width as i32, uniform.0, uniform.1).clamp(1, 256) as u32;
                }
            }
        }
    }
    for parameter in &mut template.parameters {
        let binding = &parameter.bindings[0];
        let property = binding.property;
        let axis = match property {
            Property::PositionX | Property::PositionXCurve => Some((from[0], to[0])),
            Property::PositionY | Property::PositionYCurve => Some((from[1], to[1])),
            _ => None,
        };
        match &mut parameter.definition {
            Definition::Integer {
                default: Some(default),
                min,
                max,
            } => match property {
                Property::SceneWidth => *default = width as i32,
                Property::SceneHeight => *default = height as i32,
                Property::SceneOutputScale => *default = output_scale as i32,
                Property::FontSize
                | Property::LineHeight
                | Property::LetterSpacing
                | Property::StrokeWidth => {
                    (*min, *max) = match property {
                        Property::FontSize => (1, 512),
                        Property::LineHeight => (1, 2048),
                        Property::LetterSpacing => (0, 128),
                        _ => (1, 256),
                    };
                    *default = scale(*default, uniform.0, uniform.1).clamp(*min, *max);
                }
                _ => {
                    if let Some((a, b)) = axis {
                        *default = scale(*default, a, b);
                        *min = scale(*min, a, b);
                        *max = scale(*max, a, b);
                    }
                }
            },
            Definition::Rect {
                default: Some(rect),
            } => rectangle(rect, from, to),
            Definition::Time {
                default: Some(default),
            } => {
                *default = match property {
                    Property::SceneFrameRate => rate,
                    Property::SceneDuration => duration,
                    Property::LayerDuration => {
                        scene
                            .layers
                            .iter()
                            .find(|l| Some(&l.id) == binding.layer.as_ref())
                            .unwrap()
                            .duration
                    }
                    Property::LayerStart => {
                        scene
                            .layers
                            .iter()
                            .find(|l| Some(&l.id) == binding.layer.as_ref())
                            .unwrap()
                            .start
                    }
                    _ => *default,
                };
            }
            Definition::Curve { default: Some(c) } => {
                let index = old
                    .layers
                    .iter()
                    .position(|l| Some(&l.id) == binding.layer.as_ref())
                    .ok_or_else(|| invalid("Built-in curve requires a layer"))?;
                curve(
                    c,
                    ratio(scene.layers[index].duration, old.layers[index].duration)?,
                    axis,
                )?;
            }
            _ => {}
        }
    }
    Ok(template)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_ids_are_unique_and_all_output_fields_are_bound() {
        let mut ids = HashSet::new();
        for (_, source) in CATALOG {
            let template = load(source).unwrap();
            assert!(ids.insert(template.id.clone()));
            for property in [
                Property::SceneWidth,
                Property::SceneHeight,
                Property::SceneOutputScale,
                Property::SceneFrameRate,
                Property::SceneTransparent,
            ] {
                assert!(
                    template
                        .parameters
                        .iter()
                        .any(|p| p.bindings.iter().any(|b| b.property == property))
                );
            }
        }
        assert_eq!(ids.len(), 20);
        assert!(select(None, None, None, &BTreeMap::new()).is_err());
        assert!(list(Some("unknown")).is_err());
    }
}
