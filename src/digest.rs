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
        reader.read_exact(&mut chunk)?;
        left -= n as u64;
        if full.send(chunk).is_err() {
            break;
        }
    }
    drop(full);
    reader.finish()?;
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
