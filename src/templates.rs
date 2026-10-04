//! Portable original scene templates with explicit typed bindings and no code evaluation.
use crate::{
    Result,
    animation::Curve,
    error,
    graphics::Graphic,
    scene::{self, Identity, Scene},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

/// Typed template parameter value as `{"type": ..., "value": ...}`; the type must match the parameter definition.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ParameterValue {
    /// Value for a `text` parameter.
    Text(
        /// Nonempty string within the definition's `max_chars` and 4096 UTF-8 bytes.
        String,
    ),
    /// Value for an `integer` parameter.
    Integer(
        /// Signed 32-bit integer within the definition's inclusive `min`..=`max`.
        i32,
    ),
    /// Value for a `color` parameter.
    Color(
        /// Straight-alpha RGBA color as `[r, g, b, a]` bytes.
        [u8; 4],
    ),
    /// Value for a `fonts` parameter.
    Fonts(
        /// Ordered external font identities, 1..=4.
        Vec<Identity>,
    ),
    /// Value for a `curve` parameter.
    Curve(
        /// Ordinary scene keyframe curve with 1..=128 keys.
        Curve,
    ),
    /// Value for a `time` parameter.
    Time(
        /// Rational seconds `{num, den}`; frame alignment is checked in the resolved scene.
        Time,
    ),
    /// Value for a `rect` parameter.
    Rect(
        /// Rectangle `[x, y, width, height]` in pixels; x/y -32768..=32768, sizes 1..=4096.
        [i32; 4],
    ),
}
impl ParameterValue {
    fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Integer(_) => "integer",
            Self::Color(_) => "color",
            Self::Fonts(_) => "fonts",
            Self::Curve(_) => "curve",
            Self::Time(_) => "time",
            Self::Rect(_) => "rect",
        }
    }
}

/// Parameter type and constraints, tagged by `type`; a null or omitted `default` makes the parameter required.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Definition {
    /// Text parameter.
    Text {
        /// Default string, checked against `max_chars`; omit or null to require a value.
        default: Option<String>,
        /// Maximum Unicode scalars, 1..=1024.
        max_chars: u16,
    },
    /// Signed 32-bit integer parameter.
    Integer {
        /// Default within `min`..=`max`; omit or null to require a value.
        default: Option<i32>,
        /// Inclusive minimum.
        min: i32,
        /// Inclusive maximum; must not be less than `min`.
        max: i32,
    },
    /// RGBA color parameter.
    Color {
        /// Default `[r, g, b, a]` bytes; omit or null to require a value.
        default: Option<[u8; 4]>,
    },
    /// Ordered font list parameter.
    Fonts {
        /// Default 1..=4 font identities; omit or null to require a value.
        default: Option<Vec<Identity>>,
    },
    /// Keyframe curve parameter.
    Curve {
        /// Default curve with 1..=128 keys; omit or null to require a value.
        default: Option<Curve>,
    },
    /// Rational time parameter.
    Time {
        /// Default rational seconds `{num, den}`; omit or null to require a value.
        default: Option<Time>,
    },
    /// Rectangle parameter.
    Rect {
        /// Default `[x, y, width, height]`; x/y -32768..=32768, sizes 1..=4096; omit or null to require a value.
        default: Option<[i32; 4]>,
    },
}
impl Definition {
    fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Integer { .. } => "integer",
            Self::Color { .. } => "color",
            Self::Fonts { .. } => "fonts",
            Self::Curve { .. } => "curve",
            Self::Time { .. } => "time",
            Self::Rect { .. } => "rect",
        }
    }
    fn default_value(&self) -> Option<ParameterValue> {
        match self {
            Self::Text { default, .. } => default.clone().map(ParameterValue::Text),
            Self::Integer { default, .. } => default.map(ParameterValue::Integer),
            Self::Color { default } => default.map(ParameterValue::Color),
            Self::Fonts { default } => default.clone().map(ParameterValue::Fonts),
            Self::Curve { default } => default.clone().map(ParameterValue::Curve),
            Self::Time { default } => default.map(ParameterValue::Time),
            Self::Rect { default } => default.map(ParameterValue::Rect),
        }
    }
    fn validate(&self) -> Result<()> {
        if matches!(self,Self::Text{max_chars,..} if !(1..=1024).contains(max_chars))
            || matches!(self,Self::Integer{min,max,..} if min>max)
        {
            return Err(invalid("Invalid parameter constraints"));
        }
        if let Some(default) = self.default_value() {
            self.check(&default)?;
        }
        Ok(())
    }
    fn check(&self, value: &ParameterValue) -> Result<()> {
        if self.kind() != value.kind() {
            return Err(error(
                "INVALID_PARAMETER",
                "Parameter value has the wrong type",
            ));
        }
        let valid = match (self, value) {
            (Self::Text { max_chars, .. }, ParameterValue::Text(s)) => {
                !s.is_empty() && s.len() <= 4096 && s.chars().count() <= *max_chars as usize
            }
            (Self::Integer { min, max, .. }, ParameterValue::Integer(n)) => {
                (*min..=*max).contains(n)
            }
            (_, ParameterValue::Fonts(fonts)) => (1..=4).contains(&fonts.len()),
            (_, ParameterValue::Time(time)) => {
                time.validate()?;
                true
            }
            (_, ParameterValue::Rect(r)) => {
                r[..2].iter().all(|n| (-32768..=32768).contains(n))
                    && r[2..].iter().all(|n| (1..=4096).contains(n))
            }
            (_, ParameterValue::Curve(c)) => {
                // Property-specific range and layer clock validation happens after binding.
                if c.keys.is_empty() || c.keys.len() > 128 {
                    false
                } else {
                    for k in &c.keys {
                        k.time.validate()?;
                    }
                    true
                }
            }
            _ => true,
        };
        if !valid {
            return Err(error(
                "INVALID_PARAMETER",
                "Parameter value exceeds its declared constraints",
            ));
        }
        Ok(())
    }
}

/// Binding target (parameter type): scene-level `scene_background` (color, alpha 255), `scene_duration` (time); layer `layer_start`, `layer_duration` (time); text `text` (text), `text_color` (color), `fonts` (fonts), `font_size`, `line_height`, `letter_spacing` (integer); shape `shape_fill`, `stroke_color` (color), `stroke_width` (integer), strokes must already exist; `rect` (rect); static `position_x`, `position_y`, `opacity` (integer); animated `position_x_curve`, `position_y_curve`, `opacity_curve` (curve).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    SceneBackground,
    SceneDuration,
    LayerStart,
    LayerDuration,
    Text,
    TextColor,
    Fonts,
    FontSize,
    LineHeight,
    LetterSpacing,
    ShapeFill,
    StrokeColor,
    StrokeWidth,
    Rect,
    PositionX,
    PositionY,
    Opacity,
    PositionXCurve,
    PositionYCurve,
    OpacityCurve,
}
impl Property {
    fn kind(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::SceneBackground | Self::TextColor | Self::ShapeFill | Self::StrokeColor => {
                "color"
            }
            Self::Fonts => "fonts",
            Self::SceneDuration | Self::LayerStart | Self::LayerDuration => "time",
            Self::PositionXCurve | Self::PositionYCurve | Self::OpacityCurve => "curve",
            Self::Rect => "rect",
            _ => "integer",
        }
    }
    fn conflict_key(self) -> Self {
        match self {
            Self::PositionXCurve => Self::PositionX,
            Self::PositionYCurve => Self::PositionY,
            Self::OpacityCurve => Self::Opacity,
            _ => self,
        }
    }
}

/// Writes a parameter value into one scene or layer property; each target may be bound only once per template.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Target property; static and curve bindings of the same property conflict.
    pub property: Property,
    /// ID of a base scene layer for layer properties; omit for `scene_background` and `scene_duration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
}
/// Named typed template input applied through one or more bindings.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// Unique name of 1..=64 ASCII letters, digits, `_` or `-`; the key in `values`.
    pub name: String,
    /// Type, constraints and optional default.
    pub definition: Definition,
    /// At least one target; every binding's property type must match the definition.
    pub bindings: Vec<Binding>,
}
/// Reusable scene template for `graphics.instantiate`: a base scene plus typed parameters and bindings.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// Template format version; must be 1.
    pub schema_version: u32,
    /// Nonblank template ID, at most 128 bytes.
    pub id: String,
    /// Base scene recipe; bound fields may hold placeholders, and layer IDs must be unique.
    pub scene: Scene,
    /// Up to 64 parameters with at most 256 bindings in total.
    pub parameters: Vec<Parameter>,
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_TEMPLATE", message)
}
fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 128
}
fn parameter_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn bind(scene: &mut Scene, binding: &Binding, value: &ParameterValue) -> Result<()> {
    use ParameterValue as V;
    use Property as P;
    if binding.property.kind() != value.kind() {
        return Err(invalid("Binding and parameter types differ"));
    }
    if matches!(binding.property, P::SceneBackground | P::SceneDuration) {
        if binding.layer.is_some() {
            return Err(invalid("Scene properties cannot name a layer"));
        }
        match value {
            V::Color(c) if c[3] == 255 => scene.background = [c[0], c[1], c[2]],
            V::Time(t) => scene.duration = *t,
            _ => return Err(invalid("Scene background must have alpha 255")),
        }
        return Ok(());
    }
    let layer = scene
        .layers
        .iter_mut()
        .find(|l| Some(&l.id) == binding.layer.as_ref())
        .ok_or_else(|| invalid("Binding names a missing layer"))?;
    match (binding.property, value) {
        (P::LayerStart, V::Time(t)) => layer.start = *t,
        (P::LayerDuration, V::Time(t)) => layer.duration = *t,
        (P::PositionX | P::PositionY | P::Opacity, V::Integer(n)) => {
            if layer
                .animation
                .as_ref()
                .is_some_and(|a| match binding.property {
                    P::PositionX => a.position_x.is_some(),
                    P::PositionY => a.position_y.is_some(),
                    _ => a.opacity.is_some(),
                })
            {
                return Err(invalid(
                    "A static property binding cannot be hidden by an existing animation curve",
                ));
            }
            match binding.property {
                P::PositionX => layer.transform.position[0] = *n,
                P::PositionY => layer.transform.position[1] = *n,
                _ => {
                    layer.transform.opacity = (*n)
                        .try_into()
                        .map_err(|_| invalid("Opacity must fit 0-255"))?
                }
            }
        }
        (P::PositionXCurve | P::PositionYCurve | P::OpacityCurve, V::Curve(c)) => {
            let a = layer.animation.get_or_insert(scene::Animation {
                position_x: None,
                position_y: None,
                opacity: None,
            });
            match binding.property {
                P::PositionXCurve => a.position_x = Some(c.clone()),
                P::PositionYCurve => a.position_y = Some(c.clone()),
                _ => a.opacity = Some(c.clone()),
            }
        }
        _ => {
            let graphic = layer
                .graphics
                .as_mut()
                .ok_or_else(|| invalid("Graphic binding requires a graphics layer"))?;
            match (graphic, binding.property, value) {
                (Graphic::Text { text, .. }, P::Text, V::Text(s)) => *text = s.clone(),
                (Graphic::Text { color, .. }, P::TextColor, V::Color(c)) => *color = *c,
                (Graphic::Text { fonts, .. }, P::Fonts, V::Fonts(f)) => *fonts = f.clone(),
                (Graphic::Text { size, .. }, P::FontSize, V::Integer(n)) => {
                    *size = (*n)
                        .try_into()
                        .map_err(|_| invalid("Font size must be nonnegative and fit u16"))?
                }
                (Graphic::Text { line_height, .. }, P::LineHeight, V::Integer(n)) => {
                    *line_height = (*n)
                        .try_into()
                        .map_err(|_| invalid("Line height must be nonnegative and fit u16"))?
                }
                (Graphic::Text { letter_spacing, .. }, P::LetterSpacing, V::Integer(n)) => {
                    *letter_spacing = (*n)
                        .try_into()
                        .map_err(|_| invalid("Letter spacing must be nonnegative and fit u16"))?
                }
                (Graphic::Shape { fill, .. }, P::ShapeFill, V::Color(c)) => *fill = *c,
                (
                    Graphic::Shape {
                        stroke: Some(stroke),
                        ..
                    },
                    P::StrokeColor,
                    V::Color(c),
                ) => stroke.color = *c,
                (
                    Graphic::Shape {
                        stroke: Some(stroke),
                        ..
                    },
                    P::StrokeWidth,
                    V::Integer(n),
                ) => {
                    stroke.width = (*n)
                        .try_into()
                        .map_err(|_| invalid("Stroke width cannot be negative"))?
                }
                (Graphic::Text { rect, .. } | Graphic::Shape { rect, .. }, P::Rect, V::Rect(r)) => {
                    *rect = *r
                }
                _ => return Err(invalid("Binding is not supported by its target graphic")),
            }
        }
    }
    Ok(())
}

/// Creates and validates a fresh concrete recipe. Does not save, render or mutate the template.
pub fn instantiate(
    template: &Template,
    instance_id: &str,
    values: &BTreeMap<String, ParameterValue>,
    input_root: &Path,
) -> Result<Value> {
    if template.schema_version != 1
        || !valid_id(&template.id)
        || !valid_id(instance_id)
        || template.parameters.len() > 64
    {
        return Err(invalid(
            "Template v1 requires bounded nonblank IDs and at most 64 parameters",
        ));
    }
    let mut layer_ids = HashSet::new();
    if template
        .scene
        .layers
        .iter()
        .any(|l| !valid_id(&l.id) || !layer_ids.insert(&l.id))
    {
        return Err(invalid("Template layers must have unique bounded IDs"));
    }
    let mut names = HashSet::new();
    let mut targets = HashSet::new();
    let mut resolved = BTreeMap::new();
    let mut defaults = Vec::new();
    for parameter in &template.parameters {
        if !parameter_name(&parameter.name)
            || !names.insert(&parameter.name)
            || parameter.bindings.is_empty()
        {
            return Err(invalid(
                "Parameters require unique names and at least one binding",
            ));
        }
        parameter.definition.validate()?;
        let value = if let Some(value) = values.get(&parameter.name) {
            value.clone()
        } else {
            defaults.push(parameter.name.clone());
            parameter.definition.default_value().ok_or_else(|| {
                error(
                    "MISSING_PARAMETER",
                    format!("Required parameter: {}", parameter.name),
                )
            })?
        };
        parameter.definition.check(&value)?;
        for binding in &parameter.bindings {
            if !targets.insert((binding.layer.clone(), binding.property.conflict_key()))
                || targets.len() > 256
            {
                return Err(invalid(
                    "Bindings must be unique, with at most 256 targets; static and animated bindings cannot target the same property",
                ));
            }
            if binding.property.kind() != parameter.definition.kind() {
                return Err(invalid("Binding and parameter types differ"));
            }
        }
        resolved.insert(parameter.name.clone(), value);
    }
    if values.keys().any(|key| !names.contains(key)) {
        return Err(error(
            "UNKNOWN_PARAMETER",
            "A supplied parameter is not declared by this template",
        ));
    }
    let mut result = template.scene.clone();
    result.id = instance_id.into();
    for parameter in &template.parameters {
        for binding in &parameter.bindings {
            bind(&mut result, binding, &resolved[&parameter.name])?;
        }
    }
    let inspection = scene::inspect(&result, input_root)?;
    defaults.sort();
    Ok(
        json!({"template_id":template.id,"template_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(template)?)),"scene_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&result)?)),"scene":result,"parameters":resolved,"defaulted_parameters":defaults,"inspection":inspection}),
    )
}

pub fn capabilities() -> Value {
    json!({"schema_version":1,"command":"graphics.instantiate","value_types":["text","integer","color","fonts","curve","time","rect"],"maximum_parameters":64,"maximum_bindings":256,"evaluation":"explicit_typed_bindings_no_scripts","instance_validation":"complete_scene_and_source_identities","mutation":"returns_fresh_scene_no_files_written"})
}
