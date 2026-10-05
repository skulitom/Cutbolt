//! SHA-256 of decoded media, computed in Rust from FFmpeg's raw output. FFmpeg's own hash muxer
//! (`-f hash -hash sha256`) hashes at about a third of a gigabyte per second, so verifying 80 s of
//! 1080p RGB (12.5 GB) took about 39 s, against 2 s to decode it. Reading the same bytes through a
//! pipe into the sha2 crate gives the identical digest several times faster.
use crate::{Result, media};
use sha2::{Digest, Sha256};
use std::{sync::mpsc, thread, time::Duration};

const CHUNK: usize = 4 * 1024 * 1024;

/// Lowercase hex SHA-256 of exactly `bytes` bytes that FFmpeg, run with `args`, writes to stdout
/// (`... -f rawvideo -` or `... -f s16le -`); the same digest `-f hash -hash sha256` prints for
/// that stream. Fewer bytes, or any trailing output, fail. Hashing runs on its own thread while
/// the next chunk is read.
pub(crate) fn raw_sha256(args: &[String], bytes: u64, timeout: Duration) -> Result<String> {
    let mut reader = media::StreamReader::spawn(&media::tool("ffmpeg"), args, timeout)?;
    let digest = exact_sha256(bytes, |chunk| reader.read_exact(chunk), |_| Ok(()))?;
    reader.finish()?;
    Ok(digest)
}

/// Lowercase hex SHA-256 of exactly `bytes` bytes that `fill` supplies, one buffer at a time.
/// Hashing runs on its own thread while the next buffer fills; `progress` receives the number of
/// bytes read so far after each one. The caller checks that the source has nothing more.
pub(crate) fn exact_sha256(
    bytes: u64,
    mut fill: impl FnMut(&mut [u8]) -> Result<()>,
    mut progress: impl FnMut(u64) -> Result<()>,
) -> Result<String> {
    let (full, filled) = mpsc::sync_channel::<Vec<u8>>(2);
    let (empty, recycled) = mpsc::channel::<Vec<u8>>();
    let hasher = thread::spawn(move || {
        let mut hash = Sha256::new();
        for chunk in filled {
            hash.update(&chunk);
            let _ = empty.send(chunk);
        }
        format!("{:x}", hash.finalize())
    });
    let mut left = bytes;
    while left > 0 {
        let n = left.min(CHUNK as u64) as usize;
        let mut chunk = recycled.try_recv().unwrap_or_else(|_| vec![0; CHUNK]);
        chunk.resize(n, 0);
        fill(&mut chunk)?;
        left -= n as u64;
        if full.send(chunk).is_err() {
            break;
        }
        progress(bytes - left)?;
    }
    drop(full);
    hasher
        .join()
        .map_err(|_| crate::error("TOOL_FAILED", "Digest thread failed"))
}

/// Lowercase hex SHA-256 of `count` repetitions of `size` zero bytes: the decoded digest of a
/// silent black RGB picture or of digital silence.
pub(crate) fn zeros_sha256(size: u64, count: u64) -> String {
    let mut hash = Sha256::new();
    let zeros = vec![0u8; CHUNK];
    let mut left = size * count;
    while left > 0 {
        let n = left.min(CHUNK as u64) as usize;
        hash.update(&zeros[..n]);
        left -= n as u64;
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_digests_hash_every_buffer_in_order() {
        // More than two chunks and not a multiple of one, so buffers are recycled and the last
        // one is partial.
        let data: Vec<u8> = (0..CHUNK * 2 + 12_345)
            .map(|i| (i * 7 % 251) as u8)
            .collect();
        let (mut read, mut reported) = (0, Vec::new());
        let digest = exact_sha256(
            data.len() as u64,
            |chunk| {
                chunk.copy_from_slice(&data[read..read + chunk.len()]);
                read += chunk.len();
                Ok(())
            },
            |done| {
                reported.push(done);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(&data)));
        assert_eq!(
            reported,
            [CHUNK as u64, 2 * CHUNK as u64, data.len() as u64]
        );
        // A failed read or a refused progress report stops the digest with that error.
        let short = exact_sha256(10, |_| Err(crate::error("SHORT", "short")), |_| Ok(()));
        assert_eq!(short.unwrap_err().code, "SHORT");
        let cancelled = exact_sha256(10, |_| Ok(()), |_| Err(crate::error("STOP", "stop")));
        assert_eq!(cancelled.unwrap_err().code, "STOP");
    }

    #[test]
    fn zero_digests_match_a_direct_hash() {
        // Not a multiple of the chunk, so the last partial chunk is covered.
        let (size, count) = (1920 * 1080 * 3, 3);
        let direct = format!("{:x}", Sha256::digest(vec![0u8; size * count]));
        assert_eq!(zeros_sha256(size as u64, count as u64), direct);
        assert_eq!(
            zeros_sha256(0, 5),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
