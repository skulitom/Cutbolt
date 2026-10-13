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
    let status = Command::new(env::var("CUTBOLT_CONFORM_COUNT_REAL").unwrap())
        .args(args)
        .status()
        .unwrap();
    exit(status.code().unwrap_or(1));
}
