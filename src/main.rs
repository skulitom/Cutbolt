use cutbolt::{
    Result,
    commands::{Request, handle},
    error,
};
use serde_json::{Value, json};
use std::io::{self, Read};

fn execute() -> Result<Value> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() > 1 {
        return Err(error(
            "INVALID_ARGUMENT",
            "Usage: cutbolt [capabilities | mcp | request.json]",
        ));
    }
    if args.first().is_some_and(|a| a == "capabilities") {
        return handle(Request::Capabilities {});
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
    handle(serde_json::from_slice(&bytes)?)
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "mcp" {
        if let Err(e) = cutbolt::mcp::serve() {
            eprintln!("{}: {}", e.code, e.message);
            std::process::exit(1);
        }
        return;
    }
    if args.len() >= 2 && args[0] == "job-tool" {
        match cutbolt::jobs::tool_worker(&args[1], &args[2..]) {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("{}: {}", e.code, e.message);
                std::process::exit(1);
            }
        }
    }
    if args.len() == 2 && args[0] == "job-worker" {
        if let Err(e) = cutbolt::jobs::worker(std::path::Path::new(&args[1])) {
            eprintln!("{}: {}", e.code, e.message);
            std::process::exit(1);
        }
        return;
    }
    let (response, code) = match execute() {
        Ok(result) => (json!({"ok":true,"result":result}), 0),
        Err(error) => (json!({"ok":false,"error":error}), 1),
    };
    println!("{response}");
    std::process::exit(code);
}
