//! Instantiate a graphic as a pinned scene source, then attach it through one optimistic saved edit.
use crate::{
    At, Result, error,
    generated::SceneRef,
    model::{Operation, Project},
    scene::Scene,
    store::{self, prepared::Begin},
    templates::{self, ParameterValue, Template},
    time::Time,
    tracks::{Collision, Composite, Edit, Kind, Track, TrackClip},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

fn zero() -> Time {
    Time::ZERO
}
fn graphics_track() -> String {
    "graphics".into()
}

/// A revision-checked graphics placement request; the pinned scene is saved without compiling a movie.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// Existing local session-store directory.
    pub store_root: PathBuf,
    /// Project whose saved head receives the graphic.
    pub project_id: String,
    /// Stable edit request ID, unique in this project. Identical retries return its saved receipt.
    pub request_id: String,
    /// Required saved head; checked before instantiation and again at commit.
    pub expected_revision: u64,
    /// Original built-in template ID; give exactly one of template_id or template.
    pub template_id: Option<String>,
    /// Inline original template, as for graphics.instantiate.
    pub template: Option<Template>,
    /// ID for the generated scene and placed clip; must not collide with an existing clip or media asset.
    pub instance_id: String,
    /// Typed template values. Output canvas, scale 1 and frame clock default from the project.
    #[serde(default)]
    pub values: BTreeMap<String, ParameterValue>,
    /// Frame-aligned timeline start; defaults to zero. A nonempty timeline is never lengthened.
    #[serde(default = "zero")]
    pub start: Time,
    /// Destination video alpha_over track; defaults to graphics. Created above existing tracks if absent.
    #[serde(default = "graphics_track")]
    pub track_id: String,
    /// Existing absolute root containing scene inputs.
    pub input_root: PathBuf,
    /// Existing absolute directory inside input_root, retained for request/replay compatibility. No graphic movie is written.
    pub output_root: PathBuf,
}

fn edit(edit: Edit) -> Operation {
    Operation::Tracks { edit }
}

fn operations(request: &Place, project: &Project, scene: Scene) -> Result<Vec<Operation>> {
    request
        .start
        .units(project.frame_rate)
        .at(|| "start".into())?;
    let source_duration = scene.duration;
    let end = request.start.plus(source_duration)?;
    let duration = project.duration()?;
    if duration != Time::ZERO && end.compare(duration)?.is_gt() {
        return Err(error("INVALID_RANGE", "Graphic ends after the existing timeline; shorten its template duration or extend the timeline explicitly").with_path("values.duration"));
    }
    if project
        .assets
        .iter()
        .any(|asset| asset.id == request.instance_id)
    {
        return Err(error(
            "DUPLICATE_ID",
            "Graphic instance_id already names a media asset",
        )
        .with_path("instance_id"));
    }
    let mut operations = Vec::new();
    if project.tracks.is_none() {
        operations.push(if duration == Time::ZERO {
            edit(Edit::Create { duration: end })
        } else {
            edit(Edit::Promote {
                video_track_id: "graphics-source-video".into(),
                audio_track_id: "graphics-source-audio".into(),
            })
        });
    }
    if let Some(track) = project
        .tracks
        .as_ref()
        .and_then(|a| a.tracks.iter().find(|t| t.id == request.track_id))
    {
        if track.kind != Kind::Video || track.composite != Composite::AlphaOver || !track.enabled {
            return Err(error(
                "INVALID_TRACKS",
                "graphics.place requires an enabled video alpha_over track",
            )
            .with_path("track_id"));
        }
    } else {
        operations.push(edit(Edit::Add {
            track: Track {
                id: request.track_id.clone(),
                kind: Kind::Video,
                locked: false,
                enabled: true,
                clips: vec![],
                transitions: vec![],
                composite: Composite::AlphaOver,
                dynamics: None,
            },
        }));
    }
    operations.push(edit(Edit::Place {
        track_id: request.track_id.clone(),
        collision: Collision::Reject,
        clip: TrackClip {
            id: request.instance_id.clone(),
            asset_id: String::new(),
            sequence_id: None,
            scene: Some(SceneRef(Box::new(scene))),
            start: request.start,
            source_in: Time::ZERO,
            duration: source_duration,
            gain_milli: 1000,
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            gain_curve: None,
            envelope: None,
            transform: None,
            effects: Vec::new(),
        },
    }));
    Ok(operations)
}

fn response(request: &Place, receipt: store::Receipt) -> Result<Value> {
    let project = store::get(
        &request.store_root,
        &request.project_id,
        Some(receipt.revision),
    )?;
    if let Some(clip) = project
        .tracks
        .as_ref()
        .and_then(|a| a.tracks.iter().find(|t| t.id == request.track_id))
        .and_then(|t| t.clips.iter().find(|c| c.id == request.instance_id))
        && let Some(scene) = &clip.scene
    {
        return Ok(
            json!({"project_id":request.project_id,"revision":receipt.revision,"request_id":request.request_id,
            "scene_id":scene.0.id,"scene_sha256":scene.key()?,"compiled":false,
            "track_id":request.track_id,"clip_id":clip.id,"start":clip.start,"duration":clip.duration,"session":receipt}),
        );
    }
    // A retry of an older compiled placement returns that revision's original response.
    // Keep the prepared request serialization unchanged so its request-ID hash still matches.
    let asset = project.asset(&request.instance_id)?;
    Ok(
        json!({"project_id":request.project_id,"revision":receipt.revision,"request_id":request.request_id,
        "asset":asset,"track_id":request.track_id,"clip_id":request.instance_id,"start":request.start,
        "scene_sha256":asset.metadata.fields.get("graphics.scene_sha256"),"session":receipt}),
    )
}

pub fn run(request: &Place) -> Result<Value> {
    let pending = match store::prepared::begin(
        &request.store_root,
        &request.project_id,
        &request.request_id,
        request.expected_revision,
        "graphics.place",
        request,
    )? {
        Begin::Replayed(receipt) => return response(request, *receipt),
        Begin::Prepare(pending) => *pending,
    };
    let input_root = request
        .input_root
        .canonicalize()
        .map_err(crate::Error::from)
        .at(|| "input_root".into())?;
    let output_root = request
        .output_root
        .canonicalize()
        .map_err(crate::Error::from)
        .at(|| "output_root".into())?;
    if !request.input_root.is_absolute()
        || !request.output_root.is_absolute()
        || !input_root.is_dir()
        || !output_root.is_dir()
    {
        return Err(error(
            "INVALID_PATH",
            "Graphic input/output roots must be existing absolute directories",
        ));
    }
    if !output_root.starts_with(&input_root) {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            "The compatible graphic output_root must stay inside input_root",
        )
        .with_path("output_root"));
    }
    let selected = templates::select(
        request.template_id.as_deref(),
        request.template.as_ref(),
        Some(&pending.project),
        &request.values,
    )?;
    let expanded = templates::instantiate_for_project(
        &selected,
        &request.instance_id,
        &request.values,
        &input_root,
        Some(&pending.project),
    )?;
    let scene: Scene = serde_json::from_value(expanded["scene"].clone())?;
    if scene.audio.is_some() || scene.audio_mix.is_some() {
        return Err(error(
            "INVALID_ARGUMENT",
            "graphics.place places video only; use a separate audio edit for a scene soundtrack",
        )
        .with_path("template.scene.audio"));
    }
    if scene.width.checked_mul(scene.output_scale) != Some(pending.project.width)
        || scene.height.checked_mul(scene.output_scale) != Some(pending.project.height)
        || scene.clock()? != pending.project.frame_rate
    {
        return Err(error(
            "INVALID_MEDIA",
            "Graphic output dimensions and frame rate must equal the target project",
        )
        .with_path("values"));
    }
    let operations = operations(request, &pending.project, scene)?;
    let receipt = pending.commit(operations, || {})?;
    response(request, receipt)
}
