//! Original test-only encoder adapter. Binaries and all artifacts stay external.
use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio, exit},
    thread,
    time::Duration,
};

struct LimitedWriter {
    file: fs::File,
    remaining: usize,
    written: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::from_raw_os_error(if cfg!(windows) {
                112
            } else {
                28
            }));
        }
        let count = self.file.write(&bytes[..bytes.len().min(self.remaining)])?;
        self.remaining -= count;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let real = env::var("CUTBOLT_FAILURE_REAL_TOOL").unwrap();
    let Some(progress) = args.iter().position(|s| s == "-progress") else {
        exit(
            Command::new(real)
                .args(args)
                .status()
                .unwrap()
                .code()
                .unwrap_or(1),
        );
    };
    let root = fs::canonicalize(env::var("CUTBOLT_FAILURE_ROOT").unwrap()).unwrap();
    let output = PathBuf::from(args.last().unwrap());
    let output_root = fs::canonicalize(env::var("CUTBOLT_FAILURE_OUTPUT").unwrap()).unwrap();
    assert!(
        fs::canonicalize(output.parent().unwrap())
            .unwrap()
            .starts_with(output_root)
    );
    assert!(
        output
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(".cutbolt-")
    );
    let count = root.join("attempt-count.txt");
    let attempt = fs::read_to_string(&count)
        .ok()
        .map(|s| s.parse::<u32>().unwrap())
        .unwrap_or(0)
        + 1;
    fs::write(count, attempt.to_string()).unwrap();
    let mode = env::var("CUTBOLT_FAILURE_MODE").unwrap();
    if attempt > 1 && (mode == "once" || mode == "hold") {
        exit(
            Command::new(real)
                .args(args)
                .status()
                .unwrap()
                .code()
                .unwrap_or(1),
        );
    }
    // Real encoder bytes go through an original bounded writer, not fabricated output.
    args.drain(progress..progress + 2);
    *args.last_mut().unwrap() = "pipe:1".into();
    let mut child = Command::new(real)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let diagnostics = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut writer = LimitedWriter {
        file: fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .unwrap(),
        remaining: if mode == "changed" { usize::MAX } else { 8192 },
        written: 0,
    };
    let result = io::copy(&mut stdout, &mut writer);
    writer.flush().unwrap();
    writer.file.sync_all().unwrap();
    fs::copy(&output, root.join(format!("encoded-{attempt}.mkv"))).unwrap();
    if mode == "hold" {
        fs::write(
            root.join("held.txt"),
            format!("{} {}", std::process::id(), child.id()),
        )
        .unwrap();
        for _ in 0..1200 {
            if root.join("release").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    if let Err(failure) = result {
        let _ = child.kill();
        let _ = child.wait();
        let _ = diagnostics.join().unwrap();
        fs::write(
            root.join(format!("failure-{attempt}.json")),
            format!(
                "{{\"written\":{},\"raw_os_error\":{},\"injected\":true}}\n",
                writer.written,
                failure.raw_os_error().unwrap()
            ),
        )
        .unwrap();
        eprintln!(
            "Original fixture injected output write failure after {} bytes: {failure}",
            writer.written
        );
        exit(74);
    }
    assert!(
        child.wait().unwrap().success(),
        "{:?}",
        diagnostics.join().unwrap()
    );
    assert_eq!(mode, "changed");
    let source = fs::canonicalize(env::var("CUTBOLT_FAILURE_SOURCE").unwrap()).unwrap();
    let source_root = fs::canonicalize(env::var("CUTBOLT_FAILURE_SOURCES").unwrap()).unwrap();
    assert!(source.starts_with(source_root));
    fs::OpenOptions::new()
        .append(true)
        .open(source)
        .unwrap()
        .write_all(b"original changed-source fixture")
        .unwrap();
}
