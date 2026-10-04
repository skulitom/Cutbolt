//! Original pure, typed property graph. No source-code evaluation or ambient state.
use crate::{Result, animation, error, scene::Scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
mod number;
pub use number::{Kind, Number, Value};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    Position,
    Opacity,
}
impl Property {
    fn kind(self) -> Kind {
        match self {
            Self::Position => Kind::Vector2,
            Self::Opacity => Kind::Scalar,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expr {
    Literal {
        value: Value,
    },
    Time {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer: Option<String>,
    },
    Base {
        layer: String,
        property: Property,
    },
    Property {
        layer: String,
        property: Property,
    },
    Link {
        node: String,
    },
    Vector {
        x: String,
        y: String,
    },
    Component {
        vector: String,
        axis: Axis,
    },
    Add {
        a: String,
        b: String,
    },
    Subtract {
        a: String,
        b: String,
    },
    Multiply {
        a: String,
        b: String,
    },
    Divide {
        a: String,
        b: String,
    },
    Modulo {
        a: String,
        b: String,
    },
    Minimum {
        a: String,
        b: String,
    },
    Maximum {
        a: String,
        b: String,
    },
    Floor {
        value: String,
    },
    Less {
        a: String,
        b: String,
    },
    Equal {
        a: String,
        b: String,
    },
    And {
        a: String,
        b: String,
    },
    Not {
        value: String,
    },
    Select {
        condition: String,
        yes: String,
        no: String,
    },
    Seeded {
        stream: u32,
        index: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub kind: Kind,
    pub expression: Expr,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub layer: String,
    pub property: Property,
    pub node: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub schema_version: u32,
    pub seed: u64,
    pub nodes: Vec<Node>,
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    pub scene: Scene,
    pub times: Vec<Time>,
}

struct Base {
    start: Time,
    duration: Time,
    position: [i32; 2],
    opacity: u8,
    x: Option<animation::Sampler>,
    y: Option<animation::Sampler>,
    alpha: Option<animation::Sampler>,
}
impl Base {
    fn sample(&self, time: Time, property: Property) -> Result<Value> {
        let local = if time.compare(self.start)?.is_lt() {
            Time::ZERO
        } else {
            let elapsed = time.minus(self.start)?;
            if elapsed.compare(self.duration)?.is_gt() {
                self.duration
            } else {
                elapsed
            }
        };
        let scalar = |curve: &Option<animation::Sampler>, fallback: i32| -> Result<Number> {
            Ok(Number::integer(
                curve
                    .as_ref()
                    .map(|c| c.sample(local))
                    .transpose()?
                    .unwrap_or(fallback),
            ))
        };
        match property {
            Property::Position => Ok(Value::Vector2([
                scalar(&self.x, self.position[0])?,
                scalar(&self.y, self.position[1])?,
            ])),
            Property::Opacity => Ok(Value::Scalar(scalar(&self.alpha, self.opacity as i32)?)),
        }
    }
}
pub(crate) struct Prepared<'a> {
    program: &'a Program,
    duration: Time,
    layers: BTreeMap<String, Base>,
    indices: BTreeMap<String, usize>,
    bindings: BTreeMap<(String, Property), usize>,
    order: Vec<usize>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Bound {
    pub layer: String,
    pub property: Property,
    pub node: String,
    pub value: Value,
    pub rounded: Vec<i32>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Sample {
    pub time: Time,
    pub values: BTreeMap<String, Value>,
    pub bindings: Vec<Bound>,
}

fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_EXPRESSION", message)
}
fn id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

impl<'a> Prepared<'a> {
    pub(crate) fn new(scene: &'a Scene) -> Result<Self> {
        let program = scene
            .expressions
            .as_ref()
            .ok_or_else(|| invalid("Scene has no property expression program"))?;
        if program.schema_version != 1
            || program.nodes.is_empty()
            || program.nodes.len() > 256
            || program.bindings.is_empty()
            || program.bindings.len() > 32
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Expression v1 requires 1..256 nodes and 1..32 property bindings",
            ));
        }
        let mut prepared = Self {
            program,
            duration: scene.duration,
            layers: BTreeMap::new(),
            indices: BTreeMap::new(),
            bindings: BTreeMap::new(),
            order: Vec::new(),
        };
        for layer in &scene.layers {
            let (x, y, alpha) = if let Some(animation) = &layer.animation {
                if animation.position_x.is_none()
                    && animation.position_y.is_none()
                    && animation.opacity.is_none()
                {
                    return Err(invalid("Empty base animation"));
                }
                (
                    animation
                        .position_x
                        .as_ref()
                        .map(|c| c.prepare(layer.duration, -32768, 32768))
                        .transpose()?,
                    animation
                        .position_y
                        .as_ref()
                        .map(|c| c.prepare(layer.duration, -32768, 32768))
                        .transpose()?,
                    animation
                        .opacity
                        .as_ref()
                        .map(|c| c.prepare(layer.duration, 0, 255))
                        .transpose()?,
                )
            } else {
                (None, None, None)
            };
            prepared.layers.insert(
                layer.id.clone(),
                Base {
                    start: layer.start,
                    duration: layer.duration,
                    position: layer.transform.position,
                    opacity: layer.transform.opacity,
                    x,
                    y,
                    alpha,
                },
            );
        }
        for (i, node) in program.nodes.iter().enumerate() {
            if !id(&node.id) || prepared.indices.insert(node.id.clone(), i).is_some() {
                return Err(invalid("Node IDs must be unique bounded labels"));
            }
        }
        for binding in &program.bindings {
            let index = prepared.index(&binding.node)?;
            if !prepared.layers.contains_key(&binding.layer)
                || program.nodes[index].kind != binding.property.kind()
            {
                return Err(error(
                    "EXPRESSION_TYPE",
                    "Binding requires an existing layer and matching property type",
                ));
            }
            if prepared
                .bindings
                .insert((binding.layer.clone(), binding.property), index)
                .is_some()
            {
                return Err(invalid("A property can have only one expression binding"));
            }
        }
        let dependencies = program
            .nodes
            .iter()
            .map(|n| prepared.dependencies(&n.expression))
            .collect::<Result<Vec<_>>>()?;
        let mut colors = vec![0u8; program.nodes.len()];
        fn visit(
            i: usize,
            depth: usize,
            edges: &[Vec<usize>],
            colors: &mut [u8],
            order: &mut Vec<usize>,
        ) -> Result<()> {
            if depth > 64 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Expression dependency depth exceeds 64",
                ));
            }
            if colors[i] == 1 {
                return Err(error(
                    "EXPRESSION_CYCLE",
                    "Property links or node references form a cycle",
                ));
            }
            if colors[i] == 2 {
                return Ok(());
            }
            colors[i] = 1;
            for &dependency in &edges[i] {
                visit(dependency, depth + 1, edges, colors, order)?;
            }
            colors[i] = 2;
            order.push(i);
            Ok(())
        }
        for i in 0..program.nodes.len() {
            visit(i, 1, &dependencies, &mut colors, &mut prepared.order)?;
        }
        let mut depths = vec![0usize; program.nodes.len()];
        for &i in &prepared.order {
            depths[i] = 1 + dependencies[i]
                .iter()
                .map(|d| depths[*d])
                .max()
                .unwrap_or(0);
            if depths[i] > 64 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Expression dependency depth exceeds 64",
                ));
            }
        }
        for node in &program.nodes {
            if prepared.infer(&node.expression)? != node.kind {
                return Err(error(
                    "EXPRESSION_TYPE",
                    format!("Declared type disagrees with expression: {}", node.id),
                ));
            }
        }
        Ok(prepared)
    }
    fn index(&self, name: &str) -> Result<usize> {
        if !id(name) {
            return Err(invalid("Node references require bounded labels"));
        }
        self.indices
            .get(name)
            .copied()
            .ok_or_else(|| invalid(format!("Unknown node: {name}")))
    }
    fn property_dependency(&self, layer: &str, property: Property) -> Result<Option<usize>> {
        if !id(layer) || !self.layers.contains_key(layer) {
            return Err(invalid("Unknown or invalid layer reference"));
        }
        Ok(self.bindings.get(&(layer.into(), property)).copied())
    }
    fn dependencies(&self, expr: &Expr) -> Result<Vec<usize>> {
        let names: Vec<&str> = match expr {
            Expr::Literal { value } => {
                value.normalized()?;
                vec![]
            }
            Expr::Time { layer } => {
                if let Some(layer) = layer {
                    self.property_dependency(layer, Property::Position)?;
                }
                vec![]
            }
            Expr::Base { layer, property } => {
                self.property_dependency(layer, *property)?;
                vec![]
            }
            Expr::Property { layer, property } => {
                return Ok(self
                    .property_dependency(layer, *property)?
                    .into_iter()
                    .collect());
            }
            Expr::Link { node } => vec![node],
            Expr::Vector { x, y } => vec![x, y],
            Expr::Component { vector, .. } => vec![vector],
            Expr::Add { a, b }
            | Expr::Subtract { a, b }
            | Expr::Multiply { a, b }
            | Expr::Divide { a, b }
            | Expr::Modulo { a, b }
            | Expr::Minimum { a, b }
            | Expr::Maximum { a, b }
            | Expr::Less { a, b }
            | Expr::Equal { a, b }
            | Expr::And { a, b } => vec![a, b],
            Expr::Floor { value } | Expr::Not { value } => vec![value],
            Expr::Select { condition, yes, no } => vec![condition, yes, no],
            Expr::Seeded { index, .. } => vec![index],
        };
        names.into_iter().map(|n| self.index(n)).collect()
    }
    fn infer(&self, expr: &Expr) -> Result<Kind> {
        let kind = |name: &str| -> Result<Kind> { Ok(self.program.nodes[self.index(name)?].kind) };
        let numeric = |k: Kind| matches!(k, Kind::Scalar | Kind::Vector2);
        let failure = || {
            error(
                "EXPRESSION_TYPE",
                "Expression operand types do not match the operator",
            )
        };
        match expr {
            Expr::Literal { value } => Ok(value.kind()),
            Expr::Time { .. } => Ok(Kind::Scalar),
            Expr::Base { property, .. } | Expr::Property { property, .. } => Ok(property.kind()),
            Expr::Link { node } => kind(node),
            Expr::Vector { x, y } => {
                if kind(x)? == Kind::Scalar && kind(y)? == Kind::Scalar {
                    Ok(Kind::Vector2)
                } else {
                    Err(failure())
                }
            }
            Expr::Component { vector, .. } => {
                if kind(vector)? == Kind::Vector2 {
                    Ok(Kind::Scalar)
                } else {
                    Err(failure())
                }
            }
            Expr::Add { a, b } | Expr::Subtract { a, b } => {
                let k = kind(a)?;
                if numeric(k) && k == kind(b)? {
                    Ok(k)
                } else {
                    Err(failure())
                }
            }
            Expr::Multiply { a, b } | Expr::Divide { a, b } => {
                let k = kind(a)?;
                if numeric(k) && kind(b)? == Kind::Scalar {
                    Ok(k)
                } else {
                    Err(failure())
                }
            }
            Expr::Modulo { a, b } | Expr::Minimum { a, b } | Expr::Maximum { a, b } => {
                if kind(a)? == Kind::Scalar && kind(b)? == Kind::Scalar {
                    Ok(Kind::Scalar)
                } else {
                    Err(failure())
                }
            }
            Expr::Floor { value } | Expr::Seeded { index: value, .. } => {
                if kind(value)? == Kind::Scalar {
                    Ok(Kind::Scalar)
                } else {
                    Err(failure())
                }
            }
            Expr::Less { a, b } => {
                if kind(a)? == Kind::Scalar && kind(b)? == Kind::Scalar {
                    Ok(Kind::Boolean)
                } else {
                    Err(failure())
                }
            }
            Expr::Equal { a, b } => {
                if kind(a)? == kind(b)? {
                    Ok(Kind::Boolean)
                } else {
                    Err(failure())
                }
            }
            Expr::And { a, b } => {
                if kind(a)? == Kind::Boolean && kind(b)? == Kind::Boolean {
                    Ok(Kind::Boolean)
                } else {
                    Err(failure())
                }
            }
            Expr::Not { value } => {
                if kind(value)? == Kind::Boolean {
                    Ok(Kind::Boolean)
                } else {
                    Err(failure())
                }
            }
            Expr::Select { condition, yes, no } => {
                let k = kind(yes)?;
                if kind(condition)? == Kind::Boolean && k == kind(no)? {
                    Ok(k)
                } else {
                    Err(failure())
                }
            }
        }
    }
    pub(crate) fn sample(&self, time: Time) -> Result<Sample> {
        time.validate()?;
        if time.compare(self.duration)?.is_gt() {
            return Err(invalid(
                "Sample time must be inside the scene, including its final endpoint",
            ));
        }
        let time = Time::new(time.num, time.den)?;
        let seconds = Number::make(time.num as i128, time.den as u128)?;
        let mut values: Vec<Option<Value>> = vec![None; self.program.nodes.len()];
        for &i in &self.order {
            let get = |name: &str| -> Result<&Value> {
                values[self.index(name)?]
                    .as_ref()
                    .ok_or_else(|| invalid("Dependency was not evaluated"))
            };
            let value = match &self.program.nodes[i].expression {
                Expr::Literal { value } => value.normalized()?,
                Expr::Time { layer } => Value::Scalar(if let Some(layer) = layer {
                    let start = self.layers[layer].start;
                    seconds.sub(Number::make(start.num as i128, start.den as u128)?)?
                } else {
                    seconds
                }),
                Expr::Base { layer, property } => self.layers[layer].sample(time, *property)?,
                Expr::Property { layer, property } => {
                    if let Some(index) = self.bindings.get(&(layer.clone(), *property)) {
                        values[*index]
                            .as_ref()
                            .expect("topological property dependency")
                            .clone()
                    } else {
                        self.layers[layer].sample(time, *property)?
                    }
                }
                Expr::Link { node } => get(node)?.clone(),
                Expr::Vector { x, y } => Value::Vector2([get(x)?.scalar()?, get(y)?.scalar()?]),
                Expr::Component { vector, axis } => Value::Scalar(
                    get(vector)?.vector()?[match axis {
                        Axis::X => 0,
                        Axis::Y => 1,
                    }],
                ),
                Expr::Add { a, b } => get(a)?.pair(get(b)?, Number::add)?,
                Expr::Subtract { a, b } => get(a)?.pair(get(b)?, Number::sub)?,
                Expr::Multiply { a, b } => get(a)?.scaled(get(b)?.scalar()?, Number::mul)?,
                Expr::Divide { a, b } => get(a)?.scaled(get(b)?.scalar()?, Number::div)?,
                Expr::Modulo { a, b } => {
                    Value::Scalar(get(a)?.scalar()?.modulo(get(b)?.scalar()?)?)
                }
                Expr::Minimum { a, b } | Expr::Maximum { a, b } => {
                    let a = get(a)?.scalar()?;
                    let b = get(b)?.scalar()?;
                    let minimum = matches!(&self.program.nodes[i].expression, Expr::Minimum { .. });
                    Value::Scalar(if a.less(b) == minimum { a } else { b })
                }
                Expr::Floor { value } => Value::Scalar(get(value)?.scalar()?.floor()?),
                Expr::Less { a, b } => Value::Boolean(get(a)?.scalar()?.less(get(b)?.scalar()?)),
                Expr::Equal { a, b } => Value::Boolean(get(a)? == get(b)?),
                Expr::And { a, b } => Value::Boolean(get(a)?.boolean()? && get(b)?.boolean()?),
                Expr::Not { value } => Value::Boolean(!get(value)?.boolean()?),
                Expr::Select { condition, yes, no } => {
                    get(if get(condition)?.boolean()? { yes } else { no })?.clone()
                }
                Expr::Seeded { stream, index } => {
                    let index = get(index)?.scalar()?;
                    if index.den != 1 || !(0..=u32::MAX as i64).contains(&index.num) {
                        return Err(error(
                            "EXPRESSION_DOMAIN",
                            "Seeded index must be an integer in 0..4294967295",
                        ));
                    }
                    let mut digest = Sha256::new();
                    digest.update(b"cutbolt-property-seed-v1\0");
                    digest.update(self.program.seed.to_le_bytes());
                    digest.update(stream.to_le_bytes());
                    digest.update((index.num as u32).to_le_bytes());
                    let bytes = digest.finalize();
                    let value = u32::from_le_bytes(bytes[..4].try_into().expect("four hash bytes"));
                    Value::Scalar(Number::make(value as i128, 1u128 << 32)?)
                }
            };
            values[i] = Some(value);
        }
        let mut bindings = Vec::new();
        for binding in &self.program.bindings {
            let value = values[self.indices[&binding.node]]
                .as_ref()
                .expect("all nodes evaluated")
                .clone();
            let rounded = match binding.property {
                Property::Position => value
                    .vector()?
                    .into_iter()
                    .map(|n| n.rounded(-32768, 32768))
                    .collect::<Result<Vec<_>>>()?,
                Property::Opacity => vec![value.scalar()?.rounded(0, 255)?],
            };
            bindings.push(Bound {
                layer: binding.layer.clone(),
                property: binding.property,
                node: binding.node.clone(),
                value,
                rounded,
            });
        }
        Ok(Sample {
            time,
            values: self
                .program
                .nodes
                .iter()
                .enumerate()
                .map(|(i, node)| {
                    (
                        node.id.clone(),
                        values[i].take().expect("all nodes evaluated"),
                    )
                })
                .collect(),
            bindings,
        })
    }
    pub(crate) fn check_work(&self, samples: usize) -> Result<()> {
        if samples == 0 || samples > 256 || samples * self.program.nodes.len() > 65_536 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Expression evaluation requires 1..256 samples and at most 65536 node evaluations",
            ));
        }
        Ok(())
    }
    pub(crate) fn check_render_work(&self, samples: usize) -> Result<()> {
        if samples == 0 || samples > 8000 || samples * self.program.nodes.len() > 65_536 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Scene property evaluation permits at most 8000 temporal slots and 65536 node evaluations",
            ));
        }
        Ok(())
    }
}

pub fn inspect(request: &Inspect) -> Result<Json> {
    request.scene.validate()?;
    let prepared = Prepared::new(&request.scene)?;
    prepared.check_work(request.times.len())?;
    let samples = request
        .times
        .iter()
        .map(|t| prepared.sample(*t))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"profile":"typed-property-graph-v1","scene_id":request.scene.id,"program":request.scene.expressions,
        "samples":samples,"node_evaluations":request.times.len()*prepared.program.nodes.len(),"reads_media":false,"writes_files":false,
        "base_clock":"layer_local_clamped_to_active_interval","expression_clock":"scene_or_signed_unclamped_layer_seconds",
        "bound_rounding":"nearest_ties_away_from_zero_after_exact_range_check"}),
    )
}

pub fn capabilities() -> Json {
    json!({"profile":"typed-property-graph-v1","maximum_nodes":256,"maximum_bindings":32,"maximum_dependency_depth":64,
        "maximum_samples":256,"maximum_node_evaluations":65536,"types":["scalar","vector2","boolean"],
        "bound_properties":["position","opacity"],"numeric":"checked_exact_signed_rationals",
        "maximum_absolute_reduced_numerator":9007199254740991u64,"maximum_reduced_denominator":1000000000000u64,
        "seeded":"sha256_domain_seed_stream_integer_index;first_u32_le_divided_by_2_pow_32",
        "evaluation":"all_nodes_all_branches;pure_random_access","code_execution":false,"network":false})
}
