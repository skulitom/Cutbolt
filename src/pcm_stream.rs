//! Original streaming inspection/writing for long stereo timeline PCM files.
use crate::{Result, error, media};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) const MAX_FRAMES: u64 = 48_000 * 7_200;
const MAX_BYTES: u64 = MAX_FRAMES * 4 + 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Info {
    pub frames: u64,
    pub bytes: u64,
    pub sha256: String,
    pub pcm_sha256: String,
}
fn bad(message: &str) -> crate::Error {
    error("UNSUPPORTED_AUDIO", message)
}
fn io(e: std::io::Error) -> crate::Error {
    error("AUDIO_IO", e.to_string())
}
fn word(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(b[i..i + 2].try_into().expect("bounded format"))
}
fn dword(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().expect("bounded format"))
}
fn hash_open_file(file: &mut File, control: &dyn media::Control) -> Result<String> {
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        control.check()?;
        let n = file.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn inspect(path: &Path, control: &dyn media::Control) -> Result<Info> {
    control.check()?;
    let mut file = File::open(path).map_err(io)?;
    let bytes = file.metadata().map_err(io)?.len();
    if !(44..=MAX_BYTES).contains(&bytes) {
        return Err(bad(
            "Stereo timeline WAV requires 1 sample to two hours and bounded metadata",
        ));
    }
    let before = hash_open_file(&mut file, control)?;
    let mut header = [0; 12];
    file.read_exact(&mut header).map_err(io)?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" || dword(&header, 4) as u64 + 8 != bytes {
        return Err(bad("Expected complete RIFF WAVE"));
    }
    let (mut pos, mut format, mut data, mut count) = (12u64, false, None, 0);
    while pos < bytes {
        control.check()?;
        count += 1;
        if count > 128 || bytes - pos < 8 {
            return Err(bad("Invalid or excessive WAV chunks"));
        }
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk).map_err(io)?;
        let size = dword(&chunk, 4) as u64;
        let end = pos + 8 + size + size % 2;
        if end > bytes {
            return Err(bad("Truncated WAV chunk or padding"));
        }
        match &chunk[..4] {
            b"fmt " => {
                if format || ![16, 18, 40].contains(&size) {
                    return Err(bad("Duplicate or unsupported WAV format"));
                }
                let mut fmt = [0; 40];
                file.read_exact(&mut fmt[..size as usize]).map_err(io)?;
                let tag = word(&fmt, 0);
                let classic = tag == 1 && (size == 16 || size == 18 && word(&fmt, 16) == 0);
                let extensible = tag == 65534
                    && size == 40
                    && word(&fmt, 16) == 22
                    && word(&fmt, 18) == 16
                    && dword(&fmt, 20) == 3
                    && fmt[24..40] == [1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113];
                if !(classic || extensible)
                    || word(&fmt, 2) != 2
                    || dword(&fmt, 4) != 48_000
                    || dword(&fmt, 8) != 192_000
                    || word(&fmt, 12) != 4
                    || word(&fmt, 14) != 16
                {
                    return Err(bad(
                        "Timeline WAV requires 48 kHz stereo PCM16 with explicit stereo layout",
                    ));
                }
                format = true;
            }
            b"data" => {
                if data.is_some() || size == 0 || !size.is_multiple_of(4) || size / 4 > MAX_FRAMES {
                    return Err(bad("Invalid or duplicate stereo PCM data"));
                }
                data = Some((pos + 8, size));
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(end)).map_err(io)?;
        pos = end;
    }
    let (offset, length) = data.ok_or_else(|| bad("Missing PCM data"))?;
    if bytes - length > 1024 * 1024 {
        return Err(bad("WAV metadata exceeds one MiB"));
    }
    if !format {
        return Err(bad("Missing PCM format"));
    }
    file.seek(SeekFrom::Start(offset)).map_err(io)?;
    let mut remaining = length;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    while remaining > 0 {
        control.check()?;
        let n = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n]).map_err(io)?;
        digest.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if file.metadata().map_err(io)?.len() != bytes
        || hash_open_file(&mut file, control)? != before
        || media::file_hash_controlled(path, control)? != before
    {
        return Err(error("MEDIA_CHANGED", "WAV changed during inspection"));
    }
    Ok(Info {
        frames: length / 4,
        bytes,
        sha256: before,
        pcm_sha256: format!("{:x}", digest.finalize()),
    })
}

pub(crate) struct Writer<W: Write + Seek> {
    file: W,
    target: u64,
    frames: u64,
    digest: Sha256,
}
impl Writer<File> {
    pub fn create(path: &Path, target: u64) -> Result<Self> {
        if target == 0 || target > MAX_FRAMES {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Recording requires 1 sample to two hours",
            ));
        }
        Self::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(io)?,
            target,
        )
    }
    pub fn finish_file(self) -> Result<String> {
        let (file, digest) = self.finish()?;
        file.sync_all().map_err(io)?;
        Ok(digest)
    }
}
impl<W: Write + Seek> Writer<W> {
    fn new(mut file: W, target: u64) -> Result<Self> {
        if target == 0 || target > MAX_FRAMES {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Recording requires 1 sample to two hours",
            ));
        }
        file.write_all(&[0; 44]).map_err(io)?;
        Ok(Self {
            file,
            target,
            frames: 0,
            digest: Sha256::new(),
        })
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if !bytes.len().is_multiple_of(4) || self.frames + bytes.len() as u64 / 4 > self.target {
            return Err(error(
                "INVALID_CAPTURE_PACKET",
                "PCM exceeds requested recording length or stereo sample alignment",
            ));
        }
        self.file.write_all(bytes).map_err(io)?;
        self.digest.update(bytes);
        self.frames += bytes.len() as u64 / 4;
        Ok(())
    }
    fn finish(mut self) -> Result<(W, String)> {
        if self.frames != self.target {
            return Err(error(
                "INCOMPLETE_CAPTURE",
                "Recording did not reach its exact requested sample count",
            ));
        }
        let length = (self.frames * 4) as u32;
        let mut h = Vec::with_capacity(44);
        h.extend(b"RIFF");
        h.extend((length + 36).to_le_bytes());
        h.extend(b"WAVEfmt ");
        h.extend(16u32.to_le_bytes());
        h.extend(1u16.to_le_bytes());
        h.extend(2u16.to_le_bytes());
        h.extend(48000u32.to_le_bytes());
        h.extend(192000u32.to_le_bytes());
        h.extend(4u16.to_le_bytes());
        h.extend(16u16.to_le_bytes());
        h.extend(b"data");
        h.extend(length.to_le_bytes());
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        self.file.write_all(&h).map_err(io)?;
        self.file.flush().map_err(io)?;
        Ok((self.file, format!("{:x}", self.digest.finalize())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn two_hour_streaming_writer_exact_size_and_digest() {
        #[derive(Debug)]
        struct Sink {
            position: u64,
            maximum: u64,
            header: [u8; 44],
            largest_write: usize,
        }
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if self.position < 44 {
                    let n = (44 - self.position).min(b.len() as u64) as usize;
                    self.header[self.position as usize..self.position as usize + n]
                        .copy_from_slice(&b[..n]);
                }
                self.position += b.len() as u64;
                self.maximum = self.maximum.max(self.position);
                self.largest_write = self.largest_write.max(b.len());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Sink {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                match p {
                    SeekFrom::Start(n) => {
                        self.position = n;
                        Ok(n)
                    }
                    _ => Err(std::io::Error::other("unexpected relative seek")),
                }
            }
        }
        let mut writer = Writer::new(
            Sink {
                position: 0,
                maximum: 0,
                header: [0; 44],
                largest_write: 0,
            },
            MAX_FRAMES,
        )
        .unwrap();
        let chunk = [0u8; 8191 * 4];
        let mut frames = 0;
        while frames < MAX_FRAMES {
            let n = (MAX_FRAMES - frames).min(8191) as usize;
            writer.push(&chunk[..n * 4]).unwrap();
            frames += n as u64;
        }
        let (sink, digest) = writer.finish().unwrap();
        assert_eq!(sink.maximum, 1_382_400_044);
        assert_eq!(sink.largest_write, 8191 * 4);
        assert_eq!(dword(&sink.header, 4), 1_382_400_036);
        assert_eq!(dword(&sink.header, 40), 1_382_400_000);
        // Independently computed with Python/OpenSSL over exactly 1,382,400,000 zero bytes.
        assert_eq!(
            digest,
            "563ebb324ffec857ac9973c772e74d0a96ae374d64290053e3f44a0bbe167db9"
        );
        assert!(Writer::new(Cursor::new(Vec::new()), MAX_FRAMES + 1).is_err());
    }
    #[test]
    fn streaming_wave_counts_boundaries_and_failed_writes() {
        let mut w = Writer::new(Cursor::new(Vec::new()), 3).unwrap();
        w.push(&[0, 128, 255, 127]).unwrap();
        w.push(&[1, 0, 255, 255, 100, 0, 156, 255]).unwrap();
        assert_eq!(w.push(&[0; 4]).unwrap_err().code, "INVALID_CAPTURE_PACKET");
        let (cursor, digest) = w.finish().unwrap();
        let b = cursor.into_inner();
        assert_eq!(b.len(), 56);
        assert_eq!(&b[36..40], b"data");
        assert_eq!(dword(&b, 40), 12);
        assert_eq!(digest, format!("{:x}", Sha256::digest(&b[44..])));
        assert_eq!(
            crate::pcm_wave::decode(&b).unwrap().data,
            [-32768, 32767, 1, -1, 100, -100]
        );
        assert_eq!(
            Writer::new(Cursor::new(Vec::new()), 1)
                .unwrap()
                .finish()
                .unwrap_err()
                .code,
            "INCOMPLETE_CAPTURE"
        );
        struct FullDisk {
            position: u64,
        }
        impl Seek for FullDisk {
            fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
                Ok(self.position)
            }
        }
        impl Write for FullDisk {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if self.position >= 44 {
                    return Err(std::io::Error::other("injected full disk"));
                }
                self.position += b.len() as u64;
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = Writer::new(FullDisk { position: 0 }, 2).unwrap();
        assert_eq!(w.push(&[0; 8]).unwrap_err().code, "AUDIO_IO");
    }
}
