//! Instantiate and compile a graphic, then attach it through one optimistic saved edit.
use crate::{
    At, Result, error,
    model::{Asset, Operation, Project},
    scene::{self, Scene},
    store::{self, prepared::Begin},
    templates::{self, ParameterValue, Template},
    time::Time,
    tracks::{Collision, Composite, Edit, Kind, Track, TrackClip},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

fn zero() -> Time {
    Time::ZERO
}
fn graphics_track() -> String {
    "graphics".into()
}

/// A revision-checked graphics placement request; compilation precedes the atomic saved edit.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// Existing local session-store directory.
    pub store_root: PathBuf,
    /// Project whose saved head receives the graphic.
    pub project_id: String,
    /// Stable edit request ID, unique in this project. Identical retries return its saved receipt.
    pub request_id: String,
    /// Required saved head; checked before rendering and again at commit.
    pub expected_revision: u64,
    /// Original built-in template ID; give exactly one of template_id or template.
    pub template_id: Option<String>,
    /// Inline original template, as for graphics.instantiate.
    pub template: Option<Template>,
    /// ID for the generated asset and placed clip, unused in both namespaces.
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
    /// Existing absolute root containing scene inputs and the generated asset directory.
    pub input_root: PathBuf,
    /// Existing absolute directory inside input_root. A uniquely owned graphic folder is created here.
    pub output_root: PathBuf,
}

/// A successful edit owns this directory durably. Before commit, only our file and empty
/// directory are removed on failure; unrelated files and completed project assets are never swept.
struct Artifact {
    directory: PathBuf,
    keep: bool,
}
impl Artifact {
    fn new(root: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..128 {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| error("IO_ERROR", "System clock precedes the epoch"))?
                .as_nanos();
            let directory = root.join(format!(
                ".cutbolt-graphic-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        directory,
                        keep: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(error(
            "IO_ERROR",
            "Could not allocate a unique generated-graphic directory",
        ))
    }
    fn file(&self) -> PathBuf {
        self.directory.join("graphic.mkv")
    }
}
impl Drop for Artifact {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(self.file());
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn edit(edit: Edit) -> Operation {
    Operation::Tracks { edit }
}

fn operations(request: &Place, project: &Project, asset: Asset) -> Result<Vec<Operation>> {
    request
        .start
        .units(project.frame_rate)
        .at(|| "start".into())?;
    let end = request.start.plus(asset.duration)?;
    let duration = project.duration()?;
    if duration != Time::ZERO && end.compare(duration)?.is_gt() {
        return Err(error("INVALID_RANGE", "Graphic ends after the existing timeline; shorten its template duration or extend the timeline explicitly").with_path("values.duration"));
    }
    let mut operations = vec![Operation::AddMedia {
        asset: asset.clone(),
        input_root: None,
    }];
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
            asset_id: asset.id,
            sequence_id: None,
            start: request.start,
            source_in: Time::ZERO,
            duration: asset.duration,
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
            "Generated graphics must stay inside input_root so the saved timeline can render them",
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
    let mut asset: Asset = serde_json::from_value(json!({"id":request.instance_id,
        "path":output_root.join("graphic.mkv"),"duration":scene.duration}))?;
    asset.metadata.fields.insert(
        "graphics.scene_sha256".into(),
        expanded["scene_sha256"].as_str().unwrap().into(),
    );
    asset.metadata.fields.insert(
        "graphics.template_sha256".into(),
        expanded["template_sha256"].as_str().unwrap().into(),
    );
    // A pure dry run catches collisions, locks, clock errors and bounds before creating a file.
    pending.project.apply(
        pending.project.revision,
        operations(request, &pending.project, asset.clone())?,
    )?;
    let mut artifact = Artifact::new(&output_root)?;
    let compiled = scene::run(&scene, &input_root, &artifact.directory, &artifact.file())?;
    asset.path = artifact.file().to_string_lossy().into_owned();
    asset.identity = Some(serde_json::from_value(
        compiled["asset"]["identity"].clone(),
    )?);
    let operations = operations(request, &pending.project, asset)?;
    let receipt = pending.commit(operations, || artifact.keep = true)?;
    response(request, receipt)
}
