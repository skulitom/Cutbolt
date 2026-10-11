"""Installation diagnostics: real tools, controlled failures and read-only workspace checks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from engine import ENGINE
from agents import Client

ROOT = Path(__file__).resolve().parents[1]

# Original executable stand-in; built outside the repository. No external implementation.
STUB = r'''
use std::{env, io::{self, Write}};
fn main() {
    let mode = env::var("CUTBOLT_DOCTOR_TEST_MODE").unwrap_or_default();
    if mode == "failed" { eprintln!("fixture failure"); std::process::exit(7); }
    if mode == "invalid_utf8" { io::stdout().write_all(&[255,254]).unwrap(); return; }
    let exe = env::current_exe().unwrap();
    let name = if exe.file_stem().unwrap().to_string_lossy().contains("probe") {"ffprobe"} else {"ffmpeg"};
    let arg = env::args().last().unwrap();
    match arg.as_str() {
        "-version" if mode == "bad_header" => println!("unrelated program"),
        "-version" => println!("{name} version doctor-fixture\nconfiguration: original-test-stand-in"),
        "-encoders" => {
            println!(" V..... = Video\n A..... = Audio\n V..... ffv1\n A..... pcm_s16le\n V..... png\n A..... aac\n V..... libx264rgb");
            if mode != "missing_encoder" { println!(" V..... libx264"); }
        }
        "-decoders" => {
            println!(" V..... ffv1\n V..... h264\n V..... hevc\n A..... aac\n A..... pcm_s16le\n A..... mp3\n A..... flac");
            if mode != "missing_decoder" { println!(" A..... opus"); }
        }
        "-muxers" => {
            println!(" E = Muxing supported\n E matroska\n E mp4\n E image2\n E rawvideo\n E s16le\n D wav");
            if mode != "missing_muxer" { println!(" E wav"); }
        }
        _ => std::process::exit(8),
    }
}
'''


def snapshot(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else 'directory'
            for p in root.rglob('*')}


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    passed = []
    environment = {k: v for k, v in os.environ.items() if k not in
                   ('CUTBOLT_WORKSPACE', 'CUTBOLT_FFMPEG', 'CUTBOLT_FFPROBE', 'CUTBOLT_DOCTOR_TEST_MODE')}

    def call(*args, env=None, ready=True):
        process = subprocess.run([str(ENGINE), *map(str, args), 'doctor'], capture_output=True,
                                 env=env or environment, timeout=60)
        response = json.loads(process.stdout)
        assert process.returncode == (0 if ready else 1), (process.returncode, response, process.stderr)
        assert response['ok'] and response['result']['ready'] is ready, response
        assert response['result']['runtime_network'] is False
        return response['result']

    actual = call()
    assert actual['workspace']['configured'] is False
    for name in ('ffmpeg', 'ffprobe'):
        tool = actual['tools'][name]
        path = Path(tool['path'])
        assert tool['ready'] and tool['version'].startswith(name + ' version ')
        assert tool['identity'] == {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'bytes': path.stat().st_size}
    assert actual['platform']['audio_capture'] is (os.name == 'nt')
    passed.append('doctor.real_tool_versions_features_and_identities')

    client = Client(ENGINE, env=environment, args=('mcp',))
    try:
        client.initialize()
        catalog = client.rpc('tools/list')['result']['tools']
        assert len(catalog) == 18 and all(item['name'] != 'cutbolt_doctor' for item in catalog)
        result = client.rpc('tools/call', {'name': 'cutbolt_run', 'arguments': {
            'command': 'project.create', 'arguments': {
                'id': 'installation-check', 'width': 32, 'height': 16, 'frame_rate': 25}}})
        assert result['result']['structuredContent']['ok'], result
    finally:
        client.close()
    passed.append('doctor.executable_still_serves_compact_mcp_and_first_command')

    helpers = root / 'tools with spaces'
    helpers.mkdir()
    source = helpers / 'tool.rs'
    source.write_text(STUB, encoding='utf-8')
    suffix = '.exe' if os.name == 'nt' else ''
    ffmpeg, ffprobe = [helpers / (name + suffix) for name in ('ffmpeg', 'ffprobe')]
    subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(ffmpeg)], check=True, capture_output=True)
    shutil.copy2(ffmpeg, ffprobe)
    selected = {**environment, 'CUTBOLT_FFMPEG': str(ffmpeg), 'CUTBOLT_FFPROBE': str(ffprobe)}
    assert call(env=selected)['tools']['ffmpeg']['selected'] == str(ffmpeg)
    discovery = call(env={**environment, 'PATH': str(helpers) + os.pathsep + environment['PATH']})
    assert Path(discovery['tools']['ffmpeg']['path']).samefile(ffmpeg)
    for name in ('ffmpeg', 'ffprobe'):
        response = call(env={**selected, 'CUTBOLT_' + name.upper(): str(root / 'missing-tool')}, ready=False)
        assert not response['tools'][name]['ready'] and response['tools'][name]['hint']
    for mode in ('failed', 'invalid_utf8', 'bad_header'):
        response = call(env={**selected, 'CUTBOLT_DOCTOR_TEST_MODE': mode}, ready=False)
        assert response['tools']['ffmpeg']['error']['code'] == 'TOOL_FAILED', response
    for mode, field, missing in [('missing_encoder', 'encoders', 'libx264'),
                                 ('missing_decoder', 'decoders', 'opus'), ('missing_muxer', 'muxers', 'wav')]:
        response = call(env={**selected, 'CUTBOLT_DOCTOR_TEST_MODE': mode}, ready=False)
        assert response['tools']['ffmpeg'][field]['missing'] == [missing], response
    passed.append('doctor.discovery_and_actionable_tool_failures')

    workspace = root / 'workspace with spaces'
    workspace.mkdir()
    before = snapshot(workspace)
    result = call('--workspace', workspace, env=selected)
    assert snapshot(workspace) == before == {}
    assert all(not p['exists'] for p in result['workspace']['state_directories'])
    assert result['workspace']['write_access'] == 'not_tested'
    assert call(env={**selected, 'CUTBOLT_WORKSPACE': str(workspace)})['workspace']['configured']
    call('--workspace', root / 'absent', env=selected, ready=False)
    assert not (root / 'absent').exists()
    ordinary = workspace / 'ordinary.txt'
    ordinary.write_bytes(b'preserve original bytes')
    call('--workspace', ordinary, env=selected, ready=False)
    state = workspace / '.cutbolt'
    state.write_bytes(b'not a directory')
    before = snapshot(workspace)
    call('--workspace', workspace, env=selected, ready=False)
    assert snapshot(workspace) == before
    state.unlink()  # Only this fixture's own disposable file.
    state.mkdir()
    for name in ('store', 'jobs', 'cache', 'scratch'):
        (state / name).mkdir()
    (state / 'store' / 'preserved.bin').write_bytes(b'opaque store content')
    before = snapshot(workspace)
    call('--workspace', workspace, env=selected)
    assert snapshot(workspace) == before
    passed.append('doctor.workspace_structure_without_writes_or_store_claims')

    linked = root / 'linked'
    outside = root / 'outside'
    linked.mkdir()
    outside.mkdir()
    link = linked / '.cutbolt'
    if os.name == 'nt':
        subprocess.run(['cmd', '/c', 'mklink', '/J', str(link), str(outside)], check=True, capture_output=True)
    else:
        link.symlink_to(outside, target_is_directory=True)
    result = call('--workspace', linked, env=selected, ready=False)
    assert all(not p['inside_workspace'] for p in result['workspace']['state_directories'])
    assert list(outside.iterdir()) == []
    passed.append('doctor.rejects_escaping_parent_even_when_state_children_are_missing')
    (root / 'verification.json').write_text(json.dumps({'passed': passed, 'actual': actual}, indent=2) + '\n', encoding='utf-8')
    print(f'{len(passed)} installation diagnostic checks passed')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
