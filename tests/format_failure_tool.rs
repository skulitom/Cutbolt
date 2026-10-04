//! Original test adapter: run the real encoder, then perturb only an owned image fixture.
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, exit},
};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let status = Command::new(env::var("CUTBOLT_FORMAT_REAL").unwrap())
        .args(&args)
        .status()
        .unwrap();
    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
    let Some(pattern) = args.last().filter(|p| p.ends_with("frame-%06d.png")) else {
        return;
    };
    let pattern = PathBuf::from(pattern);
    let root = fs::canonicalize(env::var("CUTBOLT_FORMAT_ROOT").unwrap()).unwrap();
    let parent = fs::canonicalize(pattern.parent().unwrap()).unwrap();
    assert!(parent.starts_with(&root));
    let mut files: Vec<_> = fs::read_dir(&parent)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    files.sort();
    assert!(files.len() >= 3);
    match env::var("CUTBOLT_FORMAT_MODE").unwrap().as_str() {
        "missing" => fs::remove_file(&files[files.len() / 2]).unwrap(),
        "corrupt" => fs::write(&files[files.len() / 2], b"original corrupt fixture").unwrap(),
        "swapped" => {
            fs::copy(&files[0], files.last().unwrap()).unwrap();
        }
        "failure" => exit(74),
        "occupied" => {
            let target = PathBuf::from(env::var("CUTBOLT_FORMAT_DESTINATION").unwrap());
            assert!(
                fs::canonicalize(target.parent().unwrap())
                    .unwrap()
                    .starts_with(&root)
            );
            fs::create_dir(&target).unwrap();
            fs::write(
                target.join("preserve.txt"),
                b"original occupied destination",
            )
            .unwrap();
        }
        "changed" => {
            use std::io::Write;
            let source = fs::canonicalize(env::var("CUTBOLT_FORMAT_SOURCE").unwrap()).unwrap();
            assert!(source.starts_with(
                fs::canonicalize(env::var("CUTBOLT_FORMAT_SOURCES").unwrap()).unwrap()
            ));
            fs::OpenOptions::new()
                .append(true)
                .open(source)
                .unwrap()
                .write_all(b"original changed source")
                .unwrap();
        }
        _ => panic!("unknown fixture mode"),
    }
}
