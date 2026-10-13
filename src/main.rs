use cutbolt::{Result, commands::handle_json, error, workspace::Workspace};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read},
    path::PathBuf,
};

const USAGE: &str = "Usage: cutbolt --version | [--workspace DIR] [doctor | capabilities | mcp [--tools core|full] | request.json]";

/// An optional leading `--workspace DIR`, else CUTBOLT_WORKSPACE.
fn workspace_path(args: &mut Vec<OsString>) -> Result<Option<PathBuf>> {
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
    Ok(path)
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
    if args.len() == 1 && args[0] == "job-command" {
        if let Err(e) = cutbolt::jobs::command_process() {
            eprintln!("{}: {}", e.code, e.message);
            exit(1);
        }
        exit(0);
    }
    if args.len() == 1 && args[0] == "--version" {
        println!("{}", cutbolt::version());
        exit(0);
    }
    if args.len() == 2 && args[0] == "job-worker" {
        if let Err(e) = cutbolt::jobs::worker(std::path::Path::new(&args[1])) {
            eprintln!("{}: {}", e.code, e.message);
            exit(1);
        }
        return;
    }
    let outcome = workspace_path(&mut args).and_then(|path| {
        if args.len() == 1 && args[0] == "doctor" {
            return Ok(Some(cutbolt::doctor::inspect(path.as_deref())));
        }
        let workspace = path.map(|p| Workspace::open(&p)).transpose()?;
        // Workspace caches stay together even when a request selects a narrower input root.
        // Explicit environment settings win. Queue workers inherit both locations.
        if let Some(workspace) = &workspace {
            for (variable, path) in [
                (
                    cutbolt::inspection_cache::DIRECTORY_VARIABLE,
                    ".cutbolt/cache/inspections",
                ),
                ("CUTBOLT_CONFORM_CACHE", ".cutbolt/cache/conform"),
            ] {
                if std::env::var_os(variable).is_none() {
                    // SAFETY: still single-threaded; nothing else accesses the environment yet.
                    unsafe { std::env::set_var(variable, workspace.inside(path)) };
                }
            }
        }
        if args.first().is_some_and(|a| a == "mcp") {
            // Compact by default; a flag overrides CUTBOLT_MCP_TOOLS. Full remains opt-in.
            let choice = match args.len() {
                1 => std::env::var("CUTBOLT_MCP_TOOLS").unwrap_or_else(|_| "core".into()),
                3 if args[1] == "--tools" => args[2].to_string_lossy().into_owned(),
                _ => return Err(error("INVALID_ARGUMENT", USAGE)),
            };
            let core = match choice.as_str() {
                "core" => true,
                "full" => false,
                _ => return Err(error("INVALID_ARGUMENT", "--tools takes core or full")),
            };
            return cutbolt::mcp::serve(workspace, core).map(|()| None);
        }
        execute(&args, workspace.as_ref()).map(Some)
    });
    let (response, code) = match outcome {
        Ok(None) => return,
        Ok(Some(result)) => {
            // Diagnostics succeeded even if setup needs attention. Keep the report available,
            // and use a nonzero exit status so installation scripts can stop before rendering.
            let failed = args.len() == 1 && args[0] == "doctor" && result["ready"] != true;
            (json!({"ok":true,"result":result}), i32::from(failed))
        }
        Err(error) if args.first().is_some_and(|a| a == "mcp") => {
            eprintln!("{}: {}", error.code, error.message);
            exit(1);
        }
        Err(error) => (json!({"ok":false,"error":error}), 1),
    };
    println!("{response}");
    exit(code);
}
