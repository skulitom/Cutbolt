//! Per-frame evaluation of saved source-clock pixel effects, shared with the scene processor.
use crate::{At, Result, composite::AlphaMode, effects, error, time::Time};
use serde::Serialize;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// One bounded table pool shared by every decoder branch in a timeline render.
#[derive(Clone, Default)]
pub(crate) struct Resources {
    effects: Arc<Mutex<effects::Resources>>,
    pub(crate) scenes: crate::generated::Pool,
}
impl std::fmt::Debug for Resources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipEffectResources")
            .finish_non_exhaustive()
    }
}
impl Resources {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, effects::Resources>> {
        self.effects.lock().map_err(|_| {
            error(
                "RENDER_VALIDATION_FAILED",
                "Clip effect resource pool failed",
            )
        })
    }
    pub(crate) fn add(
        &self,
        effects: &[effects::Effect],
        root: &Path,
    ) -> Result<Vec<crate::render::Source>> {
        let mut pool = self.lock()?;
        pool.add(effects, root)?;
        pool.identities()
            .map(|file| {
                Ok(crate::render::Source {
                    path: crate::media::project_file(&file.path, root)?,
                    sha256: file.sha256.clone(),
                    frames: 0,
                    samples: 0,
                    conversion: None,
                })
            })
            .collect()
    }
    pub(crate) fn report(&self) -> Result<Option<serde_json::Value>> {
        Ok(self.lock()?.report())
    }
}

/// The recipe and source coordinates of the first frame supplied by one clip decoder.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Stack {
    pub clip_id: String,
    pub effects: Vec<effects::Effect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<crate::clip_properties::Properties>,
    pub source_start: Time,
    pub source_duration: Time,
    pub rate: Time,
    /// Crop origin in decoded pixels; masks are evaluated on the original source canvas.
    pub origin: [u32; 2],
    /// Preview proxy divisor, or 1 for the original. Proxy pixel centers map into the source.
    pub coordinate_scale: u32,
}
pub(crate) struct Prepared {
    samplers: Vec<effects::Sampler>,
}
impl Stack {
    pub(crate) fn time(&self, frame: u64) -> Result<Time> {
        self.source_start.plus(Time::new(
            frame
                .checked_mul(self.rate.den)
                .ok_or_else(|| error("INVALID_TIME", "Clip property frame time overflow"))?,
            self.rate.num,
        )?)
    }
    pub(crate) fn prepare(&self) -> Result<Prepared> {
        Ok(Prepared {
            samplers: effects::prepare(&self.effects, self.source_duration)
                .under(|| format!("clip {:?}.effects", self.clip_id))?,
        })
    }
}
impl Prepared {
    /// Keep the processor's straight alpha for the ordered native layer stack.
    pub(crate) fn apply_rgba(
        &self,
        stack: &Stack,
        resources: &Resources,
        frame: u64,
        dimensions: [u32; 2],
        pixels: &mut [u8],
    ) -> Result<()> {
        let [width, height] = dimensions;
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Native RGBA effect frame size differs",
            ));
        }
        let time = stack.source_start.plus(Time::new(
            frame
                .checked_mul(stack.rate.den)
                .ok_or_else(|| error("INVALID_TIME", "Clip effect frame time overflow"))?,
            stack.rate.num,
        )?)?;
        let samples = self
            .samplers
            .iter()
            .map(|s| s.sample(time))
            .collect::<Result<Vec<_>>>()?;
        let pool = resources.lock()?;
        let Some(processor) = effects::Processor::new(&stack.effects, &samples, &pool) else {
            return Ok(());
        };
        let scale = i64::from(stack.coordinate_scale);
        if let Some(tables) = processor.tables() {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                if pixel[3] != 0 {
                    for c in 0..3 {
                        pixel[c] = tables[c][pixel[c] as usize];
                    }
                }
            }
            return Ok(());
        }
        for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let position = [
                (i as i64 % i64::from(width) + i64::from(stack.origin[0])) * scale + scale / 2,
                (i as i64 / i64::from(width) + i64::from(stack.origin[1])) * scale + scale / 2,
            ];
            if let Some(value) = processor.pixel(pixel, AlphaMode::Straight, position) {
                *pixel = value;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loaded_luts_remain_publication_dependencies_after_loading() {
        let root = std::env::temp_dir().join(format!(
            "cutbolt-clip-lut-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("look.cube");
        let original = b"LUT_1D_SIZE 2\n0 0 0\n1 1 1\n";
        std::fs::write(&path, original).unwrap();
        let effect = effects::Effect::Lut(crate::lut::Transform {
            file: crate::scene::Identity {
                path: "look.cube".into(),
                bytes: original.len() as u64,
                sha256: crate::media::file_hash(&path).unwrap(),
            },
            interpolation: crate::lut::Interpolation::Linear,
        });
        let resources = Resources::default();
        let sources = resources.add(std::slice::from_ref(&effect), &root).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!((sources[0].frames, sources[0].samples), (0, 0));
        assert_eq!(resources.clone().add(&[effect], &root).unwrap().len(), 1);
        sources[0].verify(&crate::media::Uncontrolled).unwrap();
        std::fs::write(&path, b"LUT_1D_SIZE 2\n0 0 0\n1 0 1\n").unwrap();
        let result = sources[0].verify(&crate::media::Uncontrolled);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert_eq!(result.unwrap_err().code, "MEDIA_CHANGED");
    }
}
