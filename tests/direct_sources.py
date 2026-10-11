"""Direct-footage preparation: verify the generated codec/timestamp matrix independently.

This initial stage proves fixture properties only; it does not claim direct timeline decoding.
Later stages add direct-vs-conform and seek oracles to this same fixture.
"""
import argparse
from fractions import Fraction as F
import json
from pathlib import Path

from footage_sources import ROOT, generate, identity, probe


def run(root, long=False):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents, "Keep generated media outside the checkout"
    root.mkdir(parents=True, exist_ok=True)
    manifest = generate(root / "sources", long)
    verify(root, manifest, long)


def verify(root, manifest, long=False):
    results = {}
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
        results[name] = {"frames": len(frames), "first_timestamp": stamps[0], "time_base": str(tb),
                         "intervals": intervals, "codec": video["codec_name"], "profile": video.get("profile"),
                         "pix_fmt": video["pix_fmt"], "audio_rate": int(audio["sample_rate"])}
    report = {"passed": ["direct_sources.original_codec_and_clock_fixtures"], "stage": "preparation",
              "direct_decode_verified": False, "sources_preserved": True, "cases": results,
              "tools": manifest["tools"], "long_source_generated": long}
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--long-source", action="store_true")
    args = parser.parse_args()
    run(args.output, args.long_source)
