use cutbolt::{Result, commands::handle_json, error, workspace::Workspace};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read},
    path::PathBuf,
};

const USAGE: &str = "Usage: cutbolt [--workspace DIR] [capabilities | mcp | request.json]";

/// An optional leading `--workspace DIR`, else CUTBOLT_WORKSPACE.
fn workspace(args: &mut Vec<OsString>) -> Result<Option<Workspace>> {
    let path = if args.first().is_some_and(|a| a == "--workspace") {
        if args.len() < 2 {
            return Err(error("INVALID_ARGUMENT", USAGE));
        }
        let path = args.remove(1);
        args.remove(0);
        Some(PathBuf::from(path))
    } else {
        std::env::var_os("CUTBOLT_WORKSPACE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    path.map(|p| Workspace::open(&p)).transpose()
}

fn execute(args: &[OsString], workspace: Option<&Workspace>) -> Result<Value> {
    if args.len() > 1
        || args
            .first()
            .is_some_and(|a| a.to_string_lossy().starts_with('-'))
    {
        return Err(error("INVALID_ARGUMENT", USAGE));
    }
    if args.first().is_some_and(|a| a == "capabilities") {
        return handle_json(json!({"command":"capabilities"}), workspace);
    }
    let input: Box<dyn Read> = match args.first() {
        Some(path) => Box::new(std::fs::File::open(path)?),
        None => Box::new(io::stdin()),
    };
    let mut bytes = Vec::new();
    input.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(error("LIMIT_EXCEEDED", "Requests are limited to 4 MiB"));
    }
    handle_json(serde_json::from_slice(&bytes)?, workspace)
}

/// Ends the process. Coverage builds made by tools/impact.py (`--cfg cutbolt_coverage`) first flush
/// their counters, because `process::exit` skips the profiler runtime's exit hook.
fn exit(code: i32) -> ! {
    #[cfg(cutbolt_coverage)]
    {
        unsafe extern "C" {
            fn __llvm_profile_write_file() -> i32;
        }
        // SAFETY: provided by the profiler runtime linked into instrumented builds; called once, at exit.
        unsafe { __llvm_profile_write_file() };
    }
    std::process::exit(code)
}

fn main() {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() >= 2 && args[0] == "job-tool" {
        match cutbolt::jobs::tool_worker(&args[1], &args[2..]) {
            Ok(code) => exit(code),
            Err(e) => {
                eprintln!("{}: {}", e.code, e.message);
                exit(1);
            }
        }
    }
    if args.len() == 2 && args[0] == "job-worker" {
        if let Err(e) = cutbolt::jobs::worker(std::path::Path::new(&args[1])) {
            eprintln!("{}: {}", e.code, e.message);
            exit(1);
        }
        return;
    }
    let outcome = workspace(&mut args).and_then(|workspace| {
        if args.len() == 1 && args[0] == "mcp" {
            return cutbolt::mcp::serve(workspace).map(|()| None);
        }
        execute(&args, workspace.as_ref()).map(Some)
    });
    let (response, code) = match outcome {
        Ok(None) => return,
        Ok(Some(result)) => (json!({"ok":true,"result":result}), 0),
        Err(error) if args.first().is_some_and(|a| a == "mcp") => {
            eprintln!("{}: {}", error.code, error.message);
            exit(1);
        }
        Err(error) => (json!({"ok":false,"error":error}), 1),
    };
    println!("{response}");
    exit(code);
}
