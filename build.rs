use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn sources(path: &Path, output: &mut Vec<PathBuf>) {
    for item in fs::read_dir(path).expect("source directory") {
        let path = item.expect("source entry").path();
        if path.is_dir() {
            sources(&path, output);
        } else if path.extension().is_some_and(|x| x == "rs") {
            output.push(path);
        }
    }
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "tools/transcribe_worker.py",
        "tools/transcribe_supervisor.py",
    ] {
        files.push(root.join(name));
        println!("cargo:rerun-if-changed={name}");
    }
    println!("cargo:rerun-if-changed=src");
    files.sort();
    let mut generated = String::from("pub const ENGINE_SOURCES: &[(&str, &[u8])] = &[\n");
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .expect("local source")
            .to_string_lossy()
            .replace('\\', "/");
        generated.push_str(&format!(
            "({relative:?}, include_bytes!({:?})),\n",
            file.to_string_lossy()
        ));
    }
    generated.push_str("];\n");
    let compiler = std::process::Command::new(env::var_os("RUSTC").expect("Rust compiler"))
        .arg("-Vv")
        .output()
        .expect("compiler version");
    assert!(compiler.status.success(), "compiler version unavailable");
    let mut configuration = String::from_utf8(compiler.stdout).expect("compiler version UTF-8");
    for name in [
        "TARGET",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        configuration.push_str(&format!(
            "{name}={:?}\n",
            env::var(name).unwrap_or_default()
        ));
    }
    generated.push_str(&format!(
        "pub const BUILD_CONFIGURATION: &str = {configuration:?};\n"
    ));
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("build output")).join("engine_sources.rs"),
        generated,
    )
    .expect("source identity output");
}
