//! Numbered, complete RGB image directories with exact timing and independent verification.
use super::*;
use std::io::{BufReader, Write};

struct Images {
    path: PathBuf,
    first: u32,
    count: u64,
    published: bool,
}
impl Drop for Images {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        // Only this invocation's fixed output names are eligible for normal failure cleanup.
        for index in 0..self.count {
            let _ = fs::remove_file(self.path.join(name(self.first as u64 + index)));
        }
        for file in ["audio.wav", "manifest.json"] {
            let _ = fs::remove_file(self.path.join(file));
        }
        let _ = fs::remove_dir(&self.path);
    }
}
fn name(number: u64) -> String {
    format!("frame-{number:06}.png")
}
fn pixels(path: &Path, width: u32, height: u32) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > 128 * 1024 * 1024 {
        return Err(verification("Encoded sequence frame is too large"));
    }
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_limits(png::Limits {
        bytes: 64 * 1024 * 1024,
    });
    let mut reader = decoder
        .read_info()
        .map_err(|e| verification(&e.to_string()))?;
    let info = reader.info();
    if info.width != width
        || info.height != height
        || info.bit_depth != png::BitDepth::Eight
        || info.color_type != png::ColorType::Rgb
        || info.animation_control.is_some()
        || info.trns.is_some()
    {
        return Err(verification(
            "Sequence frame dimensions, depth or opaque RGB format changed",
        ));
    }
    let mut bytes = vec![0; width as usize * height as usize * 3];
    let frame = reader
        .next_frame(&mut bytes)
        .map_err(|e| verification(&e.to_string()))?;
    if frame.buffer_size() != bytes.len() {
        return Err(verification("Incomplete sequence frame"));
    }
    reader.finish().map_err(|e| verification(&e.to_string()))?;
    Ok(bytes)
}

#[cfg(windows)]
fn publish_directory(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if source[..source.len() - 1].contains(&0) || destination[..destination.len() - 1].contains(&0)
    {
        return Err(error("INVALID_PATH", "Embedded NUL in directory path"));
    }
    // Same-volume rename. Neither replacement nor copy/delete fallback is authorized.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 8) } == 0 {
        return Err(error(
            "PUBLISH_FAILED",
            format!(
                "Image directory was not published: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok(())
}
#[cfg(not(windows))]
fn publish_directory(_source: &Path, _destination: &Path) -> Result<()> {
    Err(error(
        "UNSUPPORTED_PLATFORM",
        "Atomic image directory publication currently requires Windows",
    ))
}

pub(super) fn run(
    request: &Export,
    c: &Checked,
    reference: &Path,
    scratch: &Path,
) -> Result<Value> {
    let first = request.sequence_first.unwrap_or(0);
    let mut images = Images {
        path: scratch.join("images"),
        first,
        count: c.reference.frames,
        published: false,
    };
    fs::create_dir(&images.path)?;
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(reference.to_string_lossy().into_owned());
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "png",
            "-pix_fmt",
            "rgb24",
            "-threads",
            "1",
            "-start_number",
        ]
        .map(str::to_owned),
    );
    args.push(first.to_string());
    args.extend([
        "-f".into(),
        "image2".into(),
        PathBuf::from(images.path.to_string_lossy().replace('%', "%%"))
            .join("frame-%06d.png")
            .to_string_lossy()
            .into_owned(),
    ]);
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(600))?;
    let mut files = Vec::new();
    let mut decoded = Sha256::new();
    for index in 0..c.reference.frames {
        let number = first as u64 + index;
        let relative = name(number);
        let path = images.path.join(&relative);
        decoded.update(pixels(&path, c.project.width, c.project.height)?);
        File::options().write(true).open(&path)?.sync_all()?;
        files.push(json!({"number":number,"path":relative,"bytes":fs::metadata(&path)?.len(),"sha256":media::file_hash(&path)?}));
    }
    let digest = format!("{:x}", decoded.finalize());
    if digest != video_hash(reference)? {
        return Err(verification(
            "Numbered PNG pixels differ from the selected timeline",
        ));
    }
    let mut audio = Value::Null;
    if request.streams.audio() {
        let path = images.path.join("audio.wav");
        let args = vec![
            "-v".into(),
            "error".into(),
            "-xerror".into(),
            "-nostdin".into(),
            "-n".into(),
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-i".into(),
            reference.to_string_lossy().into_owned(),
            "-map".into(),
            "0:a:0".into(),
            "-vn".into(),
            "-c:a".into(),
            "pcm_s16le".into(),
            "-f".into(),
            "wav".into(),
            path.to_string_lossy().into_owned(),
        ];
        media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(600))?;
        let pcm = scratch.join("decoded.pcm");
        let original = scratch.join("reference.pcm");
        if decode_pcm(&path, &pcm)? != c.reference.samples
            || decode_pcm(reference, &original)? != c.reference.samples
            || prefix_hash(&pcm, c.reference.samples * 4)?
                != prefix_hash(&original, c.reference.samples * 4)?
        {
            return Err(verification(
                "Sequence soundtrack sample count or PCM changed",
            ));
        }
        File::options().write(true).open(&path)?.sync_all()?;
        audio = json!({"path":"audio.wav","bytes":fs::metadata(&path)?.len(),"sha256":media::file_hash(&path)?,"sample_rate":48000,"channels":2,"samples":c.reference.samples,"decoded_sha256":prefix_hash(&pcm,c.reference.samples*4)?});
    }
    if fs::read_dir(&images.path)?.count() as u64
        != c.reference.frames + request.streams.audio() as u64
    {
        return Err(verification(
            "Sequence has missing or additional output files",
        ));
    }
    let manifest = json!({"schema_version":1,"profile":"cutbolt-numbered-rgb-v1","first_number":first,"frame_count":c.reference.frames,
        "frame_rate":c.project.frame_rate,"duration":c.range.duration,"source_range":c.range,"project_revision":c.project.revision,
        "width":c.project.width,"height":c.project.height,"pixel_format":"rgb24","alpha":"opaque","transfer":request.transfer(),
        "sampling":"one_image_per_native_frame_half_open_range","decoded_rgb_sha256":digest,"frames":files,"audio":audio,"sources":c.reference.sources});
    let path = images.path.join("manifest.json");
    let mut file = File::options().write(true).create_new(true).open(&path)?;
    serde_json::to_writer_pretty(&mut file, &manifest)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    for source in &c.reference.sources {
        if media::file_hash(&source.path)? != source.sha256 {
            return Err(error("MEDIA_CHANGED", "Source changed during image export"));
        }
    }
    let mut result = report(request, c);
    result["manifest"] = json!({"path":"manifest.json","bytes":fs::metadata(&path)?.len(),"sha256":media::file_hash(&path)?});
    result["verification"] = json!({"decoded_video_sha256":digest,"decoded_audio_samples":if request.streams.audio(){c.reference.samples}else{0},"all_frames_verified":true});
    result["ffmpeg"] = json!(media::version("ffmpeg")?);
    result["ffprobe"] = json!(media::version("ffprobe")?);
    publish_directory(&images.path, &c.output)?;
    images.published = true;
    Ok(result)
}
