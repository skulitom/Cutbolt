//! Bounded encoded output for cached normalization. Legacy file encoding is unchanged.
use crate::{Result, error, media};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) fn limit(bytes: u64) -> crate::Error {
    error(
        "LIMIT_EXCEEDED",
        format!("Conformed output exceeds its {bytes}-byte storage budget"),
    )
    .with_path("max_output_bytes")
    .with_hint(
        "Raise the explicit output budget or shorten the conversion; no output was published.",
    )
    .with_detail(serde_json::json!({"max_output_bytes":bytes,"published":false}))
}

/// Consume encoded Matroska from stdout without ever writing beyond the caller's byte cap.
/// An over-budget or failed sink cancels the encoder, unblocking its input producer as well.
pub(super) fn bounded(
    args: &[String],
    output: &Path,
    max_bytes: u64,
    timeout: Duration,
    quota: Option<super::Quota>,
    produce: impl FnOnce(&mut dyn Write) -> Result<()>,
) -> Result<()> {
    let mut args = args.to_vec();
    *args.last_mut().expect("encoder output") = "pipe:1".into();
    let mut file = File::create_new(output)?;
    let abort = Arc::new(AtomicBool::new(false));
    let failure = Arc::new(Mutex::new(None));
    let (failed, stopped) = (failure.clone(), abort.clone());
    let consume = move |mut reader: std::process::ChildStdout| {
        let result = (|| -> Result<()> {
            let mut count = 0u64;
            let mut block = [0u8; 65536];
            loop {
                let n = reader.read(&mut block)?;
                if n == 0 {
                    break;
                }
                if n as u64 > max_bytes.saturating_sub(count) {
                    return Err(limit(max_bytes));
                }
                if let Some(reserve) = &quota {
                    reserve(count + n as u64)?;
                }
                file.write_all(&block[..n])?;
                count += n as u64;
            }
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = result {
            *failed.lock().expect("output failure") = Some(error);
            stopped.store(true, Ordering::SeqCst);
        }
        // Dropping stdout wakes an encoder blocked on output; the watchdog handles other waits.
        Ok(())
    };
    let program = media::tool("ffmpeg");
    let mut command = Command::new(&program);
    command.args(args);
    let result = media::feed_stdin_consuming(
        command,
        &program,
        timeout,
        media::Watch { abort: Some(abort) },
        consume,
        produce,
    );
    if let Some(error) = failure.lock().expect("output failure").take() {
        return Err(error);
    }
    result
}
