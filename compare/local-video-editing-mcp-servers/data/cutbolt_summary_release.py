"""Summarise the 25 Oct runs of the downloaded Cutbolt 0.4.0 release exe and compare them with the 17 Oct runs.

Usage: python -I cutbolt_summary_release.py <data folder> <data folder> <out.json>
Both folders are this page's data/ folder: the 25 Oct runs and the 17 Oct summary (cutbolt-mcp-probe-summary.json) sit there.
"""
import json
import pathlib
import statistics
import sys

runs_dir, old_dir, out = (pathlib.Path(a) for a in sys.argv[1:4])


def load(p):
    return json.loads(p.read_text(encoding="utf-8"))


colour = [load(runs_dir / f"cutbolt-mcp-probe-release-download-v0.4.0-run{n}.json") for n in (1, 2, 3)]
motion = [load(runs_dir / f"cutbolt-motion-release-download-v0.4.0-run{n}.json") for n in (1, 2)]
names = [s["step"] for s in colour[0]["steps"]]
medians = {nm: statistics.median(next(s["ms"] for s in r["steps"] if s["step"] == nm) for r in colour) for nm in names}


def sets(rs, frame_checks=True):
    d = {
        "h264_rev1_sha256_all_exports": sorted({r["files"][f]["sha256"] for r in rs for f in ("rev1-a.mp4", "rev1-b.mp4")}),
        "h264_head_sha256_all_runs": sorted({r["files"]["head.mp4"]["sha256"] for r in rs}),
        "encoder_input_video_sha256_all_exports": sorted({r["receipts"][k]["verification"]["timeline_video_sha256"]
                                                          for r in rs for k in ("export_a", "export_b")}),
        "source_sha256_before_and_after_all_runs": sorted({r["source"]["sha256_before"] for r in rs}
                                                          | {r["source"]["sha256_after"] for r in rs}),
        "source_unchanged_all_runs": all(r["checks"]["source_unchanged"] for r in rs),
        "resent_request_kept_revision_1": all(
            r["checks"]["retry_same_revision"] and r["checks"]["head_revision_after_retry"] == 1 for r in rs),
        "undo_equals_revision_0": all(r["checks"]["head_equals_rev0_apart_from_revision"] for r in rs),
    }
    if frame_checks:
        d["every_frame_and_preview_matches_expected_source_frame"] = all(
            f["frames_match_expected"] for r in rs for f in r["files"].values())
        d["reference_decoded_video_sha256_all_runs"] = sorted({r["checks"]["reference_decoded_video_sha256"] for r in rs})
        d["mp4_audio_durations"] = sorted({s["duration"] for r in rs for f in ("rev1-a.mp4", "head.mp4")
                                           for s in r["files"][f]["ffprobe"] if s["codec_name"] == "aac"})
    else:
        d["h264_rev1_decoded_video_sha256_all_exports"] = sorted(
            {r["receipts"][k]["verification"]["decoded_video_sha256"] for r in rs for k in ("export_a", "export_b")})
    return d


summary = {
    "what": "Cutbolt 0.4.0 as downloaded: cutbolt-v0.4.0-x86_64-pc-windows-msvc.zip from the v0.4.0 GitHub release "
            "(SHA-256 ee8458b2... matches SHA256SUMS); its cutbolt.exe is byte-identical to server/cutbolt.exe in "
            "cutbolt-windows-x64.mcpb (SHA-256 5f755d1e... matches cutbolt-windows-x64.mcpb.sha256)",
    "engine": colour[0]["cutbolt"], "ffmpeg": colour[0]["ffmpeg"], "os": colour[0]["os"], "cpu": colour[0]["cpu"],
    "frame_coded_clip": {"runs": [r["date"] for r in colour],
                         "tools_list": {"tools": colour[0]["steps"][1].get("tools"),
                                        "wire_bytes": colour[0]["steps"][1]["reply_bytes"]},
                         "median_ms": medians, **sets(colour)},
    "moving_pattern_clip": {"runs": [r["date"] for r in motion], **sets(motion, frame_checks=False)},
}

old = load(old_dir / "cutbolt-mcp-probe-summary.json")
cmp = {}
for label in ("main ddc77c9", "release 0.4.0 (e668c16)"):
    o_c, o_m = old["frame_coded_clip"][label], old["moving_pattern_clip"][label]
    cmp[label] = {
        "colour_h264_rev1_same": o_c["h264_rev1_sha256_all_exports"] == summary["frame_coded_clip"]["h264_rev1_sha256_all_exports"],
        "colour_h264_head_same": o_c["h264_head_sha256_all_runs"] == summary["frame_coded_clip"]["h264_head_sha256_all_runs"],
        "colour_encoder_input_same": o_c["encoder_input_video_sha256_all_exports"] == summary["frame_coded_clip"]["encoder_input_video_sha256_all_exports"],
        "motion_h264_rev1_same": o_m["h264_rev1_sha256_all_exports"] == summary["moving_pattern_clip"]["h264_rev1_sha256_all_exports"],
        "motion_encoder_input_same": o_m["encoder_input_video_sha256_all_exports"] == summary["moving_pattern_clip"]["encoder_input_video_sha256_all_exports"],
        "old_colour_h264_rev1": o_c["h264_rev1_sha256_all_exports"],
        "old_motion_h264_rev1": o_m["h264_rev1_sha256_all_exports"],
    }
summary["compared_with_17_oct_source_builds"] = cmp
out.write_text(json.dumps(summary, indent=1) + "\n", encoding="utf-8", newline="\n")
print(json.dumps({k: v for k, v in summary.items() if k != "frame_coded_clip"}, indent=1)[:3500])
print(json.dumps({k: v for k, v in summary["frame_coded_clip"].items() if k != "median_ms"}, indent=1)[:2500])
