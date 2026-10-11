"""Direct-footage preparation: verify the generated codec/timestamp matrix independently.

This initial stage proves fixture properties only; it does not claim direct timeline decoding.
Later stages add direct-vs-conform and seek oracles to this same fixture.
"""
import argparse
from fractions import Fraction as F
import json
import os
from pathlib import Path
import shutil
import subprocess
import source_clocks
import direct_decode

from engine import ENGINE
from footage_sources import ROOT, FFMPEG, FFPROBE, generate, identity, probe


def counted_tools(root):
    source = root / 'counted-index-tool.rs'
    source.write_text(r'''
use std::{env, fs::OpenOptions, io::Write, process::{Command, exit}};
fn main() {
    let probe = env::current_exe().unwrap().file_stem().unwrap().to_string_lossy().contains("probe");
    let args: Vec<String> = env::args().skip(1).collect();
    let kind = if probe { if args.iter().any(|a| a == "-show_frames") { "frames" } else { "metadata" } } else { "audio" };
    let mut log = OpenOptions::new().create(true).append(true).open(env::var("CUTBOLT_INDEX_LOG").unwrap()).unwrap();
    writeln!(log,"{kind}").unwrap();
    let program = env::var(if probe { "CUTBOLT_INDEX_PROBE" } else { "CUTBOLT_INDEX_DECODER" }).unwrap();
    exit(Command::new(program).args(args).status().unwrap().code().unwrap_or(1));
}
''', encoding='utf-8')
    decoder, prober = root / 'index-decoder.exe', root / 'index-probe.exe'
    subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(decoder)], check=True, capture_output=True)
    shutil.copy2(decoder, prober)
    log = root / 'index-launches.txt'
    return {**os.environ, 'CUTBOLT_FFMPEG': str(decoder), 'CUTBOLT_FFPROBE': str(prober),
            'CUTBOLT_INDEX_DECODER': shutil.which(FFMPEG), 'CUTBOLT_INDEX_PROBE': shutil.which(FFPROBE),
            'CUTBOLT_INDEX_LOG': str(log)}, log


def run(root, long=False):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents, "Keep generated media outside the checkout"
    root.mkdir(parents=True, exist_ok=True)
    manifest = generate(root / "sources", long)
    verify(root, manifest, long)


def verify(root, manifest, long=False):
    results = {}
    conformed = {}
    env, launches = counted_tools(root)
    def inspect(path):
        process = subprocess.run([str(ENGINE), '--workspace', str(root)], input=json.dumps({
            'command':'media.inspect', 'path':str(path)}), env=env, capture_output=True, text=True, encoding='utf-8', timeout=1800)
        result = json.loads(process.stdout)
        assert process.returncode == 0 and result['ok'], result
        indexed = result['result']['source_index']
        assert 'error' not in indexed, indexed
        assert indexed['direct_decode_available'] is False and result['result']['timeline']['ready'] is False
        return indexed
    for name, case in manifest["cases"].items():
        path = root / "sources" / case["path"]
        data = probe(path)
        video = next(s for s in data["streams"] if s["codec_type"] == "video")
        audio = next(s for s in data["streams"] if s["codec_type"] == "audio")
        frames = [f for f in data["frames"] if f["media_type"] == "video"]
        stamps = [int(f["best_effort_timestamp"]) for f in frames]
        tb = F(video["time_base"])
        assert len(stamps) == len(set(stamps)) and stamps == sorted(stamps), name
        assert video["pix_fmt"] == case["pix_fmt"], (name, video["pix_fmt"])
        assert [video["width"], video["height"]] == case["size"], name
        assert int(audio["sample_rate"]) == case["audio_rate"] and audio["channels"] == case["channels"], name
        assert video["codec_name"] == {"h264": "h264", "hevc": "hevc", "prores": "prores"}[case["codec"]]
        intervals = sorted(set(b - a for a, b in zip(stamps, stamps[1:])))
        if name == "h264-vfr":
            assert len(intervals) > 1 and len(frames) < case["encoded_frames"], intervals
        else:
            assert len(frames) == case["encoded_frames"], (name, len(frames))
            assert all(abs((stamp - stamps[0]) * tb - F(n) / F(case["rate"])) <= tb / 2
                       for n, stamp in enumerate(stamps)), name
        if case["codec"] in {"h264", "hevc"}:
            assert any(f["pict_type"] == "B" for f in frames), name
            keys = [n for n, f in enumerate(frames) if f["key_frame"]]
            if name != "h264-vfr":
                assert max(b - a for a, b in zip(keys, keys[1:])) == case["gop"], (name, keys)
        if name == "h264-edit-list":
            assert stamps[0] * tb == F(2, 5), (name, stamps[0], tb)
            assert F(audio["start_time"]) > 0, audio
        if name == "h264-rotation":
            assert any(abs(s.get("rotation", 0)) == 90 for s in video.get("side_data_list", [])), video
        if name == "h264-sar":
            assert video["sample_aspect_ratio"] == "4:3", video
        if name == "prores-hq":
            assert video["profile"] == "HQ", video
        assert identity(path) == {k: case[k] for k in ("bytes", "sha256")}
        before = len(launches.read_text().splitlines()) if launches.exists() else 0
        indexed = inspect(path)
        first_calls = launches.read_text().splitlines()[before:]
        assert first_calls == ['metadata', 'frames', 'audio'], (name, first_calls)
        assert not indexed['cache_hit'] and indexed['cache_bytes'] > 0
        repeated = inspect(path)
        assert repeated['cache_hit'] and repeated['key'] == indexed['key']
        assert launches.read_text().splitlines()[before + len(first_calls):] == ['metadata']
        qualifies = name not in {'h264-vfr','h264-rotation','h264-sar','hevc-main10','prores-hq'}
        assert indexed['eligible'] == qualifies and bool(indexed['reasons']) != qualifies, (name, indexed)
        assert indexed['frames'] == len(frames) and indexed['first_timestamp'] == stamps[0]
        assert F(indexed['time_base']['num'],indexed['time_base']['den']) == tb
        assert indexed['keyframes'] == sum(f['key_frame'] for f in frames)
        assert indexed['b_frames'] == sum(f['pict_type'] == 'B' for f in frames)
        audio_frames = [f for f in data['frames'] if f['media_type'] == 'audio']
        decoded = indexed['audio']
        assert decoded['decoded_samples'] == sum(f['nb_samples'] for f in audio_frames), (name, decoded)
        first_audio = int(audio_frames[0]['best_effort_timestamp']) * F(audio['time_base'])
        assert abs(F(decoded['first_pts'],decoded['sample_rate']) - first_audio) <= F(audio['time_base'])/2
        assert decoded['channels'] == case['channels'] and decoded['sample_rate'] == case['audio_rate']
        if qualifies:
            policy = indexed['audio_policy']
            relative = F(decoded['first_pts'],decoded['sample_rate']) - stamps[0]*tb
            duration = F(len(frames))/F(case['rate'])
            tail = relative + F(decoded['decoded_samples'],decoded['sample_rate']) - duration
            assert policy['audio_first_relative_to_video'] == source_clocks.time(relative)
            assert policy['start'] == ('trim_before_video' if relative < 0 else 'pad_with_silence' if relative > 0 else 'aligned')
            assert policy['tail'] == ('trim_after_video' if tail > 0 else 'pad_with_silence' if tail < 0 else 'aligned')
            assert policy['channels'] == ('duplicate_mono' if case['channels']==1 else 'preserve_stereo')
            assert policy['video_duration'] == source_clocks.time(duration)
        assert identity(path) == {k: case[k] for k in ('bytes','sha256')}
        results[name] = {"frames": len(frames), "first_timestamp": stamps[0], "time_base": str(tb),
                         "intervals": intervals, "codec": video["codec_name"], "profile": video.get("profile"),
                         "pix_fmt": video["pix_fmt"], "audio_rate": int(audio["sample_rate"]), "index":indexed}
        if qualifies and name != 'h264-20min':
            conformed[name] = source_clocks.verify(root,name,case,data)
            offset = conformed[name]['audio_sample_offset']
            assert F(offset['num'],offset['den']) == -relative * decoded['sample_rate'], (name, offset, relative)
    decoded_ranges = direct_decode.verify(root, manifest, results)
    report = {"passed": ["direct_sources.original_codec_and_clock_fixtures", "direct_sources.cached_presented_frame_and_pcm_index", "direct_sources.explicit_sdr_clock_rgb_pcm_reference", "direct_sources.library_native_yuv_exact_sought_frames"], "stage": "indexed_video_library",
              "direct_decode_verified": False, "sources_preserved": True, "cases": results,
              "tools": manifest["tools"], "long_source_generated": long, "conform_references":conformed,
              "library_video_ranges":decoded_ranges}
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--long-source", action="store_true")
    args = parser.parse_args()
    run(args.output, args.long_source)
