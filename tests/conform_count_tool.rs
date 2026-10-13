//! Original adapter counting actual conversion encoder launches, forwarding every byte unchanged.
use std::{
    env,
    fs::OpenOptions,
    io::Write,
    process::{Command, exit},
};
fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let encoded = args.windows(2).any(|w| w == ["-c:v", "ffv1"]);
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var("CUTBOLT_CONFORM_COUNT_LOG").unwrap())
        .unwrap();
    log.write_all(if encoded { b"encode\n" } else { b"other\n" })
        .unwrap();
    drop(log);
    // An original cancellation fixture can hold only the conversion encoder at this boundary.
    if encoded && let Some(path) = env::var_os("CUTBOLT_CONFORM_COUNT_GATE") {
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.with_extension("entered"), b"encoder waiting").unwrap();
        let start = std::time::Instant::now();
        while path.exists() {
            if start.elapsed() > std::time::Duration::from_secs(45) {
                exit(99);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let status = Command::new(env::var("CUTBOLT_CONFORM_COUNT_REAL").unwrap())
        .args(args)
        .status()
        .unwrap();
    exit(status.code().unwrap_or(1));
}
