"""Timeline dynamics: track and master limiters, checked sample by sample against an independent
integer oracle of the documented design over whole renders, ranges, previews and chunked renders,
with the 4x true-peak meter and audio.normalize's limiter proposal."""
from engine import ENGINE
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import wave
import numpy as np
from jsonschema import Draft202012Validator
from tracks import time, seconds, edit, placement, track

ROOT = Path(__file__).resolve().parents[1]
W, H = 32, 24
ONE = 1 << 32
SCALE = 1 << 16


def bessel(x):
    total = term = 1.0
    k = 1
    while True:
        term *= (x / (2 * k)) ** 2
        total += term
        if term < 1e-17 * total:
            return total
        k += 1


def design():
    """Interpolation taps from the documented design: a Kaiser-windowed sinc (beta 5, half-width
    8) at 1/4, 1/2 and 3/4 of a sample, normalized to sum to 2^16 and rounded, the residue on the
    tap(s) nearest the point; the outer phases mirror each other."""
    def raw(tau):
        values = []
        for t in range(16):
            d = t - 7 - tau
            values.append(math.sin(math.pi * d) / (math.pi * d) * bessel(5 * math.sqrt(1 - (d / 8) ** 2)) / bessel(5))
        total = sum(values)
        return [round(SCALE * v / total) for v in values]
    quarter = raw(0.25)
    quarter[7] += SCALE - sum(quarter)
    half = raw(0.5)
    residue = SCALE - sum(half)
    half[7] += residue // 2
    half[8] += residue // 2
    return np.array([quarter, half, quarter[::-1]], dtype=np.int64)


TAPS = design()


def codes(db):
    value = 32768 * 10 ** (db / 20)
    assert abs(value - round(value)) > 1e-6, db
    return min(32767, math.floor(value))


def interval_peaks(x):
    """Per sample j in [-24, T+24): the largest magnitude over both channels at sample j and the
    three interpolated points toward j+1, in 1/2^16 codes, with silence outside the signal."""
    count = len(x)
    pad = np.zeros((count + 64, 2), dtype=np.int64)
    pad[32:32 + count] = x
    length = count + 48
    best = np.abs(pad[8:8 + length]).max(axis=1) * SCALE
    for row in TAPS:
        acc = np.zeros((length, 2), dtype=np.int64)
        for t in range(16):
            acc += row[t] * pad[1 + t:1 + t + length]
        best = np.maximum(best, np.abs(acc).max(axis=1))
    return best


def limit(x, limiter):
    """The limiter's output for the whole signal x (int64, T x 2) with silence before and after,
    and its gain per sample."""
    count = len(x)
    assert np.abs(x).max(initial=0) < 1 << 30
    lookahead = limiter.get('lookahead_ms', 5) * 48
    release = limiter.get('release_ms', 150) * 48
    ceiling = codes(limiter['ceiling_dbfs']) * SCALE
    step = -(-ONE // release)
    peaks = interval_peaks(x)  # j from -24
    # Required gain of sample i from the largest interval peak within 8 samples, i in [-16, T+16).
    held = np.lib.stride_tricks.sliding_window_view(peaks, 17).max(axis=1)
    required = np.where(held <= ceiling, ONE, (ceiling * ONE) // np.maximum(held, 1))
    # Positions from -16 - lookahead; unity elsewhere.
    start = -16 - lookahead
    extended = np.full(count + 32 + 2 * lookahead, ONE, dtype=np.int64)
    extended[lookahead:lookahead + count + 32] = required  # position -16 at index lookahead
    # m[i] = min(required[i .. i + lookahead]) for positions start .. T+16
    minimum = np.lib.stride_tricks.sliding_window_view(extended, lookahead + 1).min(axis=1)
    released = []
    level = ONE
    for value in minimum[:count - start].tolist():
        level = min(value, min(ONE, level + step))
        released.append(level)
    # Gain of sample n: the floor of the mean of the released gains of n - lookahead .. n.
    padded = [ONE] * (lookahead + 1) + released
    sums = np.cumsum(np.array(padded, dtype=np.int64))
    offset = -start + lookahead + 1  # index in padded of position 0
    gain = (sums[offset:offset + count] - sums[offset - lookahead - 1:offset + count - lookahead - 1]) // (lookahead + 1)
    product = x * gain[:, None]
    out = np.sign(product) * ((np.abs(product) + (1 << 31)) >> 32)
    return out, gain


def wav(path, frames):
    with wave.open(str(path), 'wb') as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(48000)
        w.writeframes(np.asarray(frames, dtype='<i2').tobytes())


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, out, store = [root / n for n in ('sources', 'output', 'store')]
    for path in (sources, out, store):
        path.mkdir()
    passed, cases = [], []
    counts = {'rejected': 0, 'samples': 0}

    def call(request, error=None):
        p = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, timeout=600)
        value = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and value['error']['code'] == error, (error, value)
            counts['rejected'] += 1
            return value
        if p.returncode or not value['ok']:
            (root / 'failed-request.json').write_text(json.dumps(request, indent=2), encoding='utf-8')
        assert p.returncode == 0 and value['ok'], value
        return value['result']

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', *args], check=True, capture_output=True, timeout=120).stdout

    # Original synthetic sources, 3 s of 48 kHz stereo PCM16 each (whole 25 fps frames).
    T = 3 * 48000
    n = np.arange(T)
    rng = np.random.default_rng(20261005)
    # Speech-like: 125 ms syllables of a 180 Hz harmonic voice every 250 ms, each opened by a
    # consonant click, for a peak-to-loudness ratio near 18 dB like synthesized narration.
    phase = n % 12000
    envelope = np.where(phase < 6000, np.minimum(phase / 96, 1), np.where(phase < 9600, np.exp(-(phase - 6000) / 1200), 0))
    harmonic = sum(np.sin(2 * np.pi * 180 * h * n / 48000 + h) / h for h in range(1, 9))
    voice = 5000 * envelope * harmonic
    voice[phase < 3] += 22000
    voice = np.stack([voice, 0.8 * voice], axis=1)
    # Music bed: a sustained chord with slow tremolo.
    chord = sum(np.sin(2 * np.pi * f * n / 48000 + i) for i, f in enumerate((110, 138.6, 164.8, 220)))
    music = 2600 * chord * (0.8 + 0.2 * np.sin(2 * np.pi * 0.7 * n / 48000))
    music = np.stack([music, np.roll(music, 37)], axis=1)
    # Intersample-hot: 12 kHz at 45 degrees (samples read 0.707 of the peak) under faded edges.
    fade = np.minimum(np.minimum(n, T - 1 - n) / 960, 1)
    hot = 0.94 * 32767 * fade * np.sin(np.pi / 4 + n * np.pi / 2)
    hot = np.stack([hot, -hot], axis=1)
    # Clicks: short bursts of noise for many-clip (chunked) timelines.
    clicks = rng.integers(-20000, 20000, size=(T, 2)) * (np.arange(T) % 1920 < 240)[:, None]
    signals = {}
    assets = []
    for index, (name, values) in enumerate([('voice', voice), ('music', music), ('hot', hot), ('clicks', clicks)]):
        pcm = np.clip(np.round(values), -32768, 32767).astype(np.int64)
        path = sources / f'{name}.wav'
        wav(path, pcm)
        signals[str(index)] = pcm
        assets.append({'id': str(index), 'path': str(path), 'duration': time(3), 'identity': {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(), 'bytes': path.stat().st_size}})
    original = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def preserved():
        assert original == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def apply(project, ops, error=None):
        return call({'command': 'timeline.apply', 'project': project, 'expected_revision': project['revision'], 'operations': ops}, error)

    def curve_gains(curve, first, count):
        """Integer gains of a hold/linear gain curve at source samples first .. first+count."""
        keys = sorted((seconds(k['time']) * 48000, k['value'], k['interpolation']) for k in curve['keys'])
        assert all(k[0].denominator == 1 and k[2] in ('hold', 'linear') for k in keys)
        s = np.arange(first, first + count, dtype=np.int64)
        gains = np.full(count, keys[0][1], dtype=np.int64)
        for (ta, va, mode), (tb, vb, _) in zip(keys, keys[1:]):
            inside = (s >= int(ta)) & (s < int(tb))
            if mode == 'hold':
                gains[inside] = va
            else:
                d = int(tb - ta)
                w = va * (d - (s[inside] - int(ta))) + vb * (s[inside] - int(ta))
                gains[inside] = np.sign(w) * ((np.abs(w) + d // 2) // d)
        gains[s >= int(keys[-1][0])] = keys[-1][1]
        return gains

    def rounded(v, d):
        return np.sign(v) * ((np.abs(v) + d // 2) // d)

    def clip_values(c, i0, i1):
        """A clip's voice at timeline samples i0 .. i1 (handles may pass its placement): gain or
        curve and linear fades, one rounding half away from zero."""
        start = int(seconds(c['start']) * 48000)
        source_in = int(seconds(c['source_in']) * 48000)
        length = int(seconds(c['duration']) * 48000)
        local = np.arange(i0, i1, dtype=np.int64) - start
        s = signals[c['asset_id']][source_in + local]
        g = curve_gains(c['gain_curve'], source_in + i0 - start, i1 - i0) if c.get('gain_curve') else np.full(i1 - i0, c.get('gain_milli', 1000), dtype=np.int64)
        fi = int(seconds(c.get('fade_in', time(0))) * 48000)
        fo = int(seconds(c.get('fade_out', time(0))) * 48000)
        w, d = g.copy(), np.full(i1 - i0, 1000, dtype=np.int64)
        if fo:
            tail = local >= length - fo
            w[tail], d[tail] = g[tail] * (length - local[tail]), 1000 * fo
        if fi:
            head = local < fi
            w[head], d[head] = g[head] * local[head], 1000 * fi
        return rounded(s * w[:, None], d[:, None])

    cached = {}

    def track_sum(layer, count):
        """One audio track's clips with gain, curves, fades and transitions, summed unsaturated."""
        key = json.dumps(layer, sort_keys=True) + str(count)
        if key in cached:
            return cached[key].copy()
        total = np.zeros((count, 2), dtype=np.int64)
        clips = {c['id']: c for c in layer['clips']}
        covered = np.zeros(count, dtype=bool)
        for e in layer.get('transitions', []):
            left, right = clips[e['left_id']], clips[e['right_id']]
            cut = int(seconds(right['start']) * 48000)
            begin, finish = cut - int(seconds(e['before']) * 48000), cut + int(seconds(e['after']) * 48000)
            k = 2 * np.arange(finish - begin, dtype=np.int64)[:, None] + 1
            d = 2 * (finish - begin)
            a, b = (np.maximum(d - 2 * k, 0), np.maximum(2 * k - d, 0)) if e['kind'] == 'dip_black' else (d - k, k)
            total[begin:finish] += rounded(clip_values(left, begin, finish) * a + clip_values(right, begin, finish) * b, d)
            covered[begin:finish] = True
        for c in layer['clips']:
            start = int(seconds(c['start']) * 48000)
            end = start + int(seconds(c['duration']) * 48000)
            values = clip_values(c, start, end)
            keep = ~covered[start:end]
            total[start:end][keep] += values[keep]
        cached[key] = total
        return total.copy()

    def oracle(project):
        """The timeline's final PCM16 and each limiter's gain."""
        count = int(seconds(project['tracks']['duration']) * 48000)
        total = np.zeros((count, 2), dtype=np.int64)
        gains = {}
        for layer in project['tracks']['tracks']:
            if layer['kind'] != 'audio' or not layer['enabled']:
                continue
            part = track_sum(layer, count)
            if layer.get('dynamics'):
                part, gains[layer['id']] = limit(part, layer['dynamics']['limiter'])
            total += part
        master = project['tracks'].get('master')
        if master:
            total, gains['master'] = limit(total, master['limiter'])
        return np.clip(total, -32768, 32767).astype('<i2'), gains

    def pcm_of(path):
        return np.frombuffer(ff(['-i', str(path), '-vn', '-f', 's16le', '-']), dtype='<i2').reshape(-1, 2)

    def compare(actual, expected, label):
        assert actual.shape == expected.shape, (label, actual.shape, expected.shape)
        if not np.array_equal(actual, expected):
            at = int(np.argwhere(actual != expected)[0][0])
            raise AssertionError((label, at, actual[at].tolist(), expected[at].tolist()))
        counts['samples'] += len(actual)
        cases.append(label)
        preserved()

    def render(project, label):
        output = out / f'{label}.mkv'
        receipt = call({'command': 'render.run', 'project': project, 'input_root': str(root), 'output_root': str(out), 'output': str(output)})
        expected, gains = oracle(project)
        compare(pcm_of(output), expected, label)
        return receipt, expected, gains

    def reduction(gain):
        reduced = gain < ONE
        low = int(gain.min()) if len(gain) else ONE
        return round(-20 * math.log10(max(low, 1) / ONE), 2), int(reduced.sum())

    base = call({'command': 'project.create', 'id': 'dynamics', 'width': W, 'height': H, 'frame_rate': time(25)})
    ops = [{'op': 'media.add', 'asset': a} for a in assets] + [edit('create', duration=time(75, 25))]
    for name in ('v', 'voice', 'music'):
        ops.append(edit('add', track=track(name, 'video' if name == 'v' else 'audio')))
    # The voice is two clips joined by a dissolve, raised 8 dB so its onsets pass full scale; the
    # music ducks on a gain curve.
    ops += [edit('place', track_id='voice', clip=placement('v1', 0, 0, 72000, 0, 48000), collision='reject'),
            edit('place', track_id='voice', clip=placement('v2', 0, 72000, 72000, 72000, 48000), collision='reject'),
            edit('transition_set', track_id='voice', transition={'id': 'vx', 'left_id': 'v1', 'right_id': 'v2', 'before': time(4800, 48000), 'after': time(4800, 48000), 'kind': 'dissolve'}),
            edit('clip_audio', clip_ids=['v1', 'v2'], gain_milli=2500),
            edit('place', track_id='music', clip=placement('bed', 1, 0, 75, 0), collision='reject'),
            edit('clip_audio', clip_ids=['bed'], fade_in=time(1, 5), gain_curve={'keys': [
                {'time': time(0), 'value': 900, 'interpolation': 'linear'}, {'time': time(1), 'value': 400, 'interpolation': 'hold'},
                {'time': time(2), 'value': 1200, 'interpolation': 'linear'}, {'time': time(5, 2), 'value': 700, 'interpolation': 'linear'}]})]
    base = apply(base, ops)
    plain_receipt, plain, _ = render(base, 'unlimited')
    assert 'dynamics' not in plain_receipt
    assert np.abs(plain.astype(np.int64)).max() == 32768 or plain.max() == 32767, 'the unlimited mix clips'

    # 1. Master limiter: exact against the oracle; no sample over the ceiling, no clipping.
    master = {'ceiling_dbfs': -1.0, 'lookahead_ms': 5, 'release_ms': 150}
    limited = apply(base, [edit('audio_dynamics', dynamics={'limiter': {'ceiling_dbfs': -1.0}})])
    assert limited['tracks']['master'] == {'limiter': master}, limited['tracks'].get('master')
    receipt, expected, gains = render(limited, 'master-limiter')
    assert np.abs(expected.astype(np.int64)).max() <= codes(-1.0)
    max_db, reduced = reduction(gains['master'])
    assert receipt['dynamics']['master']['max_reduction_db'] == max_db and receipt['dynamics']['tracks'] == [], receipt['dynamics']
    assert receipt['dynamics']['master']['reduced_seconds'] == round(reduced / 48000, 3) and max_db > 6, (receipt['dynamics'], max_db)
    # An H.264 export streams the timeline into its encoder: what AAC received is the limited mix.
    delivery = call({'command': 'export.run', 'project': limited, 'input_root': str(root), 'output_root': str(out), 'output': str(out / 'master-limiter.mp4'),
                     'profile': 'h264_aac', 'streams': 'audio_video', 'input_transfer': 'bt709'})
    assert delivery['verification']['encoder_input'] == 'streamed'
    assert delivery['verification']['timeline_audio_sha256'] == hashlib.sha256(expected.tobytes()).hexdigest()
    preserved()
    passed.append('dynamics.master_limiter_exact')

    # 2. A track limiter before the master: the voice is limited alone, then the sum again.
    both = apply(limited, [edit('audio_dynamics', track_id='voice', dynamics={'limiter': {'ceiling_dbfs': -7.5, 'lookahead_ms': 2, 'release_ms': 40}}),
                           edit('audio_dynamics', dynamics={'limiter': {'ceiling_dbfs': -2.5, 'lookahead_ms': 3, 'release_ms': 600}})])
    receipt, expected, gains = render(both, 'track-and-master-limiters')
    assert [t['track_id'] for t in receipt['dynamics']['tracks']] == ['voice'] and receipt['dynamics']['tracks'][0]['max_reduction_db'] == reduction(gains['voice'])[0]
    track_only = apply(both, [edit('audio_dynamics')])
    assert 'master' not in track_only['tracks']
    receipt, expected, _ = render(track_only, 'track-limiter-only')
    assert receipt['dynamics']['master'] is None
    passed.append('dynamics.track_limiters_exact')

    # 3. Windows: ranges, previews, audio-only exports and chunked renders hold the whole render's samples.
    whole, _ = oracle(both)
    for first, count in [(0, 10), (31, 7), (60, 15)]:
        output = out / f'range-{first}.mkv'
        call({'command': 'preview.range', 'project': both, 'input_root': str(root), 'output_root': str(out), 'output': str(output), 'start': time(first, 25), 'duration': time(count, 25)})
        compare(pcm_of(output), whole[first * 1920:(first + count) * 1920], f'range-{first}-{count}')
    meters = call({'command': 'timeline.meters', 'project': both, 'input_root': str(root), 'start': time(40, 25), 'duration': time(25, 25), 'tracks': False})['mix']
    assert meters['dynamics']['master']['max_reduction_db'] == reduction(oracle(both)[1]['master'][40 * 1920:65 * 1920])[0]
    clicks = apply(limited, [edit('add', track=track('clicks', 'audio'))] + [edit('place', track_id='clicks', clip=placement(f'k{i}', 3, i * 1920, 1920, i * 1920, 48000), collision='reject') for i in range(70)])
    plan = call({'command': 'render.plan', 'project': clicks, 'input_root': str(root), 'output_root': str(out), 'output': str(out / 'chunked-plan.mkv')})
    assert len(plan['chunks']) >= 2, plan.get('chunks')
    render(clicks, 'chunked-master-limiter')
    passed.append('dynamics.windows_and_chunks_exact')

    # 4. A limiter that never engages changes nothing.
    quiet = apply(base, [edit('clip_audio', clip_ids=['v1', 'v2'], gain_milli=300), edit('audio_dynamics', dynamics={'limiter': {'ceiling_dbfs': -0.5}}),
                         edit('audio_dynamics', track_id='music', dynamics={'limiter': {'ceiling_dbfs': -0.5}})])
    receipt, expected, gains = render(quiet, 'inactive-limiters')
    _, unlimited, _ = render(apply(quiet, [edit('audio_dynamics'), edit('audio_dynamics', track_id='music')]), 'inactive-limiters-removed')
    assert np.array_equal(expected, unlimited) and receipt['dynamics']['master']['max_reduction_db'] == 0 and receipt['dynamics']['master']['reduced_seconds'] == 0
    passed.append('dynamics.inactive_limiter_is_identity')

    # 5. True peaks: a 12 kHz tone whose samples stay at -3.5 dBFS but whose peaks between samples
    # reach -0.5 dBTP is held at a -1 dBFS ceiling. The meter's reading is recomputed here and
    # checked against FFmpeg's ebur128 true-peak meter.
    def tone(project):
        return apply(project, [edit('add', track=track('hot', 'audio')), edit('place', track_id='hot', clip=placement('tone', 2, 0, 75, 0), collision='reject'),
                               edit('state', track_id='voice', locked=False, enabled=False), edit('state', track_id='music', locked=False, enabled=False)])

    def true_peak(pcm):
        return 20 * math.log10(int(interval_peaks(pcm.astype(np.int64)).max()) / (SCALE * 32768))

    def ebur128(path):
        log = subprocess.run(['ffmpeg', '-hide_banner', '-nostats', '-i', str(path), '-vn', '-af', 'ebur128=peak=true', '-f', 'null', '-'], capture_output=True, text=True, check=True).stderr
        return float(re.search(r'True peak:\s+Peak:\s+(-?[\d.]+) dBFS', log).group(1))

    open_tone = tone(base)
    _, pcm, _ = render(open_tone, 'tone-unlimited')
    sample_peak = 20 * math.log10(np.abs(pcm.astype(np.int64)).max() / 32768)
    assert sample_peak < -3 and true_peak(pcm) > -1, (sample_peak, true_peak(pcm))
    held = tone(limited)
    _, pcm, _ = render(held, 'tone-limited')
    for project, label in ((open_tone, 'tone-unlimited'), (held, 'tone-limited')):
        expected, _ = oracle(project)
        meter = call({'command': 'timeline.meters', 'project': project, 'input_root': str(root), 'tracks': False})['mix']
        assert meter['true_peak_available'] is True and abs(max(meter['true_peak_dbtp']) - true_peak(expected)) < 1e-9, (meter['true_peak_dbtp'], true_peak(expected))
        assert abs(max(meter['true_peak_dbtp']) - ebur128(out / f'{label}.mkv')) <= 0.2, (label, meter['true_peak_dbtp'], ebur128(out / f'{label}.mkv'))
    assert true_peak(pcm) <= -1 + 0.05, true_peak(pcm)
    passed.append('dynamics.true_peak_detection_and_meter')

    # 6. Normalizing proposes the limiter the target needs and measures the result honestly. Its
    #    passes replay one render of the mix in memory; the proposed levels are rendered once more
    #    and measured exactly, so measured and result equal the meters of the mix before and after.
    def hundredths(value):
        return math.copysign(math.floor(abs(value) * 100 + 0.5), value) / 100

    def exactly(report, meters):
        assert report['integrated_lkfs'] == hundredths(meters['integrated_lkfs']), (report, meters)
        assert report['sample_peak_dbfs'] == hundredths(max(meters['sample_peak_dbfs'])), (report, meters)
        assert report['true_peak_dbtp'] == hundredths(max(meters['true_peak_dbtp'])), (report, meters)

    voice_only = apply(base, [edit('remove', clip_ids=['bed'], links='include'), edit('clip_audio', clip_ids=['v1', 'v2'], gain_milli=1000)])
    stopped = call({'command': 'audio.normalize', 'project': voice_only, 'input_root': str(root), 'target_lkfs': -14, 'limiter': False})
    assert stopped['limited_by'] == 'peak_ceiling' and stopped['limiter'] is None and stopped['result']['integrated_lkfs'] < -16, stopped
    before = call({'command': 'timeline.meters', 'project': voice_only, 'input_root': str(root), 'tracks': False})['mix']
    exactly(stopped['measured'], before)
    exactly(stopped['result'], call({'command': 'timeline.meters', 'project': apply(voice_only, stopped['operations']), 'input_root': str(root), 'tracks': False})['mix'])
    proposal = call({'command': 'audio.normalize', 'project': voice_only, 'input_root': str(root), 'target_lkfs': -14})
    assert proposal['result']['renders'] == 2 and proposal['result']['measured_passes'] > 2, proposal['result']
    assert proposal['limited_by'] is None and abs(proposal['result']['integrated_lkfs'] + 14) <= 0.05, proposal
    assert proposal['result']['true_peak_dbtp'] <= -1 and proposal['result']['sample_peak_dbfs'] <= -1, proposal['result']
    assert proposal['operations'][0]['edit']['op'] == 'audio_dynamics' and proposal['operations'][0]['edit']['dynamics']['limiter'] == proposal['limiter'], proposal['operations']
    assert proposal['limiter']['ceiling_dbfs'] <= -1 and proposal['result']['dynamics']['master']['max_reduction_db'] > 3, proposal
    done = apply(voice_only, proposal['operations'])
    after = call({'command': 'timeline.meters', 'project': done, 'input_root': str(root), 'tracks': False})['mix']
    exactly(proposal['result'], after)
    assert after['dynamics'] == proposal['result']['dynamics'], (after['dynamics'], proposal['result']['dynamics'])
    render(done, 'normalized-through-limiter')
    capped = call({'command': 'audio.normalize', 'project': voice_only, 'input_root': str(root), 'target_lkfs': -14, 'max_limiting_db': 2})
    assert capped['limited_by'] == 'limiter_reduction' and capped['result']['dynamics']['master']['max_reduction_db'] <= 2.3, capped
    assert capped['result']['integrated_lkfs'] < proposal['result']['integrated_lkfs'] - 1, (capped['result'], proposal['result'])
    # With a track limiter the mix has two groups, the limited voice and the bed; the replay limits
    # the voice before the sum, exactly as a render does.
    grouped = apply(base, [edit('audio_dynamics', track_id='voice', dynamics={'limiter': {'ceiling_dbfs': -6}})])
    proposal = call({'command': 'audio.normalize', 'project': grouped, 'input_root': str(root), 'target_lkfs': -14})
    exactly(proposal['measured'], call({'command': 'timeline.meters', 'project': grouped, 'input_root': str(root), 'tracks': False})['mix'])
    after = call({'command': 'timeline.meters', 'project': apply(grouped, proposal['operations']), 'input_root': str(root), 'tracks': False})['mix']
    exactly(proposal['result'], after)
    assert after['dynamics'] == proposal['result']['dynamics'] and [t['track_id'] for t in after['dynamics']['tracks']] == ['voice'], after['dynamics']
    passed.append('dynamics.normalize_proposes_limiter')

    # 7. Saved sessions keep the dynamics, the schema describes them and bad settings are refused.
    created = call({'command': 'session.create', 'store_root': str(store), 'project': base, 'request_id': 'create'})
    receipt = call({'command': 'session.apply', 'store_root': str(store), 'project_id': base['id'], 'expected_revision': 0, 'request_id': 'limit',
                    'operations': [edit('audio_dynamics', dynamics={'limiter': {'ceiling_dbfs': -1.0}}), edit('audio_dynamics', track_id='voice', dynamics={'limiter': {'ceiling_dbfs': -6, 'release_ms': 80}})]})
    saved = call({'command': 'session.get', 'store_root': str(store), 'project_id': base['id']})
    assert saved['tracks']['master'] == {'limiter': master} and saved['tracks']['tracks'][1]['dynamics'] == {'limiter': {'ceiling_dbfs': -6.0, 'lookahead_ms': 5, 'release_ms': 80}}, saved['tracks']
    # A session preview shows a limiter change in the track layout.
    diff = call({'command': 'session.preview', 'store_root': str(store), 'project_id': base['id'], 'expected_revision': 1, 'operations': [edit('audio_dynamics')]})
    assert diff['track_layout']['before']['master'] == {'limiter': master} and 'master' not in diff['track_layout']['after'], diff['track_layout']
    assert diff['track_layout']['after']['tracks'][1]['dynamics']['limiter']['ceiling_dbfs'] == -6.0
    schema = call({'command': 'schema', 'name': 'project', 'full': True})
    Draft202012Validator(schema).validate(saved)
    assert created and receipt['revision'] == 1, receipt
    for limiter in ({'ceiling_dbfs': 0.5}, {'ceiling_dbfs': -21}, {'ceiling_dbfs': -1, 'lookahead_ms': 0}, {'ceiling_dbfs': -1, 'lookahead_ms': 21}, {'ceiling_dbfs': -1, 'release_ms': 9}, {'ceiling_dbfs': -1, 'release_ms': 2001}):
        apply(base, [edit('audio_dynamics', dynamics={'limiter': limiter})], 'INVALID_AUDIO_EFFECT')
    apply(base, [edit('audio_dynamics', dynamics={'limiter': {'ceiling_dbfs': -1, 'knee_db': 2}})], 'INVALID_JSON')
    apply(base, [edit('audio_dynamics', track_id='v', dynamics={'limiter': {'ceiling_dbfs': -1}})], 'INVALID_TRACKS')
    apply(base, [edit('audio_dynamics', track_id='nope', dynamics={'limiter': {'ceiling_dbfs': -1}})], 'MISSING_TRACK')
    locked = apply(base, [edit('state', track_id='voice', locked=True, enabled=True)])
    apply(locked, [edit('audio_dynamics', track_id='voice', dynamics={'limiter': {'ceiling_dbfs': -1}})], 'TRACK_LOCKED')
    video = copy.deepcopy(base)
    video['tracks']['tracks'][0]['dynamics'] = {'limiter': {'ceiling_dbfs': -1}}
    call({'command': 'project.validate', 'project': video}, 'UNSUPPORTED_TIMELINE')
    nested = apply(base, [{'op': 'sequence.create', 'id': 'child', 'duration': time(1)}])
    nested['sequences'][0]['arrangement']['master'] = {'limiter': {'ceiling_dbfs': -1}}
    call({'command': 'project.validate', 'project': nested}, 'UNSUPPORTED_TIMELINE')
    call({'command': 'audio.normalize', 'project': voice_only, 'input_root': str(root), 'max_limiting_db': 30}, 'INVALID_ARGUMENT')
    preserved()
    passed.append('dynamics.sessions_schema_rejections')

    report = {'passed': passed, 'stereo_sample_frames_compared': counts['samples'], 'render_cases': cases, 'rejected_cases': counts['rejected'],
              'reference': 'Independent integer oracle of the documented limiter (Kaiser-sinc 4x interval peaks, lookahead minimum, linear release, mean over the lookahead, rounding half away from zero) over an independent clip/curve/fade/transition mix; every sample matches exactly. True peak also within 0.2 dB of FFmpeg ebur128.',
              'limits': 'Linked-stereo peak limiter on top-level audio tracks and the master; no compressor, sidechain or per-sequence dynamics. True peak is measured, not guaranteed.'}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
