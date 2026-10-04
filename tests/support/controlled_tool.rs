// Original controllable media-tool fixture. Compiled only into an external test directory.
use std::{env,fs,io::{self,Write},process::{Command,Stdio},thread,time::Duration};
fn main() {
    let args:Vec<_>=env::args_os().skip(1).collect();
    if args.first().is_some_and(|v|v=="wait-child") {
        loop {thread::sleep(Duration::from_secs(1));}
    }
    if !args.iter().any(|v|v=="-version") {
        let child=Command::new(env::current_exe().unwrap()).arg("wait-child")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let marker=env::var_os("CUTBOLT_TEST_MARKER").unwrap();
        let gate=env::var_os("CUTBOLT_TEST_GATE").unwrap();
        let output=args.last().unwrap();
        fs::write(output,b"original incomplete fixture output").unwrap();
        fs::write(marker,format!("{{\"parent\":{},\"child\":{}}}",std::process::id(),child.id())).unwrap();
        loop {
            println!("frame=25\nprogress=continue");
            io::stdout().flush().unwrap();
            if std::path::Path::new(&gate).exists() {break;}
            thread::sleep(Duration::from_millis(100));
        }
        fs::remove_file(output).unwrap();
    }
    let status=Command::new(env::var_os("CUTBOLT_TEST_REAL_FFMPEG").unwrap()).args(args).status().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
