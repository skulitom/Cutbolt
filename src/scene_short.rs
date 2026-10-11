//! Explicit, bounded authoring shorthand. Expansion creates an ordinary scene; rendering is shared.
use crate::{
    At, Result, animation::Interpolation, error, model::Project, scene::Scene, time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A complete ordinary scene or an explicitly marked shorthand recipe.
#[allow(dead_code)] // JSON preparation expands the shorthand before Request deserialization.
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub(crate) enum SceneInput {
    Full(Box<Scene>),
    Short(Box<ShortScene>),
}

/// Explicit shorthand marker; never inferred from omitted fields.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Form {
    Short,
}
/// Select the supplied project's dimensions.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ProjectSize {
    Project,
}
/// Explicit logical canvas dimensions or a target-project selection.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum Size {
    Pixels([u32; 2]),
    Project(ProjectSize),
}
/// Placement of the layer's box inside the scene's safe area.
#[derive(Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Anchor {
    #[default]
    TopLeft,
    Center,
    LowerThird,
}

/// A text/shape scene with explicit form marker, exact seconds and deterministic layout defaults.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShortScene {
    /// Required literal `short`; full scenes never receive shorthand defaults.
    form: Form,
    /// Nonblank scene/asset ID.
    id: String,
    /// Logical pixel dimensions, or `project` to use the supplied target project's canvas.
    size: Size,
    /// Exact seconds (integer, decimal, or rational); normal frame/sample alignment applies.
    duration: Time,
    /// Defaults to the supplied project's native clock, otherwise 25 fps.
    #[serde(default)]
    frame_rate: Option<Time>,
    /// Hex RGB/RGBA with opaque alpha; defaults to black.
    #[serde(default = "black")]
    background: String,
    /// Straight-alpha output; default false.
    #[serde(default)]
    transparent: bool,
    /// Inset in logical pixels on every edge; default floor(min(width,height)/20).
    #[serde(default)]
    safe_area: Option<u32>,
    /// Ordered original text/shape layers; normal scene limits apply.
    layers: Vec<ShortLayer>,
}
fn black() -> String {
    "#000000".into()
}
fn white() -> String {
    "#ffffff".into()
}
fn opaque() -> u8 {
    255
}
fn easing() -> Interpolation {
    Interpolation::EaseOut
}
fn font_size() -> u16 {
    32
}
fn zero() -> Time {
    Time::ZERO
}

/// One bounded graphic with activation, box layout and optional property animation.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShortLayer {
    /// Unique nonblank layer ID.
    id: String,
    /// Text or an original rectangle/ellipse primitive.
    content: Content,
    /// Center centers the box; top_left uses the safe inset; lower_third aligns its bottom-left.
    #[serde(default)]
    anchor: Anchor,
    /// Box size; defaults to the safe rectangle, or its bottom third for lower_third.
    #[serde(default)]
    box_size: Option<[u32; 2]>,
    /// Layer activation starts at zero unless specified.
    #[serde(default = "zero")]
    start: Time,
    /// Defaults to the remaining scene duration after start.
    #[serde(default)]
    duration: Option<Time>,
    /// Static pixel translation applied after box layout; default [0,0].
    #[serde(default)]
    position: [i32; 2],
    #[serde(default = "opaque")]
    /// Static layer opacity, 0..255; defaults to opaque.
    opacity: u8,
    /// Two-key position/opacity curves; omitted properties keep their static values.
    #[serde(default)]
    animation: Option<Animation>,
}
/// One external font file or unambiguous installed family/full name.
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum Font {
    Name(String),
    File(#[schemars(with = "crate::scene::Identity")] FontFile),
}
/// External font identity; omitted identity fields are completed by ordinary request preparation.
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct FontFile {
    /// Relative input-root path or absolute installed-font path.
    path: std::path::PathBuf,
    /// Optional expected content digest, preserved when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    /// Optional expected size, preserved when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
}
/// Original text and geometric primitives; full scenes support the additional source types.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Content {
    /// Rasterized external-font text with character wrapping and explicit overflow rejection.
    Text {
        /// Nonempty Unicode text; the selected font/layout must support it.
        text: String,
        /// Exact installed family/full name or an external font identity/path object.
        font: Font,
        #[serde(default = "font_size")]
        /// Font em size in pixels, 1..512; defaults to 32.
        size: u16,
        #[serde(default = "white")]
        /// Straight-alpha hex color; defaults to opaque white.
        color: String,
        /// Defaults to ceil(size*6/5).
        #[serde(default)]
        line_height: Option<u16>,
        /// Defaults to center for the center anchor, otherwise left.
        #[serde(default)]
        align: Option<crate::graphics::Align>,
    },
    /// Filled rectangle covering the anchored box.
    Rectangle {
        /// Straight-alpha hex fill color.
        fill: String,
    },
    /// Filled ellipse inscribed in the anchored box.
    Ellipse {
        /// Straight-alpha hex fill color.
        fill: String,
    },
}
/// Curves on the layer-local clock, with the ordinary engine's exact interpolation.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Animation {
    /// Override horizontal translation in logical pixels.
    #[serde(default)]
    position_x: Option<Transition>,
    /// Override vertical translation in logical pixels.
    #[serde(default)]
    position_y: Option<Transition>,
    /// Override layer opacity, 0..255.
    #[serde(default)]
    opacity: Option<Transition>,
}
/// One two-key curve on the layer-local clock; the first/last value holds outside this interval.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Transition {
    /// Value at the first key; holds before delay.
    from: i32,
    /// Value at the last key; holds after delay plus duration.
    to: i32,
    /// Positive exact seconds between the two keys.
    duration: Time,
    /// Layer-local time of the first key, default zero.
    #[serde(default = "zero")]
    delay: Time,
    /// hold, linear, ease_in, ease_out (default), or ease_in_out; existing quadratic interpolation.
    #[serde(default = "easing")]
    ease: Interpolation,
}
impl Transition {
    fn curve(&self) -> Result<Value> {
        if self.duration.num == 0 {
            return Err(error(
                "INVALID_ANIMATION",
                "Shorthand transition duration must be positive",
            ));
        }
        Ok(
            json!({"keys":[{"time":self.delay,"value":self.from,"interpolation":self.ease},
            {"time":self.delay.plus(self.duration)?,"value":self.to,"interpolation":"hold"}]}),
        )
    }
}

fn color(text: &str) -> Result<[u8; 4]> {
    let bytes = text.strip_prefix('#').unwrap_or_default().as_bytes();
    if ![3, 4, 6, 8].contains(&bytes.len()) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(error(
            "INVALID_SCENE",
            "Shorthand colors require #RGB, #RGBA, #RRGGBB or #RRGGBBAA",
        ));
    }
    let nibble = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b.to_ascii_lowercase() - b'a' + 10
        }
    };
    let mut result = [255; 4];
    if bytes.len() <= 4 {
        for (i, b) in bytes.iter().enumerate() {
            result[i] = nibble(*b) * 17;
        }
    } else {
        for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
            result[i] = nibble(pair[0]) * 16 + nibble(pair[1]);
        }
    }
    Ok(result)
}

/// Expand layout and syntax only. Font resolution, identity binding and full scene validation
/// follow through the ordinary command preparation and renderer, including template bindings.
pub fn expand(value: Value, project: Option<&Project>) -> Result<Value> {
    let short: ShortScene = serde_json::from_value(value).map_err(crate::Error::from)?;
    let Form::Short = short.form;
    let [width, height] = match short.size {
        Size::Pixels(size) => size,
        Size::Project(_) => project.map(|p| [p.width, p.height]).ok_or_else(|| {
            error("INVALID_SCENE", "size project needs a target project").with_path("size")
        })?,
    };
    if !(1..=crate::scene::MAX_CANVAS).contains(&width)
        || !(1..=crate::scene::MAX_CANVAS).contains(&height)
    {
        return Err(error(
            "INVALID_SCENE",
            "Shorthand canvas dimensions must be 1..4096",
        )
        .with_path("size"));
    }
    let margin = short.safe_area.unwrap_or(width.min(height) / 20);
    let twice = margin
        .checked_mul(2)
        .filter(|m| *m < width && *m < height)
        .ok_or_else(|| {
            error("INVALID_SCENE", "Safe area must leave a nonempty canvas").with_path("safe_area")
        })?;
    if short.layers.is_empty() || short.layers.len() > crate::scene::MAX_LAYERS {
        return Err(error("INVALID_SCENE", "Shorthand requires 1..64 layers").with_path("layers"));
    }
    let background = color(&short.background).at(|| "background".into())?;
    if background[3] != 255 {
        return Err(error(
            "INVALID_SCENE",
            "Background alpha must be opaque; use transparent for scene transparency",
        )
        .with_path("background"));
    }
    let mut layers = Vec::new();
    for (i, layer) in short.layers.into_iter().enumerate() {
        let expanded = (|| -> Result<Value> {
            let safe = [width-twice,height-twice];
            let [w,h] = layer.box_size.unwrap_or([safe[0], if matches!(layer.anchor, Anchor::LowerThird) { (safe[1]/3).max(1) } else { safe[1] }]);
            if w == 0 || h == 0 || w > safe[0] || h > safe[1] { return Err(error("INVALID_SCENE", "Layer box must fit the safe area").with_path("box_size")); }
            let [x,y] = match layer.anchor {
                Anchor::TopLeft => [margin,margin],
                Anchor::Center => [(width-w)/2,(height-h)/2],
                Anchor::LowerThird => [margin,height-margin-h],
            };
            let rect = [x,y,w,h];
            let graphic = match layer.content {
                Content::Text { text,font,size,color:paint,line_height,align } => json!({
                    "kind":"text","text":text,"fonts":[font],"size":size,"color":color(&paint).at(|| "content.color".into())?,
                    "rect":rect,"line_height":line_height.unwrap_or(((u32::from(size)*6).div_ceil(5)).min(u16::MAX as u32) as u16),
                    "letter_spacing":0,"align":align.unwrap_or(if matches!(layer.anchor, Anchor::Center) { crate::graphics::Align::Center } else { crate::graphics::Align::Left }),
                    "valign":if matches!(layer.anchor, Anchor::Center) {"middle"} else {"top"},"wrap":"character","overflow":"reject"}),
                Content::Rectangle { fill } => json!({"kind":"shape","shape":"rectangle","fill":color(&fill).at(|| "content.fill".into())?,"rect":rect,"stroke":null}),
                Content::Ellipse { fill } => json!({"kind":"shape","shape":"ellipse","fill":color(&fill).at(|| "content.fill".into())?,"rect":rect,"stroke":null}),
            };
            let mut result = json!({"id":layer.id,"canvas":[width,height],"start":layer.start,
                "duration":layer.duration.map_or_else(||short.duration.minus(layer.start),Ok)?,"frames":[],"graphics":graphic,
                "timing":"strict","end":"hold_last","transform":{"position":layer.position,"crop":[0,0,width,height],
                    "scale":1,"quarter_turns":0,"opacity":layer.opacity}});
            if let Some(a) = layer.animation {
                let mut animation = serde_json::Map::new();
                for (name, transition) in [("position_x",a.position_x),("position_y",a.position_y),("opacity",a.opacity)] {
                    if let Some(t) = transition { animation.insert(name.into(), t.curve().at(||format!("animation.{name}"))?); }
                }
                result["animation"] = animation.into();
            }
            Ok(result)
        })().at(||format!("layers[{i}]"))?;
        layers.push(expanded);
    }
    Ok(
        json!({"schema_version":1,"id":short.id,"width":width,"height":height,"output_scale":1,
        "duration":short.duration,"frame_rate":short.frame_rate.or(project.map(|p|p.frame_rate)).unwrap_or(Time {num:25,den:1}),
        "background":&background[..3],"color":"srgb_straight_encoded","transparent":short.transparent,"audio":null,"layers":layers}),
    )
}

pub(crate) fn is_short(request: &Value) -> bool {
    matches!(
        request["command"].as_str(),
        Some("scene.inspect" | "scene.render" | "scene.still")
    ) && request["scene"]["form"] == "short"
}
pub(crate) fn prepare(request: &mut Value) -> Result<()> {
    let path = match request["command"].as_str() {
        Some("scene.inspect" | "scene.render" | "scene.still") => "/scene",
        Some("graphics.instantiate" | "graphics.place") => "/template/scene",
        _ => return Ok(()),
    };
    if !request.pointer(path).is_some_and(|v| v["form"] == "short") {
        return Ok(());
    }
    let project: Option<Project> = if request["command"] == "graphics.place" {
        let root = request["store_root"]
            .as_str()
            .ok_or_else(|| error("INVALID_PATH", "graphics.place requires store_root"))?;
        let id = request["project_id"]
            .as_str()
            .ok_or_else(|| error("INVALID_ARGUMENT", "graphics.place requires project_id"))?;
        let rev = request["expected_revision"].as_u64().ok_or_else(|| {
            error(
                "INVALID_ARGUMENT",
                "graphics.place requires expected_revision",
            )
        })?;
        Some(crate::store::get(root.as_ref(), id, Some(rev))?)
    } else {
        request
            .get("project")
            .filter(|p| !p.is_null())
            .map(|p| serde_json::from_value(p.clone()))
            .transpose()?
    };
    let value = request.pointer_mut(path).unwrap();
    *value = expand(value.take(), project.as_ref())
        .at(|| path.trim_start_matches('/').replace('/', "."))?;
    Ok(())
}
