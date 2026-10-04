"""Original clean/noisy signals and independent whole-buffer spectral references."""
from engine import ENGINE
import argparse
import budgets
import base64
import copy
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import time as clock
import uuid
import wave

import numpy as np
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from audio_routing import effects_reference
from registry import peak_memory
from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]
RATE, N, HOP = 48000, 4096, 1024
WINDOW = np.sin(np.pi*(np.arange(N)+.5)/N)**2


def samples(t):
    assert t['num']*RATE % t['den'] == 0
    return t['num']*RATE//t['den']


def rounded(x):
    return np.copysign(np.floor(np.abs(x)+.5), x).astype(np.int64)


def reference(recipe, source):
    start, count = samples(recipe['source_in']), samples(recipe['duration'])
    data = source[start:start+count].astype(float)
    noise = recipe.get('noise'); profiles = []
    if noise:
        strength = noise.get('strength_milli', 1250)/1000
        floor = noise.get('floor_milli', 100)/1000
        for ch in range(source.shape[1]):
            training = []
            for region in sorted(noise['regions'], key=lambda r: samples(r['start'])):
                at, length = samples(region['start']), samples(region['duration'])
                training.extend(source[p:p+N, ch]*WINDOW for p in range(at, at+length-N+1, HOP))
            power = np.mean(abs(np.fft.rfft(np.array(training), axis=1))**2, axis=0)
            profiles.append(power)
            if strength == 0 or floor == 1 or not np.any(power): continue
            padded = np.pad(data[:, ch], (N, N)); total = np.zeros_like(padded); norm = np.zeros_like(padded)
            for at in range(0, len(padded)-N+1, HOP):
                spectrum = np.fft.rfft(padded[at:at+N]*WINDOW)
                observed = abs(spectrum)**2
                ratio = np.divide(strength*power, observed, out=np.full_like(power, np.inf), where=observed > 0)
                ratio[(power == 0) & (observed == 0)] = 0
                gain = np.sqrt(np.maximum(floor**2, 1-ratio))
                gain = np.convolve(np.pad(gain, (2, 2), mode='edge'), np.array([1, 2, 3, 2, 1])/9, mode='valid')
                total[at:at+N] += np.fft.irfft(spectrum*gain, n=N)*WINDOW
                norm[at:at+N] += WINDOW**2
            data[:, ch] = total[N:N+count]/norm[N:N+count]
    dc = np.mean(data, axis=0) if recipe.get('remove_dc') else np.zeros(source.shape[1])
    wide = rounded(data-dc)
    if recipe.get('effects'): wide = np.array(effects_reference(wide.tolist(), recipe['effects']), dtype=np.int64)
    clipped = np.sum((wide < -32768) | (wide > 32767), axis=0).tolist()
    return np.clip(wide, -32768, 32767).astype('<i2'), profiles, dc, clipped


def speech_fixtures(directory):
    """Original held-out passages; installed voices and generated audio stay external."""
    dest = str(directory).replace("'", "''")
    script = f"""$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(48000,[System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,[System.Speech.AudioFormat.AudioChannel]::Mono)
$items = @(
    @{{id='spoken-a';voice='Microsoft Hazel Desktop';text='Fresh snow covered the garden. Please keep the soft consonants clear. A bright yellow bicycle stood beside the gate.'}},
    @{{id='spoken-b';voice='Microsoft David Desktop';text='Three children shared a quiet story. Listen for the first and final words. The clock continued ticking after sunset.'}}
)
try {{
    foreach ($item in $items) {{
        $path = Join-Path '{dest}' ($item.id + '.wav')
        if (Test-Path -LiteralPath $path) {{ throw 'Speech fixture already exists' }}
        $synth.SelectVoice($item.voice)
        $synth.SetOutputToWaveFile($path,$format)
        $synth.Speak($item.text)
        $synth.SetOutputToNull()
    }}
    @{{assembly=[System.Speech.Synthesis.SpeechSynthesizer].Assembly.FullName;os=[System.Environment]::OSVersion.VersionString;fixtures=$items}} | ConvertTo-Json -Depth 5 -Compress
}} finally {{ $synth.Dispose() }}
"""
    encoded = base64.b64encode(script.encode('utf-16-le')).decode('ascii')
    result = subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded], capture_output=True, check=True, timeout=60)
    metadata = json.loads(result.stdout.decode('utf-8-sig'))
    audio = {}
    for item in metadata['fixtures']:
        path = directory/(item['id']+'.wav')
        with wave.open(str(path), 'rb') as f:
            assert (f.getframerate(), f.getnchannels(), f.getsampwidth()) == (RATE, 1, 2)
            audio[item['id']] = np.frombuffer(f.readframes(f.getnframes()), '<i2').astype(float)
        item['sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
    return audio, metadata


def band(signal, low, high):
    spectrum = np.fft.rfft(signal)
    frequency = np.fft.rfftfreq(len(signal), 1/RATE)
    spectrum[(frequency < low) | (frequency > high)] = 0
    return np.fft.irfft(spectrum, n=len(signal))


def concentration(signal):
    frames = np.lib.stride_tricks.sliding_window_view(signal, 512)[::256]
    power = abs(np.fft.rfft(frames*np.hanning(512), axis=1))[:, 2:-2]**2
    return float(np.percentile(10*np.log10(np.max(power, axis=1)/np.maximum(np.mean(power, axis=1), 1e-30)), 95))


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = (root/n for n in ('sources', 'output', 'store'))
    for directory in (sources, output, store): directory.mkdir()
    exe = ENGINE; fixtures = {}; passed = []; cases = []; rejected = 0
    original_sources = {}; published_outputs = {}

    def write(name, values, rate=RATE):
        values = np.asarray(values, dtype='<i2')
        if values.ndim == 1: values = values[:, None]
        with wave.open(str(sources/name), 'wb') as f:
            f.setparams((values.shape[1], 2, rate, 0, 'NONE', 'not compressed')); f.writeframes(values.tobytes())
        fixtures[name] = values
        original_sources[sources/name] = hashlib.sha256((sources/name).read_bytes()).hexdigest()

    def call(request, error=None):
        nonlocal rejected
        p = subprocess.run([str(exe)], input=json.dumps(request).encode(), capture_output=True, timeout=240)
        value = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and value['error']['code'] == error, value
            rejected += 1; return value
        assert p.returncode == 0 and value['ok'], value
        if 'output' in request:
            path = Path(request['output'])
            published_outputs[path] = hashlib.sha256(path.read_bytes()).hexdigest()
        return value['result']

    def recipe(name, start=0, length=None, **settings):
        return {'schema_version': 1, 'id': 'repair-fixture', 'source': identity(sources/name, sources),
                'source_in': time(start, RATE), 'duration': time(length or len(fixtures[name])-start, RATE), **settings}

    def request(rec, name):
        return {'command': 'audio.repair.render', 'recipe': rec, 'input_root': str(sources), 'output_root': str(output), 'output': str(output/(name+'.wav'))}

    def check(rec, name, expected=None, tolerance=1, inspect=True):
        wanted, profiles, dc, clipped = reference(rec, fixtures[rec['source']['path']]) if expected is None else (expected, [], None, [0]*expected.shape[1])
        receipt = call(request(rec, name)); path = output/(name+'.wav'); raw = path.read_bytes(); channels = wanted.shape[1]
        assert raw[:4] == b'RIFF' and struct.unpack_from('<I', raw, 4)[0]+8 == len(raw)
        assert raw[12:16] == b'fmt ' and struct.unpack_from('<I', raw, 16)[0] == 40
        assert struct.unpack_from('<HHIIHHHHI', raw, 20) == (65534, channels, RATE, RATE*channels*2, channels*2, 16, 22, 16, 4 if channels == 1 else 3)
        assert uuid.UUID(bytes_le=raw[44:60]) == uuid.UUID('00000001-0000-0010-8000-00aa00389b71')
        assert raw[60:64] == b'data' and struct.unpack_from('<I', raw, 64)[0] == len(raw)-68
        actual = np.frombuffer(raw[68:], '<i2').reshape(-1, channels); assert actual.shape == wanted.shape
        difference = int(np.max(abs(actual.astype(np.int64)-wanted.astype(np.int64))))
        assert difference <= tolerance, (name, difference)
        assert receipt['samples'] == len(wanted) and receipt['clipped_samples'] == clipped
        assert receipt['pcm_sha256'] == hashlib.sha256(raw[68:]).hexdigest()
        assert receipt['peak_absolute'] == np.max(abs(actual.astype(np.int64)), axis=0).tolist()
        if dc is not None: assert np.max(abs(np.array(receipt['dc_removed_pcm'])-dc)) < 1e-8
        for observed, profile in zip(receipt['noise_profiles'], profiles):
            assert np.allclose(observed['noise_power'], profile, rtol=2e-11, atol=1e-5)
        if inspect:
            info = call({'command': 'audio.repair.inspect', 'recipe': rec, 'input_root': str(sources)})
            assert {k: v for k, v in receipt.items() if k not in ('output', 'sha256', 'recipe_sha256')} == info
        decoded = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-f', 's16le', '-'], capture_output=True, check=True).stdout
        assert decoded == raw[68:]
        (root/(name+'-recipe.json')).write_text(json.dumps(rec, indent=2)+'\n', encoding='utf-8')
        cases.append({'name': name, 'channels': channels, 'sample_frames': len(actual), 'maximum_pcm_error': difference})
        return receipt, actual.copy()

    rng = np.random.default_rng(740506)
    t = np.arange(RATE*2)/RATE
    base = rounded(3000*np.sin(2*np.pi*257*t)+900*np.sin(2*np.pi*3551*t))
    noise = rounded(rng.normal(0, 550, len(t)))
    clean = base.copy(); clean[:RATE//2] = 0
    write('noisy.wav', clean+noise)
    write('stereo.wav', np.column_stack((clean+noise, -clean+2*noise)))
    write('zero-profile.wav', clean)
    write('impulses.wav', [[100, -100], [32767, -32768], [-1, 1], [0, 0]])
    profile = {'regions': [{'start': time(0), 'duration': time(1, 2)}]}
    independent_channels = []
    for name in ('noisy.wav', 'stereo.wav'):
        independent_channels.append(check(recipe(name, noise=profile), 'profile-'+name[:-4])[1])
    assert np.array_equal(independent_channels[0][:, 0], independent_channels[1][:, 0])
    trimmed = recipe('stereo.wav', start=24617, length=17039, noise=profile)
    check(trimmed, 'trimmed-original-profile-clock')
    for start, length in ((0, 1), (3, 1), (1, 2), (0, 4)):
        check(recipe('impulses.wav', start, length), f'bypass-{start}-{length}', tolerance=0)
    for setting in ({'strength_milli': 0}, {'floor_milli': 1000}):
        check(recipe('stereo.wav', noise={**profile, **setting}), 'bypass-'+next(iter(setting)), tolerance=0)
    check(recipe('zero-profile.wav', noise=profile), 'zero-profile-identity', tolerance=0)
    multi = {'regions': [{'start': time(6144, RATE), 'duration': time(8192, RATE)}, {'start': time(0), 'duration': time(4096, RATE)}]}
    a = check(recipe('stereo.wav', noise=multi), 'two-noise-regions')[1]
    assert np.array_equal(a, check(recipe('stereo.wav', noise={**multi, 'regions': list(reversed(multi['regions']))}), 'profile-order-invariant')[1])
    passed.append('audio_repair.spectral_reference_and_sample_clocks')

    dc = np.column_stack((base+1300, -base-700)); write('biased.wav', dc)
    repaired = check(recipe('biased.wav', remove_dc=True), 'mean-dc-removal', tolerance=0)[1]
    assert np.max(abs(np.mean(repaired, axis=0))) <= .001
    highpass = {'type': 'high_pass', 'frequency_hz': 100, 'q': .707}
    cleanup = recipe('stereo.wav', noise=profile, remove_dc=True, effects=[highpass])
    cleaned = check(cleanup, 'noise-dc-highpass-order', tolerance=2)[1]
    before_recipe = copy.deepcopy(cleanup); before_recipe.pop('effects')
    filtered_source = np.array(effects_reference(fixtures['stereo.wav'].tolist(), [highpass]), dtype=np.int64)
    reversed_order = reference(before_recipe, filtered_source)[0]
    assert np.max(abs(cleaned.astype(np.int64)-reversed_order.astype(np.int64))) > 20
    rumble = rounded(3000*np.sin(2*np.pi*1000*t)+1600*np.sin(2*np.pi*20*t)+700)
    write('rumble.wav', rumble)
    rumble_fixed = check(recipe('rumble.wav', remove_dc=True, effects=[highpass]), 'rumble-cleanup', tolerance=1)[1][RATE:, 0]
    def amplitude(values, hz):
        phase = np.exp(-2j*np.pi*hz*np.arange(len(values))/RATE)
        return float(2*abs(np.dot(values, phase))/len(values))
    rumble_reduction = 20*math.log10(1600/amplitude(rumble_fixed, 20))
    speech_tone_change = 20*math.log10(amplitude(rumble_fixed, 1000)/3000)
    assert rumble_reduction >= 24 and abs(speech_tone_change) < .1, (rumble_reduction, speech_tone_change)
    loud = np.full((4800, 2), 30000, dtype=np.int16); write('loud.wav', loud)
    check(recipe('loud.wav', effects=[{'type': 'compressor', 'threshold_db': 0, 'ratio': 1, 'attack_ms': 0, 'release_ms': 0, 'makeup_db': 12}]), 'final-clipping', tolerance=0)
    passed.append('audio_repair.dc_cleanup_processing_and_clipping')

    # Quality gates are fixed before these held-out speech/noise cases execute.
    # These are controlled fidelity measurements, not a speech-recognition score.
    quality_limits = {'minimum_snr_improvement_db': 2, 'minimum_white_improvement_db': 4,
                      'minimum_noise_only_reduction_db': 6, 'minimum_aligned_voice_gain': .8,
                      'maximum_aligned_voice_gain': 1.05, 'minimum_high_band_error_improvement_db': 0,
                      'minimum_high_band_voice_gain': .65, 'maximum_high_band_voice_gain': 1.15,
                      'maximum_added_white_tonal_concentration_db': 6,
                      'maximum_impulse_shift_samples': 0, 'minimum_impulse_gain': .8,
                      'maximum_impulse_gain': 1.05, 'maximum_preecho_relative_peak': .02}
    (root/'quality-limits.json').write_text(json.dumps(quality_limits, indent=2)+'\n', encoding='utf-8')
    speeches, speech_metadata = speech_fixtures(sources)
    for item in speech_metadata['fixtures']: original_sources[sources/(item['id']+'.wav')] = item['sha256']
    quality = []
    for voice, speech in speeches.items():
        speech = rounded(speech*12000/np.max(abs(speech)))
        known = np.concatenate((np.zeros(RATE), speech, np.zeros(RATE)))
        write(voice+'-clean.wav', known)
        speech_profile = {'regions': [{'start': time(0), 'duration': time(1)}]}
        check(recipe(voice+'-clean.wav', noise=speech_profile), voice+'-clean-identity', tolerance=0, inspect=False)
        times = np.arange(len(known))/RATE
        white = rng.normal(size=len(known))
        frequency = np.fft.rfftfreq(len(known), 1/RATE)
        colored = np.fft.irfft(np.fft.rfft(white)/np.sqrt(np.maximum(frequency, 80)), n=len(known))
        signals = [('white', white, 350), ('white-high', white, 1000), ('colored', colored, 700), ('colored-low', colored, 350),
                   ('hum', np.sin(2*np.pi*50*times)+.4*np.sin(2*np.pi*100*times), 1000),
                   ('mixed', white+2*np.sin(2*np.pi*60*times), 700)]
        for kind, raw_noise, level in signals:
            added = raw_noise*level/np.sqrt(np.mean(raw_noise**2))
            noisy = rounded(known+added); assert np.max(abs(noisy)) < 32767
            name = voice+'-'+kind; write(name+'.wav', noisy)
            fixed = check(recipe(name+'.wav', noise=speech_profile), name, inspect=False)[1][:, 0].astype(float)
            selected = slice(RATE, -RATE); expected = known[selected]; original_error = (noisy-known)[selected]; error = (fixed-known)[selected]
            improvement = float(10*np.log10(np.mean(original_error**2)/np.mean(error**2)))
            reduction = float(10*np.log10(np.mean(noisy[:RATE]**2)/np.mean(fixed[:RATE]**2)))
            gain = float(np.dot(expected, fixed[selected])/np.dot(expected, expected))
            high_before = band(original_error, 3000, 10000); high_after = band(error, 3000, 10000)
            high_improvement = float(10*np.log10(np.mean(high_before**2)/np.mean(high_after**2)))
            high_voice = band(expected, 3000, 10000); high_fixed = band(fixed[selected], 3000, 10000)
            high_voice_gain = float(np.dot(high_voice, high_fixed)/np.dot(high_voice, high_voice))
            tonal_increase = concentration(fixed[:RATE])-concentration(noisy[:RATE]) if kind.startswith('white') else None
            result = {'voice': voice, 'noise': kind, 'noise_rms_pcm': level, 'snr_improvement_db': improvement,
                      'noise_only_reduction_db': reduction, 'aligned_voice_gain': gain,
                      'high_band_error_improvement_db': high_improvement, 'high_band_voice_gain': high_voice_gain,
                      'added_white_tonal_concentration_db': tonal_increase}
            quality.append(result)
            (root/'quality-observations.json').write_text(json.dumps(quality, indent=2)+'\n', encoding='utf-8')
            assert improvement >= (4 if kind.startswith('white') else 2), result
            assert reduction >= 6 and .8 <= gain <= 1.05, result
            assert high_improvement >= 0, result
            assert .65 <= high_voice_gain <= 1.15, result
            if tonal_increase is not None: assert tonal_increase <= 6, result
    passed.append('audio_repair.controlled_speech_and_noise_quality')

    # Isolate the nonlinear change caused by a known sharp foreground onset.
    background = rounded(rng.normal(0, 200, RATE*2)); impulse_at = 67234
    impulse = background.copy(); impulse[impulse_at] += 25000
    write('transient-background.wav', background); write('transient.wav', impulse)
    background_fixed = check(recipe('transient-background.wav', noise=profile), 'transient-background', inspect=False)[1][:, 0].astype(float)
    impulse_fixed = check(recipe('transient.wav', noise=profile), 'transient', inspect=False)[1][:, 0].astype(float)
    isolated = impulse_fixed-background_fixed; location = int(np.argmax(abs(isolated)))
    transient = {'shift_samples': location-impulse_at, 'peak_gain': isolated[impulse_at]/25000,
                 'preecho_relative_peak': float(np.max(abs(isolated[:impulse_at]))/25000)}
    (root/'transient-observations.json').write_text(json.dumps(transient, indent=2)+'\n', encoding='utf-8')
    assert location == impulse_at and .8 <= transient['peak_gain'] <= 1.05, transient
    assert transient['preecho_relative_peak'] <= .02, transient
    passed.append('audio_repair.clean_bypass_and_transient_artifact_limits')

    # Repaired sources feed the existing routed mixer, scene and saved history.
    scene_recipe = recipe('stereo.wav', start=24000, length=9600, noise=profile)
    scene_sound = check(scene_recipe, 'scene-repaired')[1]
    picture = output/'picture.png'; Image.new('RGB', (4, 4), (34, 80, 210)).save(picture)
    original_sources[picture] = hashlib.sha256(picture.read_bytes()).hexdigest()
    soundtrack = {'schema_version': 1, 'id': 'repaired-soundtrack', 'duration': time(1, 5),
                  'tracks': [{'id': 'voice', 'clips': [{'id': 'repaired', 'file': identity(output/'scene-repaired.wav', output), 'channels': 'preserve_layout', 'start': time(0), 'source_in': time(0), 'duration': time(1, 5)}]}],
                  'routing': {'output': 'stereo', 'tracks': [{'id': 'voice', 'layout': 'stereo'}], 'buses': [], 'routes': [{'id': 'direct', 'source': {'kind': 'track', 'id': 'voice'}, 'destination': {'kind': 'output'}, 'mapping': {'type': 'identity'}}]}}
    scene = {'schema_version': 1, 'id': 'repaired-scene', 'width': 4, 'height': 4, 'output_scale': 1, 'duration': time(1, 5), 'background': [0, 0, 0], 'color': 'srgb_straight_encoded', 'audio': None, 'audio_mix': soundtrack,
             'layers': [{'id': 'image', 'canvas': [4, 4], 'start': time(0), 'duration': time(1, 5), 'frames': [{'image': identity(picture, output), 'hold': time(1, 5), 'offset': [0, 0], 'anchor': [0, 0]}], 'timing': 'strict', 'end': 'hold_last', 'transform': {'position': [0, 0], 'crop': [0, 0, 4, 4], 'scale': 1, 'quarter_turns': 0, 'opacity': 255}}]}
    def decode_media(path, audio=False):
        return subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:a:0' if audio else '0:v:0', *(['-f', 's16le'] if audio else ['-pix_fmt', 'rgb24', '-f', 'rawvideo']), '-'], capture_output=True, check=True).stdout
    dest = output/'scene.mkv'; compiled = call({'command': 'scene.render', 'scene': scene, 'input_root': str(output), 'output_root': str(output), 'output': str(dest)})
    assert decode_media(dest, True) == scene_sound.tobytes(); assert decode_media(dest) == bytes((34, 80, 210))*4*4*5
    client = Client(exe)
    try:
        client.initialize(); catalog = client.rpc('tools/list')['result']['tools']; assert len(catalog) == 65
        tool = next(t for t in catalog if t['name'] == 'cutbolt_audio_repair_inspect')
        assert not any(t['name'] == 'cutbolt_audio_repair_render' for t in catalog)
        args = {'recipe': scene_recipe, 'input_root': str(sources)}
        Draft202012Validator(tool['inputSchema']).validate(args); assert tool['annotations']['readOnlyHint']
        assert client.call('audio.repair.inspect', **args)['pcm_sha256'] == hashlib.sha256(scene_sound.tobytes()).hexdigest()
        instantiated = client.call('graphics.instantiate', template={'schema_version': 1, 'id': 'repair-template', 'scene': scene, 'parameters': []}, values={}, instance_id='repair-copy', input_root=str(output))
        restored_mix = instantiated['scene']['audio_mix']
        assert restored_mix['routing'] == soundtrack['routing']
        assert restored_mix['tracks'][0]['clips'][0]['file'] == soundtrack['tracks'][0]['clips'][0]['file']
        assert client.call('audio.inspect', mix=restored_mix, input_root=str(output))['pcm_sha256'] == hashlib.sha256(scene_sound.tobytes()).hexdigest()
        project = call({'command': 'project.create', 'id': 'repaired-edit', 'width': 4, 'height': 4, 'frame_rate': time(25)})
        client.call('session.create', store_root=str(store), project=project, request_id='create'); common = {'store_root': str(store), 'project_id': project['id']}
        asset = compiled['asset']; asset['identity'] = {'sha256': compiled['sha256'], 'bytes': dest.stat().st_size}
        args = {**common, 'expected_revision': 0, 'request_id': 'add', 'operations': [{'op': 'media.add', 'asset': asset}, {'op': 'clip.append', 'clip': {'id': 'sound', 'asset_id': asset['id'], 'source_in': time(1, 25), 'duration': time(3, 25)}}]}
        receipt = client.call('session.apply', **args); assert client.call('session.apply', **args) == receipt
        saved = client.call('session.get', **common); dest = output/'saved.mkv'
        call({'command': 'render.run', 'project': saved, 'input_root': str(output), 'output_root': str(output), 'output': str(dest)})
        assert decode_media(dest, True) == scene_sound[1920:7680].tobytes(); assert decode_media(dest) == bytes((34, 80, 210))*4*4*3
        for n in (0, 2):
            dest = output/f'preview-{n}.png'; client.call('preview.frame', project=saved, input_root=str(output), output_root=str(output), output=str(dest), time=time(n, 25))
            with Image.open(dest) as im: assert im.tobytes() == bytes((34, 80, 210))*4*4
        dest = output/'range.mkv'; call({'command': 'preview.range', 'project': saved, 'input_root': str(output), 'output_root': str(output), 'output': str(dest), 'start': time(1, 25), 'duration': time(1, 25)})
        assert decode_media(dest, True) == scene_sound[3840:5760].tobytes(); assert decode_media(dest) == bytes((34, 80, 210))*4*4
        client.call('session.apply', **common, expected_revision=1, request_id='trim', operations=[{'op': 'clip.trim', 'clip_id': 'sound', 'source_in': time(0), 'duration': time(2, 25)}])
        client.call('session.undo', **common, expected_revision=2, request_id='undo'); assert client.call('session.get', **common)['clips'] == saved['clips']
    finally: client.close()
    passed.append('audio_repair.mcp_scenes_saved_edits_and_previews')

    # Maximum output and profile windows execute the actual spectral path, even
    # though the selected output is silent. The profile lies later in the source.
    boundary = np.zeros((RATE*61, 2), dtype=np.int16)
    boundary[RATE*60:] = rounded(rng.normal(0, 300, (RATE, 2)))
    write('boundary.wav', boundary)
    longest = recipe('boundary.wav', length=RATE*60, noise={'regions': [{'start': time(51), 'duration': time(10)}]})
    client = Client(exe)
    try:
        client.initialize(); start = clock.monotonic()
        info = client.call('audio.repair.inspect', recipe=longest, input_root=str(sources))
        elapsed = clock.monotonic()-start; memory = peak_memory(client.process.pid)
    finally: client.close()
    assert memory < 256*1024*1024, (elapsed, memory)
    budgets.check(elapsed < 40, (elapsed, memory))
    assert all(not c['bypassed'] and c['profile_windows'] == 465 and c['windows'] == 2816 for c in info['noise_profiles'])
    check(longest, 'maximum-duration-and-profile', expected=boundary[:RATE*60], tolerance=0, inspect=False)
    many = {'regions': [{'start': time(i*N, RATE), 'duration': time(N, RATE)} for i in range(8)], 'floor_milli': 0, 'strength_milli': 3000}
    check(recipe('stereo.wav', length=1, noise=many), 'maximum-regions-strength-and-floor')
    performance = {'inspection_seconds': elapsed, 'inspection_peak_working_set_bytes': memory, 'samples': RATE*60, 'profile_windows_per_channel': 465, 'output_windows_per_channel': 2816}
    passed.append('audio_repair.maximum_profiles_duration_and_memory')

    rejected_paths = []
    def invalid(edit, code='INVALID_AUDIO_REPAIR'):
        bad = copy.deepcopy(trimmed); edit(bad); name = f'invalid-{rejected}'; rejected_paths.append(output/(name+'.wav')); call(request(bad, name), code)
    for key, value in [('schema_version', 2), ('id', ''), ('duration', time(0)), ('duration', time(61))]: invalid(lambda r, k=key, v=value: r.update({k: v}))
    for key, value in [('strength_milli', 3001), ('floor_milli', 1001), ('regions', []), ('regions', profile['regions']*9), ('regions', profile['regions']*2), ('regions', [{'start': time(0), 'duration': time(4095, RATE)}]), ('regions', [{'start': time(0), 'duration': time(11)}])]: invalid(lambda r, k=key, v=value: r['noise'].update({k: v}))
    invalid(lambda r: r['noise'].update({'regions': [{'start': time(2), 'duration': time(1)}]}), 'INVALID_RANGE')
    invalid(lambda r: r.update({'source_in': time(2)}), 'INVALID_RANGE')
    invalid(lambda r: r.update({'duration': time(1, 7)}), 'UNALIGNED_TIME')
    invalid(lambda r: r['noise']['regions'][0].update({'start': time(1, 7)}), 'UNALIGNED_TIME')
    invalid(lambda r: r['source'].update({'sha256': '0'*64}), 'MEDIA_CHANGED')
    invalid(lambda r: r['source'].update({'bytes': 64*1024*1024+1}))
    invalid(lambda r: r['source'].update({'path': '../outside.wav'}), 'INVALID_PATH')
    invalid(lambda r: r.update({'effects': [{'type': 'high_pass', 'frequency_hz': 0, 'q': .707}]}), 'INVALID_AUDIO_EFFECT')
    invalid(lambda r: r.update({'noise': {**r['noise'], 'strength_milli': 0, 'regions': []}}))
    invalid(lambda r: r['noise']['regions'][0].update({'start': time(2**63)}), 'INVALID_TIME')
    invalid(lambda r: r['noise']['regions'][0].update({'start': time(2**53-1)}), 'TIME_OVERFLOW')
    write('wrong-rate.wav', np.zeros((4800, 1), dtype=np.int16), rate=24000)
    write('ambiguous-layout.wav', np.zeros((4800, 6), dtype=np.int16))
    for name in ('wrong-rate.wav', 'ambiguous-layout.wav'):
        call(request(recipe(name, length=480), name), 'UNSUPPORTED_AUDIO')
    call({**request(trimmed, 'wrong-root'), 'input_root': 'relative'}, 'INVALID_PATH')
    call({**request(trimmed, 'wrong-extension'), 'output': str(output/'wrong-extension.bin')}, 'UNSUPPORTED_OUTPUT')
    call(request(trimmed, 'trimmed-original-profile-clock'), 'OUTPUT_EXISTS')
    assert all(not p.exists() for p in rejected_paths)
    assert all(hashlib.sha256(p.read_bytes()).hexdigest() == h for p, h in original_sources.items())
    assert all(hashlib.sha256(p.read_bytes()).hexdigest() == h for p, h in published_outputs.items())
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('audio_repair.validation_and_source_preservation')
    report = {'passed': passed, 'cases': cases, 'sample_frames_compared': sum(c['sample_frames'] for c in cases), 'individual_samples_compared': sum(c['sample_frames']*c['channels'] for c in cases), 'rejected_cases': rejected, 'numpy_version': np.__version__, 'speech_fixture_generator': speech_metadata, 'quality_limits': quality_limits, 'quality': quality, 'transient': transient, 'rumble_reduction_db': rumble_reduction, 'speech_tone_change_db': speech_tone_change, 'scene_session_frames': 9, 'frame_previews': 2, 'performance': performance}
    (root/'verification.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--output', type=Path, required=True)
    print(json.dumps(run(parser.parse_args().output)))
