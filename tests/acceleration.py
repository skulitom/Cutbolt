"""Actual optional CUDA decoding against software reference pixels and source clocks."""
from engine import ENGINE
import argparse
from bisect import bisect_right
import copy
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import time as clock

import numpy as np
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]


def run(root, device):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = [root / name for name in ('sources', 'output', 'store')]
    for p in (sources, output, store):
        p.mkdir()
    exe = ENGINE
    passed, measured, cases = [], [], []
    frames = samples = rejected = 0
    source_reference = {}

    def ff(args):
        return subprocess.run(['ffmpeg', '-hide_banner', '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=180).stdout

    def digest(path):
        with path.open('rb') as f:
            return hashlib.file_digest(f, 'sha256').hexdigest()

    def call(command, error=None, env=None, **fields):
        nonlocal rejected
        request = {'command': command, **fields}
        p = subprocess.run([str(exe)], input=json.dumps(request).encode(), capture_output=True, env=env, timeout=240)
        value = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and value['error']['code'] == error, (error, value)
            rejected += 1
            return value
        if p.returncode:
            (root / 'failed-request.json').write_text(json.dumps(request, indent=2), encoding='utf-8')
        assert p.returncode == 0 and value['ok'], value
        return value['result']

    definitions = [('cfr', 320, 180, 80, '25', False), ('fractional', 320, 180, 80, '30000/1001', False), ('vfr', 320, 180, 80, '25', True), ('minimum', 128, 128, 16, '25', False), ('maximum', 1920, 1080, 100, '25', False)]
    for name, w, h, count, rate, vfr in definitions:
        y, x = np.indices((h, w))
        raw = sources / f'{name}.rgb'
        with raw.open('wb') as f:
            for n in range(count):
                picture = np.stack(((x//3+n*11)%256, (y//2+n*7)%256, ((x//16+y//16+n//3)%2)*160+48), axis=-1).astype('uint8')
                f.write(picture.tobytes())
        audio = sources / f'{name}.pcm'
        ticks = np.arange(4*48000, dtype='int64')
        pcm = np.stack(((ticks*71)%36001-18000, (ticks*53)%32001-16000), axis=-1).astype('<i2')
        audio.write_bytes(pcm.tobytes())
        movie = sources / f'{name}.mp4'
        args = ['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{w}x{h}', '-framerate', rate, '-i', str(raw), '-f', 's16le', '-ar', '48000', '-ac', '2', '-i', str(audio), '-map', '0:v:0', '-map', '1:a:0', '-c:v', 'libx264', '-crf', '16', '-g', '12', '-bf', '0' if vfr else '2', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-b:a', '320k', '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709', '-color_range', 'tv']
        if vfr:
            # The pinned muxer omits final video duration for this VFR+B-frame
            # combination; explicit no-B VFR retains the source-clock contract.
            args += ['-vf', 'settb=1/1000,setpts=N*40+floor(N/8)*20', '-fps_mode:v', 'passthrough', '-enc_time_base:v', '1/1000']
        ff(args + [str(movie)])
        reference_path = sources / f'{name}.reference.rgb'
        ff(['-noautorotate', '-i', str(movie), '-map', '0:v:0', '-an', '-fps_mode', 'passthrough', '-vf', 'scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd', '-pix_fmt', 'rgb24', '-f', 'rawvideo', str(reference_path)])
        reference = np.memmap(reference_path, dtype='uint8', mode='r').reshape((-1, h, w, 3))
        metadata = json.loads(subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'v:0', '-show_streams', '-show_frames', '-show_entries', 'stream=time_base:frame=best_effort_timestamp', '-of', 'json', str(movie)], capture_output=True, check=True).stdout)
        tb = F(metadata['streams'][0]['time_base'])
        pts = [F(f['best_effort_timestamp'])*tb for f in metadata['frames']]
        audio_reference = ff(['-i', str(movie), '-map', '0:a:0', '-f', 's16le', '-'])
        source_reference[name] = reference, audio_reference, pts, w, h
    original = {p.name: digest(p) for p in sources.iterdir()}

    def recipe(name, start=F(2, 25), duration=F(8, 25), rate=F(1), reverse=False, freeze=False, resample=True):
        _, _, _, w, h = source_reference[name]
        return {'schema_version': 1, 'id': name, 'source': {'file': identity(sources / f'{name}.mp4', sources), 'color': 'bt709_limited'}, 'source_in': time(start.numerator, start.denominator), 'duration': time(duration.numerator, duration.denominator), 'rate': time(rate.numerator, rate.denominator), 'reverse': reverse, 'freeze': freeze, 'width': w, 'height': h, 'audio': 'resample' if resample else 'mute'}

    def verify(request, label, mode, expected_backend):
        nonlocal frames, samples
        request = copy.deepcopy(request)
        request['decode'] = mode
        path = output / f'{label}.mkv'
        started = clock.perf_counter()
        result = call('media.conform', recipe=request, input_root=str(sources), output_root=str(output), output=str(path))
        elapsed = clock.perf_counter()-started
        assert result['decode']['selected_backend'] == expected_backend, result['decode']
        if expected_backend == 'cuda':
            assert result['decode']['device'] == device and result['decode']['fallback'] is None
        source, audio, pts, w, h = source_reference[request['id']]
        start = F(request['source_in']['num'], request['source_in']['den'])
        duration = F(request['duration']['num'], request['duration']['den'])
        rate = F(request['rate']['num'], request['rate']['den'])
        count = int(duration*25)
        indices = []
        for n in range(count):
            t = start if request['freeze'] else start+F(n, 25)*rate*(-1 if request['reverse'] else 1)
            indices.append(bisect_right(pts, t)-1)
        # Stream the output against mapped reference frames, so the full-HD
        # comparison does not allocate whole movies or duplicate large arrays.
        decoder = subprocess.Popen(['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        maximum_error = error_sum = 0
        try:
            for index in indices:
                pixels = decoder.stdout.read(w*h*3)
                assert len(pixels) == w*h*3, (label, 'short decoded frame')
                actual = np.frombuffer(pixels, dtype='uint8')
                wanted = source[index].reshape(-1)
                errors = np.abs(actual.astype('int16')-wanted.astype('int16'))
                maximum_error = max(maximum_error, int(errors.max()))
                error_sum += int(errors.sum())
            assert decoder.stdout.read(1) == b'', (label, 'extra decoded frame')
            assert decoder.wait(timeout=30) == 0, decoder.stderr.read()
        finally:
            if decoder.poll() is None:
                decoder.kill()
                decoder.wait(timeout=30)
            decoder.stdout.close()
            decoder.stderr.close()
        assert maximum_error <= 1, (label, maximum_error)
        actual_pcm = ff(['-i', str(path), '-map', '0:a:0', '-f', 's16le', '-'])
        if request['audio'] == 'mute':
            wanted_pcm = bytes(count*1920*4)
        else:
            # Resampling at unity speed: exact original decoded AAC sample prefix.
            assert rate == 1 and not request['reverse'] and not request['freeze']
            first = int(start*48000)
            wanted_pcm = audio[first*4:(first+count*1920)*4]
        assert actual_pcm == wanted_pcm, label
        frames += count
        samples += count*1920
        case = {'label': label, 'requested': mode, 'backend': expected_backend, 'frames': count, 'maximum_rgb_error': maximum_error, 'mean_rgb_error': error_sum/(count*w*h*3), 'wall_seconds': elapsed, 'timing': result['timing'], 'availability_micros': result['decode']['availability_micros']}
        measured.append(case)
        cases.append(label)
        return result

    gpu = {'backend': 'cuda', 'device': device, 'unavailable': 'error'}
    cpu = {'backend': 'cpu'}
    for name, *_ in definitions:
        r = recipe(name)
        verify(r, name+'-cpu', cpu, 'cpu')
        gpu_result = verify(r, name+'-gpu', gpu, 'cuda')
        if name == 'cfr':
            saved_gpu = gpu_result
    for name, options in [('source-offset', {'start': F(7, 25)}), ('reverse', {'start': F(1), 'reverse': True, 'resample': False}), ('freeze', {'freeze': True, 'resample': False}), ('double-rate', {'rate': F(2), 'resample': False})]:
        verify(recipe('fractional', **options), name, gpu, 'cuda')
    passed.append('acceleration.actual_cuda_decode_clocks_and_numerical_tolerance')

    # Ordinal 31 is intentionally absent from the bounded test environment.
    # The actual runtime must report this; no mock supplies the availability result.
    fallback = {'backend': 'cuda', 'device': 31, 'unavailable': 'cpu'}
    result = verify(recipe('cfr'), 'actual-unavailable-fallback', fallback, 'cpu')
    assert result['decode']['fallback']['reason'] == 'device_initialization_failed'
    for mode, code in [({**fallback, 'unavailable': 'error'}, 'DEVICE_UNAVAILABLE'), ({**gpu, 'device': 32}, 'INVALID_ACCELERATION')]:
        r = recipe('cfr');r['decode'] = mode
        target = output / f'rejected-{rejected}.mkv'
        call('media.conform', recipe=r, input_root=str(sources), output_root=str(output), output=str(target), error=code)
        assert not target.exists()
    bad = recipe('cfr');bad['decode'] = gpu;bad['source']['file']['sha256'] = '0'*64
    call('media.conform', recipe=bad, input_root=str(sources), output_root=str(output), output=str(output/'changed.mkv'), error='MEDIA_CHANGED')
    held = output / 'cfr-gpu.mkv'
    prior = hashlib.sha256(held.read_bytes()).hexdigest()
    r = recipe('cfr');r['decode'] = gpu
    call('media.conform', recipe=r, input_root=str(sources), output_root=str(output), output=str(held), error='OUTPUT_EXISTS')
    assert hashlib.sha256(held.read_bytes()).hexdigest() == prior
    unsupported = recipe('cfr')
    unsupported['source'] = {'file': identity(held, output), 'color': 'encoded_rgb'}
    unsupported['decode'] = gpu
    unsupported['source_in'] = time(0)
    unsupported['audio'] = 'mute'
    call('media.conform', recipe=unsupported, input_root=str(output), output_root=str(output), output=str(output/'unsupported.mkv'), error='UNSUPPORTED_ACCELERATION')
    assert not list(output.glob('.cutbolt-*'))
    passed.append('acceleration.actual_device_fallback_and_failure_preservation')

    # Real device initialization succeeds first; controlled later failures must
    # not be hidden by the unavailable-device fallback policy or publish output.
    wrapper = root / 'controlled.rs'
    wrapper.write_text(r'''
use std::{env,fs,io::Write,path::Path,process::{Command,exit},thread,time::Duration};
fn main(){
 let args:Vec<String>=env::args().skip(1).collect();
 let root=fs::canonicalize(env::var("CUTBOLT_DECODE_FIXTURE_ROOT").unwrap()).unwrap();
 let mode=env::var("CUTBOLT_DECODE_FIXTURE_MODE").unwrap();
 let init=args.iter().any(|x|x=="-init_hw_device");
 let decode=args.iter().any(|x|x=="-hwaccel");
 let software=args.last().is_some_and(|p|Path::new(p).file_name().is_some_and(|n|n=="source.rgb"))&&!decode;
 let mut log=fs::OpenOptions::new().create(true).append(true).open(root.join("calls.txt")).unwrap();
 writeln!(log,"{}",if init{"init"}else if decode{"hardware_decode"}else if software{"software_decode"}else{"other"}).unwrap();
 if init&&mode=="timeout"{fs::write(root.join("timed-pid.txt"),std::process::id().to_string()).unwrap();thread::sleep(Duration::from_secs(30));exit(0);}
 if decode {
  let path=Path::new(args.last().unwrap());
  assert!(fs::canonicalize(path.parent().unwrap()).unwrap().starts_with(&root));
  assert_eq!(path.file_name().unwrap(),"source.rgb");
  if mode=="decode_fail"{fs::write(path,b"incomplete original fixture output").unwrap();exit(73);}
 }
 let status=Command::new(env::var("CUTBOLT_DECODE_REAL_TOOL").unwrap()).args(&args).status().unwrap();
 if !status.success(){exit(status.code().unwrap_or(1));}
 if decode&&mode=="truncate"{let file=fs::OpenOptions::new().write(true).open(args.last().unwrap()).unwrap();let length=file.metadata().unwrap().len();file.set_len(length-3).unwrap();}
 if decode&&mode=="source"{let source=fs::canonicalize(env::var("CUTBOLT_DECODE_FIXTURE_SOURCE").unwrap()).unwrap();assert!(source.starts_with(root.join("sources")));let mut file=fs::OpenOptions::new().append(true).open(source).unwrap();file.write_all(b"original fixture source change").unwrap();}
}
''', encoding='utf-8')
    helper = root / 'controlled.exe'
    subprocess.run(['rustc', str(wrapper), '-o', str(helper)], capture_output=True, check=True)
    environment = {**os.environ, 'CUTBOLT_FFMPEG': str(helper), 'CUTBOLT_DECODE_REAL_TOOL': shutil.which('ffmpeg'), 'CUTBOLT_DECODE_FIXTURE_ROOT': str(root), 'CUTBOLT_DECODE_FIXTURE_SOURCE': str(sources/'cfr.mp4')}
    source_bytes = (sources/'cfr.mp4').read_bytes()
    for mode, code in [('decode_fail', 'TOOL_FAILED'), ('truncate', 'RENDER_VALIDATION_FAILED'), ('source', 'MEDIA_CHANGED'), ('timeout', 'TOOL_TIMEOUT')]:
        r = recipe('cfr');r['decode'] = {**gpu, 'unavailable': 'cpu'}
        destination = output / f'controlled-{mode}.mkv'
        (root/'calls.txt').write_text('')
        try:
            call('media.conform', recipe=r, input_root=str(sources), output_root=str(output), output=str(destination), error=code, env={**environment, 'CUTBOLT_DECODE_FIXTURE_MODE': mode})
        finally:
            if (sources/'cfr.mp4').read_bytes() != source_bytes:
                (sources/'cfr.mp4').write_bytes(source_bytes)
        calls = (root/'calls.txt').read_text().splitlines()
        selected_calls = [name for name in calls if name != 'other']
        assert selected_calls == (['init'] if mode == 'timeout' else ['init', 'hardware_decode']), calls
        assert not destination.exists() and not list(output.glob('.cutbolt-*'))
    for policy in ['cpu', 'error']:
        r = recipe('cfr');r['decode'] = {**gpu, 'unavailable': policy}
        call('media.conform', recipe=r, input_root=str(sources), output_root=str(output), output=str(output/f'missing-{policy}.mkv'), error='TOOL_UNAVAILABLE', env={**os.environ, 'CUTBOLT_FFMPEG': str(root/'missing.exe')})
    passed.append('acceleration.late_decode_faults_timeout_and_cleanup')

    normalized = recipe('cfr')
    normalized['source'].pop('color')
    normalized['source']['sdr'] = {'matrix': 'bt709', 'range': 'limited', 'transfer': 'bt709', 'missing_tags': 'reject'}
    normalized.update(working_transfer='srgb', width=160, height=90)
    normalized_pixels = []
    for backend, mode in [('cpu', cpu), ('cuda', gpu)]:
        normalized['decode'] = mode
        destination = output / f'normalized-{backend}.mkv'
        result = call('media.conform', recipe=normalized, input_root=str(sources), output_root=str(output), output=str(destination))
        assert result['decode']['selected_backend'] == backend
        normalized_pixels.append(np.frombuffer(ff(['-i', str(destination), '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']), dtype='uint8'))
    normalized_error = int(np.abs(normalized_pixels[0].astype('int16')-normalized_pixels[1].astype('int16')).max())
    assert normalized_error <= 1
    passed.append('acceleration.normalization_and_resize_pipeline_agreement')

    client = Client(exe)
    try:
        client.initialize()
        catalog = client.rpc('tools/list')['result']['tools']
        schema = next(t['inputSchema'] for t in catalog if t['name'] == 'cutbolt_media_conform_inspect')
        r = recipe('cfr');r['decode'] = gpu
        fields = {'recipe': r, 'input_root': str(sources)}
        Draft202012Validator(schema).validate(fields)
        report = client.call('media.conform.inspect', **fields)
        assert report['decode']['selected_backend'] == 'cuda'
    finally:
        client.close()
    result = saved_gpu
    project = call('project.create', id='accelerated-source', width=320, height=180, frame_rate=time(25))
    call('session.create', project=project, store_root=str(store), request_id='create')
    call('session.apply', store_root=str(store), project_id=project['id'], expected_revision=0, request_id='register', operations=[{'op': 'media.add', 'asset': result['asset']}, {'op': 'clip.append', 'clip': {'id': 'shot', 'asset_id': result['asset']['id'], 'source_in': time(0), 'duration': result['asset']['duration']}}])
    saved = call('session.get', store_root=str(store), project_id=project['id'])
    call('preview.range', project=saved, input_root=str(output), output_root=str(output), output=str(output/'saved-preview.mkv'), start=time(0), duration=result['asset']['duration'])
    assert ff(['-i', str(output/'saved-preview.mkv'), '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']) == ff(['-i', result['output'], '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
    passed.append('acceleration.typed_inspection_and_saved_native_asset')

    # Record both complete native-command and isolated decode-stage throughput.
    # Hardware setup/readback can make small inputs slower; no speedup is presumed.
    for repeat in range(3):
        for label, mode in [('cpu', cpu), ('cuda', gpu)]:
            verify(recipe('cfr', start=F(0), duration=F(3)), f'throughput-{repeat}-{label}', mode, label)
    throughput = {}
    for label in ('cpu', 'cuda'):
        values = [v for v in measured if v['label'].startswith('throughput-') and v['backend'] == label]
        throughput[label] = {'wall_seconds': [v['wall_seconds'] for v in values], 'median_output_fps': 75/statistics.median(v['wall_seconds'] for v in values), 'decode_seconds': [v['timing']['decode_video_micros']/1000000 for v in values], 'median_source_decode_fps': 80/statistics.median(v['timing']['decode_video_micros']/1000000 for v in values)}
    for repeat in range(3):
        for label, mode in [('cpu', cpu), ('cuda', gpu)]:
            verify(recipe('maximum', start=F(0), duration=F(4)), f'full-hd-throughput-{repeat}-{label}', mode, label)
    for label in ('cpu', 'cuda'):
        values = [v for v in measured if v['label'].startswith('full-hd-throughput-') and v['backend'] == label]
        throughput['full_hd_'+label] = {'wall_seconds': [v['wall_seconds'] for v in values], 'median_output_fps': 100/statistics.median(v['wall_seconds'] for v in values), 'decode_seconds': [v['timing']['decode_video_micros']/1000000 for v in values], 'median_source_decode_fps': 100/statistics.median(v['timing']['decode_video_micros']/1000000 for v in values)}
    passed.append('acceleration.measured_native_and_decode_stage_throughput')
    assert original == {p.name: digest(p) for p in sources.iterdir()}
    result = {'passed': passed, 'frames_compared': frames, 'stereo_sample_frames_compared': samples, 'rejections': rejected, 'cases': measured, 'throughput': throughput, 'numerical_tolerance': {'rgb_maximum': 1, 'pcm': 'exact'}, 'reference': 'Original generated RGB/PCM encoded as H.264/AAC. Independently decoded software RGB and AAC, exact Fraction/PTS lookup and sample slicing; actual hardware path plus a real unavailable ordinal. GPU decode only; timestamp inspection, conversion, mapping, FFV1 output and validation remain CPU work.'}
    (root/'verification.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--device', required=True, type=int)
    args = parser.parse_args()
    run(args.output, args.device)
