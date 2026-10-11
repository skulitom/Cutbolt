//! Seek audio by native PCM block anchors, then stream exact rational linear resampling.
use super::Video;
use crate::{
    Result, error,
    media::{self, Control},
    source_index::{self, Audio},
    time::{Stamp, Time},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Debug, Serialize)]
pub struct AudioDecoded {
    pub index: String,
    pub first: u64,
    pub samples: u64,
    pub sample_rate: u64,
    pub channels: u64,
    pub seek_seconds: Option<String>,
    pub decoded_native_samples: u64,
    pub verified_blocks: u64,
    pub mode: &'static str,
    pub cached_chunks: u64,
    pub rebuilt_chunks: u64,
}

fn invalid(message: impl Into<String>) -> crate::Error {
    error("RENDER_VALIDATION_FAILED", message)
}

/// Metadata and bytes use the same bounded queue. Neither pipe waits for the other before
/// draining, including when a PCM block is larger than the OS pipe buffer.
enum Event {
    Bytes(Vec<u8>),
    Frame(i64, u64, String),
    Done,
}

impl Video {
    /// Whole 48 kHz samples covered by the video extent. Timeline intervals still have to
    /// align with their own exact sample clock; a fractional final sample is not synthesized.
    pub fn audio_samples(&self) -> u64 {
        (self.frames() as u128 * self.rate.den as u128 * 48000 / self.rate.num as u128) as u64
    }

    /// Read a range on the video's normalized 48 kHz clock, returning stereo PCM16 blocks.
    /// Native audio is matched to its full-decode anchors before interpolation. Early/long
    /// audio trims, late/short audio pads, and mono duplicates. Output is provisional until
    /// success; source identity and decoder exit are checked before returning a receipt.
    pub fn read_audio(
        &self,
        first: u64,
        count: u64,
        control: &dyn Control,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<AudioDecoded> {
        first
            .checked_add(count)
            .filter(|&end| count > 0 && end <= self.audio_samples())
            .ok_or_else(|| {
                error(
                    "INVALID_RANGE",
                    "Direct audio range must contain samples inside the video extent",
                )
            })?;
        self.verify(control, false)?;
        let mut receipt = AudioDecoded {
            index: self.loaded.key.clone(),
            first,
            samples: count,
            sample_rate: 48000,
            channels: 2,
            seek_seconds: None,
            decoded_native_samples: 0,
            verified_blocks: 0,
            mode: "silence",
            cached_chunks: 0,
            rebuilt_chunks: 0,
        };
        let Some(audio) = &self.loaded.index.audio else {
            silence(count, &mut consume)?;
            self.verify(control, true)?;
            return Ok(receipt);
        };
        let origin = Stamp::ticks(self.loaded.index.frames[0].0, self.loaded.index.time_base)?;
        let offset = origin
            .minus(Stamp::ticks(audio.first_pts, Time::new(1, audio.rate)?)?)?
            .times(Time::new(audio.rate, 1)?)?;
        let mut resampler = Resampler::new(audio, offset, first, count)?;
        let start = resampler.position(0)?.0;
        let last = resampler.position(count - 1)?.0;
        if start >= audio.samples as i128 || last < 0 {
            silence(count, &mut consume)?;
            self.verify(control, true)?;
            return Ok(receipt);
        }
        let lo = start.max(0) as u64;
        let hi = last.saturating_add(1).clamp(0, audio.samples as i128 - 1) as u64;
        if crate::audio_cache::required(&audio.stream) {
            let mut cache = crate::audio_cache::Cache::configured()?;
            receipt.mode = if cache.is_some() {
                "canonical_cache"
            } else {
                "canonical_decode"
            };
            let bytes_per_sample = audio.channels * 2;
            let first_chunk = audio
                .chunks
                .partition_point(|c| c.0 + c.1 <= lo * bytes_per_sample);
            let last_chunk = audio
                .chunks
                .partition_point(|c| c.0 <= hi.min(audio.samples - 1) * bytes_per_sample)
                .saturating_sub(1);
            for n in first_chunk..=last_chunk {
                control.check()?;
                let chunk = audio
                    .chunks
                    .get(n)
                    .ok_or_else(|| invalid("Canonical audio index has no requested chunk"))?;
                let bytes = cache
                    .as_mut()
                    .map(|c| c.get(&self.loaded.key, chunk))
                    .transpose()?
                    .flatten();
                if let Some(bytes) = bytes {
                    resampler.push(chunk.0 / bytes_per_sample, &bytes, &mut consume)?;
                    receipt.cached_chunks += 1;
                } else {
                    receipt.decoded_native_samples = crate::audio_cache::rebuild(
                        &self.path,
                        audio,
                        &self.loaded.key,
                        control,
                        cache.as_mut(),
                        |at, bytes| {
                            if (n..=last_chunk).contains(&at) {
                                resampler.push(
                                    audio.chunks[at].0 / bytes_per_sample,
                                    bytes,
                                    &mut consume,
                                )?;
                                receipt.rebuilt_chunks += 1;
                            }
                            Ok(())
                        },
                    )?;
                    break;
                }
            }
            resampler.finish(&mut consume)?;
            self.verify(control, true)?;
            return Ok(receipt);
        }
        receipt.mode = "verified_seek";
        let first_block = audio.frames.partition_point(|f| f.1 + f.2 <= lo);
        let last_block = audio
            .frames
            .partition_point(|f| f.1 <= hi.min(audio.samples - 1))
            .saturating_sub(1);
        // Decode two seconds of preroll to restore compressed-codec state. Block checksums
        // prove convergence before any needed samples are used, rather than assuming it.
        let seek = Stamp::ticks(audio.frames[first_block].0, Time::new(1, audio.rate)?)?
            .minus(Stamp::time(Time::new(2, 1)?)?)?;
        let initial = Stamp::ticks(audio.first_pts, Time::new(1, audio.rate)?)?;
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
        if seek.compare(initial)?.is_gt() {
            let (micros, _, _) = seek.times(Time::new(1_000_000, 1)?)?.sample();
            let n = micros.unsigned_abs();
            let seconds = format!(
                "{}{}.{:06}",
                if micros < 0 { "-" } else { "" },
                n / 1_000_000,
                n % 1_000_000
            );
            args.extend(["-ss".into(), seconds.clone(), "-noaccurate_seek".into()]);
            receipt.seek_seconds = Some(seconds);
        }
        args.extend(
            ["-protocol_whitelist", "file,pipe", "-i"]
                .into_iter()
                .map(str::to_owned),
        );
        args.push(self.path.to_string_lossy().into_owned());
        let last = &audio.frames[(last_block + 1).min(audio.frames.len() - 1)];
        let stop = last
            .0
            .checked_add(last.2 as i64 + (audio.rate / 1000) as i64 + 1)
            .ok_or_else(|| invalid("Audio end timestamp overflow"))?;
        let filter = format!("[0:a:0]atrim=end_pts={stop},asplit=2[pcm][clock]");
        args.extend(
            [
                "-filter_complex",
                &filter,
                "-map",
                "[pcm]",
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "pipe:1",
                "-map",
                "[clock]",
                "-c:a",
                "pcm_s16le",
                "-f",
                "framehash",
                "-hash",
                "sha256",
                "pipe:2",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let mut anchors = std::collections::HashMap::new();
        for (n, f) in audio.frames.iter().enumerate() {
            anchors
                .entry((f.2, f.3.clone()))
                .or_insert_with(Vec::new)
                .push(n);
        }
        for candidates in anchors.values_mut() {
            candidates.sort_unstable_by_key(|&n| audio.frames[n].0);
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let clock_sender = sender.clone();
        let channels = audio.channels;
        let rate = audio.rate;
        let abort = Arc::new(AtomicBool::new(false));
        let mut processing = Ok(());
        let program = control.tool("ffmpeg");
        let decoded = media::feed_stdin_pipes(
            control.command(&program, &args)?,
            &program,
            Duration::from_secs(1800),
            media::Watch {
                abort: Some(abort.clone()),
            },
            (
                move |mut stdout| {
                    loop {
                        let mut bytes = vec![0; 65536];
                        let count = stdout.read(&mut bytes)?;
                        if count == 0 {
                            break;
                        }
                        bytes.truncate(count);
                        if sender.send(Event::Bytes(bytes)).is_err() {
                            return Ok(());
                        }
                    }
                    let _ = sender.send(Event::Done);
                    Ok(())
                },
                move |stderr| {
                    let mut diagnostics = Vec::new();
                    let parsed = source_index::lines(stderr, |line| {
                        if let Some(tb) = line.strip_prefix("#tb 0: ") {
                            if tb != format!("1/{rate}") {
                                return Err(invalid("Sought PCM time base changed"));
                            }
                        } else if line.starts_with("0,") {
                            let fields: Vec<_> = line.split(',').map(str::trim).collect();
                            let parse = || -> Option<(i64, u64, String)> {
                                let pts = fields.get(2)?.parse().ok()?;
                                let count: u64 = fields.get(3)?.parse().ok()?;
                                let bytes: u64 = fields.get(4)?.parse().ok()?;
                                let crc = fields.get(5)?.to_string();
                                if crc.len() != 64
                                    || !crc
                                        .bytes()
                                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                                {
                                    return None;
                                }
                                (count > 0
                                    && count.checked_mul(channels * 2) == Some(bytes)
                                    && bytes <= 1_048_576)
                                    .then_some((pts, count, crc))
                            };
                            let (pts, count, crc) =
                                parse().ok_or_else(|| invalid("Invalid sought PCM block"))?;
                            clock_sender
                                .send(Event::Frame(pts, count, crc))
                                .map_err(|_| invalid("Audio consumer stopped"))?;
                        } else if !line.starts_with('#') {
                            let keep = line.len().min(65536usize.saturating_sub(diagnostics.len()));
                            diagnostics.extend_from_slice(&line.as_bytes()[..keep]);
                        }
                        Ok(())
                    });
                    let _ = clock_sender.send(Event::Done);
                    (parsed, diagnostics)
                },
            ),
            |_| {
                processing = (|| {
                    let mut bytes = std::collections::VecDeque::new();
                    let mut frames = std::collections::VecDeque::new();
                    let mut done = 0;
                    let mut next = None;
                    while done < 2 {
                        control.check()?;
                        match receiver.recv_timeout(Duration::from_millis(100)) {
                            Ok(Event::Bytes(block)) => bytes.extend(block),
                            Ok(Event::Frame(pts, count, crc)) => {
                                frames.push_back((pts, count, crc))
                            }
                            Ok(Event::Done) => done += 1,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => {
                                return Err(invalid("Audio pipe ended without complete output"));
                            }
                        }
                        while let Some((pts, count, crc)) = frames.front().cloned() {
                            let size = (count * channels * 2) as usize;
                            if bytes.len() < size {
                                break;
                            }
                            let block: Vec<_> = bytes.drain(..size).collect();
                            frames.pop_front();
                            receipt.decoded_native_samples += count;
                            if format!("{:x}", Sha256::digest(&block)) != crc {
                                return Err(invalid(
                                    "Sought PCM bytes disagree with their block checksum",
                                ));
                            }
                            // Seeked sample clocks can differ by container rounding (Opus MKV
                            // is a common example). The PCM hash identifies the exact block;
                            // require a unique match within the index's 1 ms clock tolerance.
                            let candidate = anchors.get(&(count, crc)).and_then(|candidates| {
                                let tolerance = audio.rate as i128 / 1000;
                                let lo = candidates.partition_point(|&n| {
                                    (audio.frames[n].0 as i128) < pts as i128 - tolerance
                                });
                                let hi = candidates.partition_point(|&n| {
                                    (audio.frames[n].0 as i128) <= pts as i128 + tolerance
                                });
                                (hi - lo == 1).then(|| candidates[lo])
                            });
                            if let Some(n) = candidate {
                                if n < first_block {
                                    continue;
                                }
                                if next.is_some_and(|expected| expected != n)
                                    && !resampler.complete()
                                {
                                    return Err(invalid(
                                        "Sought audio skipped an indexed PCM block",
                                    ));
                                }
                                let anchor = &audio.frames[n];
                                if next.is_none() && anchor.1 > lo {
                                    return Err(invalid(
                                        "Sought audio did not converge before the requested samples",
                                    ));
                                }
                                next = Some(n + 1);
                                resampler.push(anchor.1, &block, &mut consume)?;
                                receipt.verified_blocks += 1;
                            } else if next.is_some() && !resampler.complete() {
                                return Err(invalid(
                                    "Sought PCM disagrees with its full-decode anchor",
                                ));
                            }
                        }
                        if bytes.len() > 2_097_152 || frames.len() > 512 {
                            return Err(invalid(
                                "Audio pipes exceeded their bounded pairing window",
                            ));
                        }
                    }
                    if !bytes.is_empty() || !frames.is_empty() {
                        return Err(invalid("Sought PCM data and clocks have different lengths"));
                    }
                    resampler.finish(&mut consume)
                })();
                if processing.is_err() {
                    abort.store(true, Ordering::Relaxed);
                }
                // Drop the receiver now, so blocked pipe senders can finish on a consumer error.
                drop(receiver);
                Ok(())
            },
        );
        processing?;
        decoded?;
        self.verify(control, true)?;
        Ok(receipt)
    }
}

fn silence(count: u64, consume: &mut impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
    let block = [0; 65536];
    let mut remaining = count;
    while remaining > 0 {
        let n = remaining.min(16384);
        consume(&block[..(n * 4) as usize])?;
        remaining -= n;
    }
    Ok(())
}

struct Resampler<'a> {
    audio: &'a Audio,
    start: Stamp,
    count: u64,
    next: u64,
    previous: Option<(u64, [i16; 2])>,
    output: Vec<u8>,
}
impl<'a> Resampler<'a> {
    fn new(audio: &'a Audio, offset: Stamp, first: u64, count: u64) -> Result<Self> {
        Ok(Self {
            audio,
            start: offset.plus(Stamp::time(
                Time::new(first, 48000)?.times(Time::new(audio.rate, 1)?)?,
            )?)?,
            count,
            next: 0,
            previous: None,
            output: Vec::with_capacity(65536),
        })
    }
    fn position(&self, n: u64) -> Result<(i128, i128, i128)> {
        Ok(self
            .start
            .plus(Stamp::time(
                Time::new(n, 48000)?.times(Time::new(self.audio.rate, 1)?)?,
            )?)?
            .sample())
    }
    fn complete(&self) -> bool {
        self.next == self.count
    }
    fn push(
        &mut self,
        first: u64,
        block: &[u8],
        consume: &mut impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let channels = self.audio.channels as usize;
        let count = block.len() / (channels * 2);
        let end = first + count as u64;
        let sample = |n: u64, ch: usize| -> Result<i16> {
            if n < first {
                return self
                    .previous
                    .filter(|(at, _)| *at == n)
                    .map(|(_, v)| v[ch])
                    .ok_or_else(|| {
                        invalid("Sought audio omitted samples needed for interpolation")
                    });
            }
            let ch = if channels == 1 { 0 } else { ch };
            let i = ((n - first) as usize * channels + ch) * 2;
            Ok(i16::from_le_bytes([block[i], block[i + 1]]))
        };
        while !self.complete() {
            let (a, remainder, denominator) = self.position(self.next)?;
            let outside = a < 0 || a >= self.audio.samples as i128;
            let b = (a + 1).min(self.audio.samples as i128 - 1);
            if !outside && b >= end as i128 {
                break;
            }
            for ch in 0..2 {
                let value = if outside {
                    0
                } else {
                    let x = sample(a as u64, ch)? as i128;
                    let y = sample(b as u64, ch)? as i128;
                    let sum = x * (denominator - remainder) + y * remainder;
                    ((sum.abs() + denominator / 2) / denominator * sum.signum()) as i16
                };
                self.output.extend_from_slice(&value.to_le_bytes());
            }
            self.next += 1;
            if self.output.len() == 65536 {
                consume(&self.output)?;
                self.output.clear();
            }
        }
        self.previous = Some((end - 1, [sample(end - 1, 0)?, sample(end - 1, 1)?]));
        Ok(())
    }
    fn finish(&mut self, consume: &mut impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        if !self.complete() {
            return Err(invalid(
                "Sought audio ended before all requested samples were produced",
            ));
        }
        if !self.output.is_empty() {
            consume(&self.output)?;
            self.output.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpolation_retains_one_sample_across_blocks_and_pads_shifted_ends() {
        let audio = Audio {
            rate: 24000,
            channels: 1,
            samples: 3,
            first_pts: 0,
            continuous: true,
            codec_initial_padding: None,
            stream: serde_json::Value::Null,
            frames: Vec::new(),
            chunks: Vec::new(),
        };
        for (offset, expected) in [
            (Stamp::ZERO, vec![10, 20, 30, 40, 50, 50]),
            (
                Stamp::ticks(-1, Time::new(1, 2).unwrap()).unwrap(),
                vec![0, 10, 20, 30, 40, 50, 50, 0],
            ),
        ] {
            let mut resampler = Resampler::new(&audio, offset, 0, expected.len() as u64).unwrap();
            let mut output = Vec::new();
            let mut sink = |bytes: &[u8]| {
                output.extend_from_slice(bytes);
                Ok(())
            };
            for (n, value) in [10i16, 30, 50].into_iter().enumerate() {
                resampler
                    .push(n as u64, &value.to_le_bytes(), &mut sink)
                    .unwrap();
            }
            resampler.finish(&mut sink).unwrap();
            let values: Vec<_> = output
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| {
                    assert_eq!(&b[..2], &b[2..]);
                    i16::from_le_bytes([b[0], b[1]])
                })
                .collect();
            assert_eq!(values, expected);
        }
    }
}
