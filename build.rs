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
/// Files besides `src` whose bytes are part of the engine's identity.
const ENGINE_FILES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "tools/transcribe_worker.py",
    "tools/transcribe_supervisor.py",
];
/// The Git commit checked out at build time and whether the engine's files differ from it, or
/// `None` when Git cannot tell. The build reruns when this worktree's HEAD moves; edits to the
/// engine's files rerun it already.
fn commit(root: &Path) -> Option<(String, bool)> {
    let git = |args: &[&str]| -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("--no-optional-locks")
            .args(args)
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        output.status.success().then(|| text.trim().to_owned())
    };
    let commit = git(&["rev-parse", "--verify", "HEAD"])?;
    // HEAD's reflog grows with every commit, reset or checkout in this worktree alone; a branch
    // without a loose ref file lives in packed-refs.
    let path = |name: &str| git(&["rev-parse", "--git-path", name]).map(|p| root.join(p));
    let mut watched = vec![path("HEAD"), path("logs/HEAD")];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let loose = path(&branch);
        let packed = !loose.as_ref().is_some_and(|p| p.is_file());
        watched.push(loose);
        watched.push(packed.then(|| path("packed-refs")).flatten());
    }
    for path in watched.into_iter().flatten().filter(|p| p.is_file()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let mut status = vec!["status", "--porcelain", "--", "src"];
    status.extend(ENGINE_FILES);
    let modified = !git(&status)?.is_empty();
    Some((commit, modified))
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    for name in ENGINE_FILES {
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
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output"));
    fs::write(output.join("engine_sources.rs"), generated).expect("source identity output");
    // Kept apart from the identity above, so a new commit of the same sources shares caches.
    fs::write(output.join("commit.rs"), format!("{:?}", commit(&root))).expect("commit output");
}
