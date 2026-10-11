//! Indexed compressed video decoding for local library clients. Frames are streamed as packed
//! RGB; no intermediate media is created. Timeline integration is a separate rendering layer.
use crate::{
    Result,
    color::{Input, Matrix, MissingTags, Range, Transfer, Yuv420Converter},
    error,
    media::{self, Control},
    registry::Identity,
    render::clock,
    scene, source_index,
    time::{Stamp, Time},
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// A verified compressed source and its presentation index. The source must remain in place
/// while reading. Every read checks its full content identity before and after decoding.
pub struct Video {
    path: PathBuf,
    loaded: source_index::Loaded,
    input: Input,
    width: u32,
    height: u32,
    rate: Time,
}

/// Evidence for one successfully decoded range. Frame indices refer to presentation order,
/// starting at zero even when the container's first timestamp is nonzero.
#[derive(Debug, Serialize)]
pub struct Decoded {
    pub index: String,
    pub first: u64,
    pub frames: u64,
    pub seek_frame: u64,
    pub seek_seconds: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: Time,
    pub transfer: Transfer,
}

impl Video {
    /// Open a content-pinned file under an explicit input root. Only the qualified 8-bit SDR
    /// CFR H.264/HEVC profile is accepted; other sources fail with their index reasons.
    pub fn open(root: &Path, file: &scene::Identity, control: &dyn Control) -> Result<Self> {
        control.check()?;
        let path = scene::identity_file(file, root, 9_007_199_254_740_991)?;
        let metadata = media::probe_controlled(&path, control)?;
        let loaded = source_index::load(
            &path,
            Identity {
                sha256: file.sha256.clone(),
                bytes: file.bytes,
            },
            &metadata,
            control,
        )?;
        if !loaded.index.reasons.is_empty() {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                format!("Direct video decoding: {}", loaded.index.reasons.join(", ")),
            ));
        }
        let video = &loaded.index.video;
        let width = video["width"].as_u64().unwrap_or(0);
        let height = video["height"].as_u64().unwrap_or(0);
        if width == 0 || height == 0 || width > 4096 || height > 4096 || width * height > 8_000_000
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Direct video requires dimensions up to 4096 and at most 8000000 pixels",
            ));
        }
        let input = Input {
            matrix: Matrix::Bt709,
            range: if video["color_range"] == "pc" {
                Range::Full
            } else {
                Range::Limited
            },
            transfer: if video["color_transfer"] == "bt709" {
                Transfer::Bt709
            } else {
                Transfer::Srgb
            },
            missing_tags: MissingTags::Reject,
        };
        // Recheck the color contract independently of qualification before allocating a table.
        input.inspect(video, true, true)?;
        let rate = loaded.index.rate.ok_or_else(|| {
            error(
                "UNSUPPORTED_MEDIA",
                "Direct video requires a native frame rate",
            )
        })?;
        Ok(Self {
            path,
            loaded,
            input,
            width: width as u32,
            height: height as u32,
            rate,
        })
    }

    pub fn frames(&self) -> u64 {
        self.loaded.index.frames.len() as u64
    }

    fn verify(&self, control: &dyn Control) -> Result<()> {
        control.check()?;
        let expected = &self.loaded.index.source;
        if fs::metadata(&self.path)?.len() != expected.bytes
            || media::file_hash_controlled(&self.path, control)? != expected.sha256
            || fs::metadata(&self.path)?.len() != expected.bytes
        {
            return Err(error(
                "MEDIA_CHANGED",
                "Direct video source changed after it was indexed",
            ));
        }
        Ok(())
    }

    /// Read exactly `[first, first + count)` in presentation order. The callback receives a
    /// borrowed RGB24 frame and its source frame index. Its output is provisional until this
    /// method succeeds: callers must publish only after decoder and final identity checks pass.
    pub fn read(
        &self,
        first: u64,
        count: u64,
        output: Transfer,
        control: &dyn Control,
        mut consume: impl FnMut(u64, &[u8]) -> Result<()>,
    ) -> Result<Decoded> {
        let end = first
            .checked_add(count)
            .filter(|&n| count > 0 && n <= self.frames())
            .ok_or_else(|| {
                error(
                    "INVALID_RANGE",
                    "Direct video range must contain frames inside the indexed source",
                )
            })?;
        self.verify(control)?;
        let index = &self.loaded.index;
        let seek = seek_frame(&index.frames, first as usize);
        let mut args: Vec<String> = [
            "-v",
            "error",
            "-nostdin",
            "-xerror",
            "-copyts",
            "-seek_timestamp",
            "1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let seek_args = clock::seek_input_at(
            self.rate,
            seek as u64,
            Stamp::ticks(index.frames[0].0, index.time_base)?,
        )?;
        let seek_seconds = seek_args.get(1).cloned();
        args.extend(seek_args);
        args.extend(
            ["-noautorotate", "-protocol_whitelist", "file,pipe", "-i"]
                .into_iter()
                .map(str::to_owned),
        );
        args.push(self.path.to_string_lossy().into_owned());
        let stop = if end < self.frames() {
            index.frames[end as usize].0
        } else {
            index
                .frames
                .last()
                .expect("indexed frame")
                .0
                .checked_add(1)
                .ok_or_else(|| {
                    error(
                        "UNSUPPORTED_MEDIA",
                        "Frame timestamp exceeds the supported clock",
                    )
                })?
        };
        let filter = format!(
            "settb=expr={}/{},trim=start_pts={}:end_pts={stop}",
            index.time_base.num, index.time_base.den, index.frames[first as usize].0
        );
        args.extend(
            [
                "-map",
                "0:v:0",
                "-an",
                "-vf",
                &filter,
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                index.video["pix_fmt"].as_str().expect("qualified format"),
                "-f",
                "rawvideo",
                "pipe:1",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        // No output -frames limit: require trim to produce exactly the indexed range and detect
        // both missing and excess frames instead of hiding an unexpected trailing frame.
        let converter = Yuv420Converter::new(self.input, output)?;
        let program = control.tool("ffmpeg");
        let mut decoder = media::StreamReader::spawn_with(
            control.command(&program, &args)?,
            &program,
            Duration::from_secs(1800),
            media::Watch::default(),
        )?;
        let pixels = self.width as usize * self.height as usize;
        let mut native = vec![0; pixels * 3 / 2];
        let mut rgb = vec![0; pixels * 3];
        for frame in first..end {
            control.check()?;
            decoder.read_exact(&mut native)?;
            converter.convert(self.width, self.height, &native, &mut rgb)?;
            consume(frame, &rgb)?;
        }
        decoder.finish()?;
        self.verify(control)?;
        Ok(Decoded {
            index: self.loaded.key.clone(),
            first,
            frames: count,
            seek_frame: seek as u64,
            seek_seconds,
            width: self.width,
            height: self.height,
            frame_rate: self.rate,
            transfer: output,
        })
    }
}

fn seek_frame(frames: &[source_index::Frame], first: usize) -> usize {
    let keys: Vec<_> = frames[..=first]
        .iter()
        .enumerate()
        .filter_map(|(n, f)| f.1.then_some(n))
        .collect();
    // B pictures can be leading pictures of an open GOP (including HEVC RASL). The index
    // cannot prove those references are closed, so retain a whole preceding GOP as preroll.
    let preceding = usize::from(frames.iter().any(|f| f.2 == b'B'));
    keys[keys.len().saturating_sub(1 + preceding)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn b_pictures_keep_previous_gop_and_early_ranges_keep_the_origin() {
        let mut frames: Vec<_> = (0..200)
            .map(|n| source_index::Frame(n, n % 60 == 0, b'P', false))
            .collect();
        assert_eq!(seek_frame(&frames, 150), 120);
        frames[1].2 = b'B';
        assert_eq!(seek_frame(&frames, 150), 60);
        assert_eq!(seek_frame(&frames, 199), 120);
        assert_eq!(seek_frame(&frames, 60), 0);
        assert_eq!(seek_frame(&frames, 0), 0);
    }
}
