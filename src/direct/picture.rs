//! Prepared frame streams shared by library reads and the timeline compositor.
use super::{Decoded, Video};
use crate::{
    Result,
    color::{Transfer, Yuv420Converter},
    error,
    media::{self, Control},
};
use serde::Serialize;
use std::{path::PathBuf, process::Command, sync::Arc, time::Duration};

pub(crate) struct Prepared {
    pub stream: PreparedStream,
    pub receipt: Decoded,
}

pub(crate) struct PreparedStream {
    pub command: Command,
    pub program: String,
    pub converter: Yuv420Converter,
    pub width: u32,
    pub height: u32,
    pub count: u64,
}
impl PreparedStream {
    pub fn start(self, watch: media::Watch, timeout: Duration) -> Result<Frames> {
        Ok(Frames {
            decoder: media::StreamReader::spawn_with(self.command, &self.program, timeout, watch)?,
            converter: self.converter,
            width: self.width,
            height: self.height,
            remaining: self.count,
            native: vec![0; self.width as usize * self.height as usize * 3 / 2],
        })
    }
}

pub(crate) struct Frames {
    decoder: media::StreamReader,
    converter: Yuv420Converter,
    width: u32,
    height: u32,
    remaining: u64,
    native: Vec<u8>,
}
impl Frames {
    pub fn read_exact(&mut self, rgb: &mut [u8]) -> Result<()> {
        if self.remaining == 0 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Direct frame reader exhausted its indexed window",
            ));
        }
        self.decoder.read_exact(&mut self.native)?;
        self.converter
            .convert(self.width, self.height, &self.native, rgb)?;
        self.remaining -= 1;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        if self.remaining != 0 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Direct frame reader did not consume its indexed window",
            ));
        }
        self.decoder.finish()
    }
}

/// A bounded source window in a composition. Index contents stay out of plan receipts.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Picture {
    pub source: PathBuf,
    pub index: String,
    pub first: u64,
    pub count: u64,
    pub crop: [u32; 4],
    pub transfer: Transfer,
    #[serde(skip)]
    video: Arc<Video>,
}
impl Picture {
    pub fn new(
        video: Arc<Video>,
        first: u64,
        count: u64,
        crop: [u32; 4],
        transfer: Transfer,
    ) -> Self {
        Self {
            source: video.path.clone(),
            index: video.loaded.key.clone(),
            first,
            count,
            crop,
            transfer,
            video,
        }
    }
    pub fn prepare(&self, control: &dyn Control) -> Result<PreparedPicture> {
        let [x, y, w, h] = self.crop;
        if w == 0
            || h == 0
            || u64::from(x) + u64::from(w) > u64::from(self.video.width)
            || u64::from(y) + u64::from(h) > u64::from(self.video.height)
        {
            return Err(error(
                "INVALID_RANGE",
                "Direct picture crop exceeds its source",
            ));
        }
        Ok(PreparedPicture {
            stream: self
                .video
                .prepare(self.first, self.count, self.transfer, control)?
                .stream,
            crop: self.crop,
        })
    }
}
pub(crate) struct PreparedPicture {
    stream: PreparedStream,
    crop: [u32; 4],
}
impl PreparedPicture {
    pub fn start(self, watch: media::Watch, timeout: Duration) -> Result<PlanarFrames> {
        let width = self.stream.width as usize;
        let rgb = vec![0; width * self.stream.height as usize * 3];
        Ok(PlanarFrames {
            frames: self.stream.start(watch, timeout)?,
            rgb,
            width,
            crop: self.crop,
        })
    }
}
pub(crate) struct PlanarFrames {
    frames: Frames,
    rgb: Vec<u8>,
    width: usize,
    crop: [u32; 4],
}
impl PlanarFrames {
    pub fn read_exact(&mut self, planar: &mut [u8]) -> Result<()> {
        self.frames.read_exact(&mut self.rgb)?;
        let [x, y, w, h] = self.crop.map(|n| n as usize);
        let plane = w * h;
        if planar.len() != plane * 3 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Direct composition frame size differs",
            ));
        }
        for row in 0..h {
            let source = ((y + row) * self.width + x) * 3;
            for col in 0..w {
                let s = source + col * 3;
                let d = row * w + col;
                planar[d] = self.rgb[s + 1];
                planar[d + plane] = self.rgb[s + 2];
                planar[d + plane * 2] = self.rgb[s];
            }
        }
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        self.frames.finish()
    }
}
