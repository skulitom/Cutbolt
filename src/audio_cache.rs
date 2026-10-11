//! Bounded canonical PCM chunks for codecs whose seeks do not reproduce a complete decode.
//! Chunk digests come from the fully verified source index, not from the cache itself.
use crate::{
    Result,
    cache_store::{Policy, Store, Temporary, digest},
    error, inspection_cache,
    media::{self, Control},
    registry::Identity,
    source_index::Audio,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(crate) const CHUNK_BYTES: usize = 1024 * 1024;
const KIND: &str = "canonical_audio";
const MAX_CHUNKS: usize = 65536;

/// Native interleaved PCM16 byte offset, length and SHA-256; never normalized/resampled PCM.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Chunk(pub u64, pub u64, pub String);

pub(crate) fn valid(chunks: &[Chunk], bytes: u64) -> bool {
    bytes > 0
        && chunks.len() <= MAX_CHUNKS
        && chunks.len() as u64 == bytes.div_ceil(CHUNK_BYTES as u64)
        && chunks.iter().enumerate().all(|(n, c)| {
            c.0 == n as u64 * CHUNK_BYTES as u64
                && c.1 == (bytes - c.0).min(CHUNK_BYTES as u64)
                && crate::cache_store::key_ok(&c.2)
        })
}

pub(crate) fn required(stream: &serde_json::Value) -> bool {
    stream["codec_name"] == "aac"
}

pub(crate) fn directory() -> Option<PathBuf> {
    inspection_cache::directory().map(|root| root.join("audio"))
}

pub(crate) struct Cache(Store);
impl Cache {
    pub(crate) fn configured() -> Result<Option<Self>> {
        directory()
            .map(|root| {
                fs::create_dir_all(&root)?;
                let parent = media::input_root(root.parent().expect("audio cache parent"))?;
                if !root.canonicalize()?.starts_with(parent) {
                    return Err(error(
                        "PATH_OUTSIDE_ROOT",
                        "Audio cache must stay inside the configured inspection directory",
                    ));
                }
                Self::open(&root)
            })
            .transpose()
    }
    fn open(root: &Path) -> Result<Self> {
        Ok(Self(Store::open(root, true)?))
    }
    fn key(index: &str, chunk: &Chunk) -> Result<String> {
        if chunk.1 == 0
            || chunk.1 > CHUNK_BYTES as u64
            || !chunk.0.is_multiple_of(CHUNK_BYTES as u64)
            || !crate::cache_store::key_ok(&chunk.2)
        {
            return Err(error(
                "CACHE_CORRUPT",
                "Canonical audio chunk descriptor is invalid",
            ));
        }
        Ok(digest(&serde_json::to_vec(&json!([
            KIND, 1, index, chunk
        ]))?))
    }
    pub(crate) fn get(&mut self, index: &str, chunk: &Chunk) -> Result<Option<Vec<u8>>> {
        let key = Self::key(index, chunk)?;
        let temp = Temporary::new(&self.0.root, "pcm")?;
        fs::remove_file(&temp.0)?; // read() reserves its own unused destination.
        let Some(hit) = self.0.read(&key, KIND, &temp.0)? else {
            return Ok(None);
        };
        if hit.identity.bytes != chunk.1 || hit.identity.sha256 != chunk.2 {
            self.0.discard(&key)?;
            return Ok(None);
        }
        let bytes = fs::read(&temp.0)?;
        // Compare again with the trusted index after reading the temporary file.
        if bytes.len() as u64 != chunk.1 || digest(&bytes) != chunk.2 {
            return Err(error(
                "CACHE_CORRUPT",
                "Canonical audio copy changed during read",
            ));
        }
        Ok(Some(bytes))
    }
    pub(crate) fn put(&mut self, index: &str, chunk: &Chunk, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty()
            || bytes.len() > CHUNK_BYTES
            || bytes.len() as u64 != chunk.1
            || digest(bytes) != chunk.2
        {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Canonical audio chunk disagrees with its source index",
            ));
        }
        let temp = Temporary::new(&self.0.root, "pcm")?;
        fs::write(&temp.0, bytes)?;
        let key = Self::key(index, chunk)?;
        let identity = Identity {
            bytes: chunk.1,
            sha256: chunk.2.clone(),
        };
        let metadata =
            json!({"source_index":index,"byte_offset":chunk.0,"format":"native_interleaved_pcm16"});
        let policy = Policy {
            max_bytes: 1024 * 1024 * 1024,
            max_entries: 4096,
        };
        let result = self
            .0
            .put(&key, KIND, &temp.0, &identity, &metadata, &policy)?;
        // Repair an existing corrupt chunk while its canonical bytes are already available,
        // rather than discovering it on the next range and decoding the whole source again.
        if result["reason"] == "concurrent_entry" && self.get(index, chunk)?.is_none() {
            self.0
                .put(&key, KIND, &temp.0, &identity, &metadata, &policy)?;
        }
        Ok(())
    }
}

fn read_chunk(input: &mut impl Read) -> Result<Vec<u8>> {
    let mut bytes = vec![0; CHUNK_BYTES];
    let mut filled = 0;
    while filled < bytes.len() {
        match input.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    bytes.truncate(filled);
    Ok(bytes)
}

/// Indexing already decodes the complete audio. Retain its canonical chunk hashes and, when
/// explicitly configured, its bounded cache payloads. A chunk key includes its actual digest:
/// provisional chunks cannot masquerade as different content in a later verified index.
pub(crate) fn collect(mut input: impl Read, index: &str) -> Result<(u64, Vec<Chunk>)> {
    let mut cache = Cache::configured()?;
    let mut chunks = Vec::new();
    let mut total = 0;
    loop {
        let bytes = read_chunk(&mut input)?;
        if bytes.is_empty() {
            break;
        }
        if chunks.len() == MAX_CHUNKS {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Canonical audio index exceeds 65536 chunks",
            ));
        }
        let chunk = Chunk(total, bytes.len() as u64, digest(&bytes));
        if let Some(cache) = &mut cache {
            cache.put(index, &chunk, &bytes)?;
        }
        total += chunk.1;
        chunks.push(chunk);
    }
    Ok((total, chunks))
}

/// Rebuild evicted/corrupt chunks from the full canonical decode. The bounded queue delivers
/// bytes to the calling thread, so cancellation and the original sink error remain observable.
pub(crate) fn rebuild(
    path: &Path,
    audio: &Audio,
    index: &str,
    control: &dyn Control,
    mut cache: Option<&mut Cache>,
    mut consume: impl FnMut(usize, &[u8]) -> Result<()>,
) -> Result<u64> {
    let program = control.tool("ffmpeg");
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-nostdin",
        "-xerror",
        "-copyts",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(path.to_string_lossy().into_owned());
    args.extend(
        [
            "-map",
            "0:a:0",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let (sender, receiver) = mpsc::sync_channel(2);
    let abort = Arc::new(AtomicBool::new(false));
    let mut processing = Ok(());
    let mut next = 0;
    let decoded = media::feed_stdin_consuming(
        control.command(&program, &args)?,
        &program,
        Duration::from_secs(1800),
        media::Watch {
            abort: Some(abort.clone()),
        },
        move |mut stdout| {
            let mut total = 0;
            loop {
                let bytes = read_chunk(&mut stdout)?;
                if bytes.is_empty() {
                    break;
                }
                total += bytes.len() as u64;
                if sender.send(bytes).is_err() {
                    break;
                }
            }
            Ok(total)
        },
        |_| {
            processing = (|| {
                loop {
                    control.check()?;
                    let bytes = match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(bytes) => bytes,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let chunk = audio.chunks.get(next).ok_or_else(|| {
                        error(
                            "RENDER_VALIDATION_FAILED",
                            "Canonical audio grew since indexing",
                        )
                    })?;
                    if bytes.len() as u64 != chunk.1 || digest(&bytes) != chunk.2 {
                        return Err(error(
                            "RENDER_VALIDATION_FAILED",
                            "Canonical audio chunk disagrees with its source index",
                        ));
                    }
                    if let Some(cache) = &mut cache {
                        cache.put(index, chunk, &bytes)?;
                    }
                    consume(next, &bytes)?;
                    next += 1;
                }
                Ok(())
            })();
            if processing.is_err() {
                abort.store(true, Ordering::Relaxed);
            }
            drop(receiver);
            Ok(())
        },
    );
    processing?;
    let bytes = decoded?;
    if next != audio.chunks.len() || bytes != audio.samples * audio.channels * 2 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Canonical audio ended before its indexed sample count",
        ));
    }
    Ok(audio.samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trusted_chunk_identity_bounds_corruption_and_lru() {
        let root = crate::tracking::Scratch::new("canonical-audio");
        let mut cache = Cache::open(&root.0).unwrap();
        let bytes = vec![37; CHUNK_BYTES];
        let chunks: Vec<_> = (0..3)
            .map(|n| Chunk(n * CHUNK_BYTES as u64, bytes.len() as u64, digest(&bytes)))
            .collect();
        assert!(valid(&chunks, 3 * CHUNK_BYTES as u64));
        assert!(!valid(&chunks, 3 * CHUNK_BYTES as u64 - 1));
        let mut broken = chunks.clone();
        broken[1].0 += 1;
        assert!(!valid(&broken, 3 * CHUNK_BYTES as u64));
        assert!(cache.put("source", &chunks[0], b"wrong").is_err());
        for c in &chunks[..2] {
            cache.put("source", c, &bytes).unwrap();
        }
        assert!(
            cache
                .get("changed-source-tool-or-engine", &chunks[0])
                .unwrap()
                .is_none()
        );
        assert_eq!(cache.get("source", &chunks[0]).unwrap().unwrap(), bytes);
        cache.put("source", &chunks[2], &bytes).unwrap();
        cache
            .0
            .prune(&Policy {
                max_bytes: 2 * CHUNK_BYTES as u64,
                max_entries: 2,
            })
            .unwrap();
        assert!(cache.get("source", &chunks[1]).unwrap().is_none());
        assert!(cache.get("source", &chunks[0]).unwrap().is_some());
        let key = Cache::key("source", &chunks[0]).unwrap();
        let db = rusqlite::Connection::open(root.0.join(crate::cache_store::FILE)).unwrap();
        // Self-consistent cache corruption still cannot substitute a different canonical digest.
        let replacement = vec![0u8; CHUNK_BYTES];
        let replacement_hash = digest(&replacement);
        db.execute(
            "UPDATE entries SET data=?1,digest=?2 WHERE key=?3",
            rusqlite::params![replacement, replacement_hash, key],
        )
        .unwrap();
        assert!(cache.get("source", &chunks[0]).unwrap().is_none());
        cache.put("source", &chunks[0], &bytes).unwrap();
        assert_eq!(cache.get("source", &chunks[0]).unwrap().unwrap(), bytes);
    }
}
