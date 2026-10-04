//! Original bounded PCM16 RIFF reader/writer with explicit speaker assignments.
use crate::{Result, error};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum Layout {
    #[serde(rename = "mono")]
    Mono,
    #[default]
    #[serde(rename = "stereo")]
    Stereo,
    #[serde(rename = "quad")]
    Quad,
    #[serde(rename = "5.1")]
    Surround51,
    #[serde(rename = "5.1(side)")]
    Surround51Side,
    #[serde(rename = "7.1")]
    Surround71,
}
impl Layout {
    pub fn speakers(self) -> &'static [&'static str] {
        match self {
            Self::Mono => &["FC"],
            Self::Stereo => &["FL", "FR"],
            Self::Quad => &["FL", "FR", "BL", "BR"],
            Self::Surround51 => &["FL", "FR", "FC", "LFE", "BL", "BR"],
            Self::Surround51Side => &["FL", "FR", "FC", "LFE", "SL", "SR"],
            Self::Surround71 => &["FL", "FR", "FC", "LFE", "BL", "BR", "SL", "SR"],
        }
    }
    pub fn channels(self) -> usize {
        self.speakers().len()
    }
    pub fn mask(self) -> u32 {
        match self {
            Self::Mono => 4,
            Self::Stereo => 3,
            Self::Quad => 0x33,
            Self::Surround51 => 0x3f,
            Self::Surround51Side => 0x60f,
            Self::Surround71 => 0x63f,
        }
    }
    fn from_mask(mask: u32) -> Result<Self> {
        [
            Self::Mono,
            Self::Stereo,
            Self::Quad,
            Self::Surround51,
            Self::Surround51Side,
            Self::Surround71,
        ]
        .into_iter()
        .find(|l| l.mask() == mask)
        .ok_or_else(|| invalid("Unsupported or ambiguous speaker mask"))
    }
}
const PCM_GUID: [u8; 16] = [1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113];
fn invalid(message: &str) -> crate::Error {
    error("UNSUPPORTED_AUDIO", message)
}
fn u16_at(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().expect("checked format length"))
}
fn u32_at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().expect("checked format length"))
}
pub(crate) struct Decoded {
    pub data: Vec<i16>,
    pub rate: u32,
    pub layout: Layout,
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded> {
    if bytes.len() < 12
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || u32_at(bytes, 4) as u64 + 8 != bytes.len() as u64
    {
        return Err(invalid("Expected a complete bounded RIFF WAVE file"));
    }
    let mut pos = 12;
    let (mut format, mut samples) = (None, None);
    while pos < bytes.len() {
        let header = bytes
            .get(pos..pos + 8)
            .ok_or_else(|| invalid("Incomplete WAV chunk header"))?;
        let size = u32_at(header, 4) as usize;
        let end = pos
            .checked_add(8)
            .and_then(|p| p.checked_add(size))
            .ok_or_else(|| invalid("WAV chunk size overflow"))?;
        let body = bytes
            .get(pos + 8..end)
            .ok_or_else(|| invalid("Truncated WAV chunk"))?;
        let slot = match &header[..4] {
            b"fmt " => Some(&mut format),
            b"data" => Some(&mut samples),
            _ => None,
        };
        if let Some(slot) = slot
            && slot.replace(body).is_some()
        {
            return Err(invalid("Duplicate WAV format or samples"));
        }
        pos = end + size % 2;
        if pos > bytes.len() {
            return Err(invalid("Missing WAV chunk padding"));
        }
    }
    let f = format.ok_or_else(|| invalid("Missing WAV format"))?;
    let samples = samples.ok_or_else(|| invalid("Missing WAV samples"))?;
    if f.len() < 16 {
        return Err(invalid("Incomplete WAV format"));
    }
    let channels = u16_at(f, 2) as usize;
    let layout = match u16_at(f, 0) {
        1 if f.len() == 16 || (f.len() == 18 && u16_at(f, 16) == 0) => match channels {
            1 => Layout::Mono,
            2 => Layout::Stereo,
            _ => {
                return Err(invalid(
                    "Multichannel PCM requires an explicit extensible speaker mask",
                ));
            }
        },
        65534
            if f.len() == 40
                && u16_at(f, 16) == 22
                && u16_at(f, 18) == 16
                && f[24..40] == PCM_GUID =>
        {
            Layout::from_mask(u32_at(f, 20))?
        }
        _ => {
            return Err(invalid(
                "Only classic or declared extensible PCM16 is supported",
            ));
        }
    };
    let rate = u32_at(f, 4);
    if channels != layout.channels()
        || u16_at(f, 14) != 16
        || ![24000, 44100, 48000].contains(&rate)
        || u16_at(f, 12) as usize != channels * 2
        || u32_at(f, 8) != rate * channels as u32 * 2
        || samples.is_empty()
        || !samples.len().is_multiple_of(channels * 2)
    {
        return Err(invalid(
            "WAV rate, precision, layout, alignment or sample count is unsupported",
        ));
    }
    Ok(Decoded {
        data: samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| i16::from_le_bytes(*s))
            .collect(),
        rate,
        layout,
    })
}
pub(crate) fn write(path: &Path, layout: Layout, samples: &[i16]) -> Result<()> {
    if samples.is_empty()
        || !samples.len().is_multiple_of(layout.channels())
        || samples.len() > 48000 * 60 * 8
    {
        return Err(invalid("Output PCM exceeds the supported sample bounds"));
    }
    let size = samples.len() as u32 * 2;
    let mut output = BufWriter::new(File::create_new(path)?);
    output.write_all(b"RIFF")?;
    output.write_all(&(size + 60).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&40u32.to_le_bytes())?;
    output.write_all(&65534u16.to_le_bytes())?;
    output.write_all(&(layout.channels() as u16).to_le_bytes())?;
    output.write_all(&48000u32.to_le_bytes())?;
    output.write_all(&(48000 * layout.channels() as u32 * 2).to_le_bytes())?;
    output.write_all(&(layout.channels() as u16 * 2).to_le_bytes())?;
    output.write_all(&16u16.to_le_bytes())?;
    output.write_all(&22u16.to_le_bytes())?;
    output.write_all(&16u16.to_le_bytes())?;
    output.write_all(&layout.mask().to_le_bytes())?;
    output.write_all(&PCM_GUID)?;
    output.write_all(b"data")?;
    output.write_all(&size.to_le_bytes())?;
    for sample in samples {
        output.write_all(&sample.to_le_bytes())?;
    }
    output.flush()?;
    Ok(())
}
