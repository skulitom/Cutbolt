"""Large media imports: sources and outputs beyond the original 64 MiB / 60 s conform bounds.

Independent checks: every output frame and PCM sample against the exactly known synthetic source
(lossless FFV1/PCM, nearest-neighbour downscale, documented linear audio interpolation), forward
streaming versus reverse random-access windows, a late audio window from a WAV longer than ten minutes,
peak process memory that does not grow with output length, and explicit limit rejections without output.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import wave

import numpy as np

from native_scenes import Watch, call
from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]
W, H, FRAMES = 256, 144, 1800
OUT_W, OUT_H = 64, 36
WAV_SECONDS = 720
MIB = 1024 * 1024


def noise():
    return np.random.default_rng(20261004).integers(0, 256, (H, W, 3), dtype=np.uint16)


def source_frame(base, n):
    frame = ((base + (n * 37 + (n // 256) * 101) % 256) % 256).astype(np.uint8)
    frame[0, 0] = (n & 255, n >> 8, 0xA5)
    return frame


def stereo(count, offset=0):
    n = np.arange(offset, offset + count, dtype=np.int64)
    left = (n * 13) % 18001 - 9000
    right = (n * 13 + 917) % 18001 - 9000
    return np.stack([left, right], axis=1).astype(np.int16)


def write_wav(path, samples):
    with wave.open(str(path), "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(48000)
        f.writeframes(samples.tobytes())


def generate(sources):
    base = noise()
    write_wav(sources / "movie-audio.wav", stereo(FRAMES * 48000 // 25))
    encoder = subprocess.Popen(["ffmpeg", "-v", "error", "-nostdin", "-n", "-f", "rawvideo", "-pixel_format", "rgb24",
                                "-video_size", f"{W}x{H}", "-framerate", "25", "-i", "pipe:0", "-i", str(sources / "movie-audio.wav"),
                                "-map", "0:v:0", "-map", "1:a:0", "-c:v", "ffv1", "-pix_fmt", "bgr0", "-c:a", "pcm_s16le",
                                "-vf", "setsar=1", str(sources / "long.mkv")], stdin=subprocess.PIPE)
    for n in range(FRAMES):
        encoder.stdin.write(source_frame(base, n).tobytes())
    encoder.stdin.close()
    assert encoder.wait(timeout=600) == 0
    (sources / "movie-audio.wav").unlink()
    write_wav(sources / "long.wav", stereo(WAV_SECONDS * 48000, offset=7))
    # A large-frame source for the reverse random-access window limit: 3840x2080 stays under eight million
    # pixels, and 190 reversed frames need about 4.5 GB of decoded RGB.
    subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", "testsrc2=s=3840x2080:r=25:d=8",
                    "-c:v", "ffv1", "-pix_fmt", "bgr0", "-vf", "setsar=1", str(sources / "wide.mkv")], check=True, timeout=600)
    return base


def decode(path, kind):
    args = ["-map", "0:v:0", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"] if kind == "video" else ["-map", "0:a:0", "-f", "s16le", "-"]
    return subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(path)] + args, capture_output=True, check=True, timeout=600).stdout


def expected_video(base, indices):
    return b"".join(source_frame(base, i)[::H // OUT_H, ::W // OUT_W].tobytes() for i in indices)


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=False)
    sources, output = root / "sources", root / "output"
    for p in (sources, output):
        p.mkdir()
    base = generate(sources)
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    sizes = {p.name: p.stat().st_size for p in sources.iterdir()}
    assert sizes["long.mkv"] > 64 * MIB and sizes["long.wav"] > 128 * MIB, sizes
    passed, cases = [], []

    capabilities = call({"command": "capabilities", "section": "all"})["conform"]
    assert capabilities["maximum_output_frames"] >= 45000 and capabilities["source_maximum_bytes"] >= 16 * 1024 * MIB
    assert capabilities["source_maximum_audio_seconds"] >= 3600
    assert capabilities["decoding"]["forward"] == "streamed_needed_frames"
    passed.append("large_imports.capabilities_report_limits")

    def recipe(name, label, **changes):
        value = {"schema_version": 1, "id": label, "source": {"file": identity(sources / name, sources),
                 "color": None if name.endswith(".wav") else "encoded_rgb"},
                 "source_in": time(0), "duration": time(1), "rate": time(1), "reverse": False, "freeze": False,
                 "width": OUT_W, "height": OUT_H, "audio": "resample"}
        value.update(changes)
        return value

    def conform(value, label):
        watch = Watch(output)
        receipt, seconds = watch.run({"command": "media.conform", "recipe": value, "input_root": str(sources),
                                      "output_root": str(output), "output": str(output / f"{label}.mkv")})
        return receipt, {"seconds": round(seconds, 3), "peak_process_tree_bytes": watch.peak_rss, "peak_scratch_bytes": watch.peak_scratch}

    movie_audio = stereo(FRAMES * 48000 // 25).astype(np.int64)

    def check(label, value, indices, audio, frames=None):
        frames = len(indices) if frames is None else frames
        plan = call({"command": "media.conform.inspect", "recipe": value, "input_root": str(sources)})
        receipt, measured = conform(value, label)
        path = output / f"{label}.mkv"
        assert receipt["source_frame_indices"] == plan["source_frame_indices"] == indices, label
        assert receipt["frames"] == frames and receipt["asset"]["identity"]["sha256"] == hashlib.sha256(path.read_bytes()).hexdigest()
        video = decode(path, "video")
        expected = expected_video(base, indices) if value["source"]["color"] else bytes(OUT_W * OUT_H * 3 * frames)
        assert len(video) == len(expected) and video == expected, label
        assert decode(path, "audio") == audio.tobytes(), label
        measured["output_bytes"] = path.stat().st_size
        cases.append({"case": label, "frames": frames, "sample_frames": len(audio), **measured})
        return measured

    # Forward 1:1 output longer than 60 seconds streams only the needed frames.
    long_forward = check("long-forward", recipe("long.mkv", "long-forward", duration=time(70)), list(range(1750)), movie_audio[:70 * 48000].astype(np.int16))
    short_forward = check("short-forward", recipe("long.mkv", "short-forward", duration=time(10)), list(range(250)), movie_audio[:10 * 48000].astype(np.int16))
    # Streaming never spills decoded source frames: scratch holds the source and output PCM and the encoded output.
    assert long_forward["peak_scratch_bytes"] <= long_forward["output_bytes"] + 2 * 70 * 48000 * 4 + 4 * MIB, long_forward
    passed.append("large_imports.forward_beyond_sixty_seconds_exact")

    # Half speed from 50 s: every source frame is repeated and audio interpolates halfway between samples.
    slow_indices = [1250 + n // 2 for n in range(1000)]
    n = np.arange(40 * 48000)
    low = 50 * 48000 + n // 2
    pairs = movie_audio[low] + movie_audio[np.minimum(low + (n % 2), len(movie_audio) - 1)]
    slow_audio = (np.sign(pairs) * ((np.abs(pairs) + 1) // 2)).astype(np.int16)
    check("slow-tail", recipe("long.mkv", "slow-tail", source_in=time(50), duration=time(40), rate=time(1, 2)), slow_indices, slow_audio)
    freeze = check("freeze-late", recipe("long.mkv", "freeze-late", source_in=time(65), duration=time(2), freeze=True, audio="mute"), [1625] * 50, np.zeros((2 * 48000, 2), np.int16))
    assert freeze["peak_scratch_bytes"] <= freeze["output_bytes"] + 2 * 2 * 48000 * 4 + 4 * MIB, freeze
    passed.append("large_imports.late_slow_and_freeze_streams_exact")

    # Reverse from the final frame reads a bounded random-access window in scratch.
    reverse = check("reverse-window", recipe("long.mkv", "reverse-window", source_in=time(1799, 25), duration=time(20), reverse=True, audio="mute"),
                    [1799 - n for n in range(500)], np.zeros((20 * 48000, 2), np.int16))
    assert reverse["peak_scratch_bytes"] >= 500 * W * H * 3, reverse
    passed.append("large_imports.reverse_window_exact")

    # Only the needed audio window of a twelve-minute WAV is decoded.
    start = 660 * 48000
    wav_audio = stereo(50 * 48000, offset=7 + start)
    late = check("late-audio", recipe("long.wav", "late-audio", source_in=time(660), duration=time(50)), [], wav_audio, frames=1250)
    assert late["peak_process_tree_bytes"] < 512 * MIB, late
    passed.append("large_imports.long_wav_audio_window_exact")

    # Memory follows the work window, not source or output length.
    assert long_forward["peak_process_tree_bytes"] <= short_forward["peak_process_tree_bytes"] * 1.25 + 32 * MIB, (long_forward, short_forward)
    assert long_forward["peak_process_tree_bytes"] < 1024 * MIB, long_forward
    passed.append("large_imports.memory_independent_of_length")

    # Explicit rejections leave no output or scratch behind.
    before = sorted(p.name for p in output.iterdir())
    wide = call({"command": "media.conform", "recipe": recipe("wide.mkv", "wide", source_in=time(199, 25), duration=time(38, 5), reverse=True, audio="mute"),
                 "input_root": str(sources), "output_root": str(output), "output": str(output / "wide.mkv")}, "LIMIT_EXCEEDED")
    assert "4 GiB" in wide["message"], wide
    too_long = call({"command": "media.conform", "recipe": recipe("long.mkv", "too-long", duration=time(45001, 25)),
                     "input_root": str(sources), "output_root": str(output), "output": str(output / "too-long.mkv")}, "INVALID_CONFORM")
    assert "45000" in too_long["message"], too_long
    beyond = call({"command": "media.conform", "recipe": recipe("long.mkv", "beyond", source_in=time(70), duration=time(3)),
                   "input_root": str(sources), "output_root": str(output), "output": str(output / "beyond.mkv")}, "INVALID_RANGE")
    assert sorted(p.name for p in output.iterdir()) == before, before
    passed.append("large_imports.explicit_rejections_without_output")

    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()} == originals
    passed.append("large_imports.sources_preserved")
    report = {"passed": passed, "source_bytes": sizes, "previous_bounds": {"source_bytes": 64 * MIB, "output_frames": 1500, "audio_seconds": 600},
              "cases": cases, "frames_compared": sum(c["frames"] for c in cases), "sample_frames_compared": sum(c["sample_frames"] for c in cases),
              "rejections": [wide["message"], too_long["message"], beyond["message"]],
              "oracle": "exactly generated lossless sources, nearest-neighbour downscale, integer half-sample interpolation"}
    for p in sources.iterdir():
        if p.stat().st_size > 64 * MIB:
            p.unlink()
    shutil.rmtree(output)
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
