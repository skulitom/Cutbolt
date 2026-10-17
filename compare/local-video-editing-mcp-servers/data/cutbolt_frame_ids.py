"""Read back the source frame number carried by each frame of a video made from the synthetic test clip.

The test clip gives frame N a flat colour: red = (N mod 16) * 16 + 8, green = floor(N / 16) * 12 + 8, blue = 64.
This decodes an 8x8 patch from the centre of every frame with FFmpeg, averages it and maps the colour back to N.
Usage: python -I cutbolt_frame_ids.py <ffmpeg.exe> <video> [yuv]
Pass "yuv" for YUV video (BT.709 limited range is converted to RGB); RGB video (FFV1 bgr0, PNG) needs nothing.
Prints JSON: frame count, the decoded N list, and the largest distance from a palette colour (a quality check).
"""
import json
import subprocess
import sys

ffmpeg, video = sys.argv[1], sys.argv[2]
yuv = len(sys.argv) > 3 and sys.argv[3] == "yuv"
vf = "crop=8:8:(iw-8)/2:(ih-8)/2"
if yuv:
    vf += ",scale=in_color_matrix=bt709:in_range=tv:out_range=pc"
vf += ",format=rgb24"
raw = subprocess.run([ffmpeg, "-v", "error", "-i", video, "-map", "0:v:0", "-vf", vf, "-f", "rawvideo", "-"],
                     check=True, capture_output=True).stdout
size = 8 * 8 * 3
ids, worst = [], 0.0
for i in range(len(raw) // size):
    block = raw[i * size:(i + 1) * size]
    r = sum(block[0::3]) / 64
    g = sum(block[1::3]) / 64
    b = sum(block[2::3]) / 64
    lo, hi = round((r - 8) / 16), round((g - 8) / 12)
    n = hi * 16 + lo
    dist = max(abs(r - (lo * 16 + 8)), abs(g - (hi * 12 + 8)), abs(b - 64))
    worst = max(worst, dist)
    ids.append(n)
print(json.dumps({"video": video, "frames": len(ids), "worst_palette_distance": round(worst, 2), "ids": ids}))
