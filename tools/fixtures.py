"""Original synthetic frames and tones. No downloaded media, fonts, or assets."""
import argparse
import os
from pathlib import Path
import subprocess

WIDTH, HEIGHT, FPS, SECONDS = 320, 180, 25, 12
COLORS = [(180, 24, 24), (24, 180, 24), (24, 24, 180)]


def frame_bytes(source, frame):
    pixels = bytearray(bytes(COLORS[source]) * (WIDTH * HEIGHT))
    # A moving white square and binary frame counter make frame boundaries testable.
    def rect(x, y, w, h, color):
        row = bytes(color) * w
        for yy in range(y, y + h):
            start = (yy * WIDTH + x) * 3
            pixels[start:start + len(row)] = row
    rect(frame % (WIDTH - 20), 60, 20, 20, (255, 255, 255))
    for bit in range(12):
        rect(8 + bit * 20, 8, 16, 16, (255, 255, 255) if frame & (1 << bit) else (0, 0, 0))
    return bytes(pixels)


def generate(root):
    root = Path(root).resolve()
    root.mkdir(parents=True, exist_ok=True)
    files = []
    ffmpeg = os.environ.get("CUTBOLT_FFMPEG", "ffmpeg")
    for source in range(3):
        path = root / f"source-{source}.mkv"
        if path.exists():
            raise FileExistsError(path)
        # This expression and its resulting tone are original test content.
        tone = f"aevalsrc=0.15*sin(2*PI*{440 + source * 220}*t)+if(lt(mod(t\\,1)\\,0.002)\\,0.5\\,0):s=48000:d={SECONDS}"
        args = [ffmpeg, "-hide_banner", "-v", "error", "-n", "-f", "rawvideo", "-pixel_format", "rgb24",
                "-video_size", f"{WIDTH}x{HEIGHT}", "-framerate", str(FPS), "-i", "pipe:0",
                "-f", "lavfi", "-i", tone, "-vf", "setsar=1", "-c:v", "ffv1", "-level", "3",
                "-pix_fmt", "bgr0", "-threads", "1", "-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2", str(path)]
        process = subprocess.Popen(args, stdin=subprocess.PIPE)
        try:
            for frame in range(FPS * SECONDS):
                process.stdin.write(frame_bytes(source, frame))
            process.stdin.close()
            if process.wait(timeout=60):
                raise RuntimeError(f"Fixture generation failed: {path}")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        files.append(path)
    return files


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("output_directory", type=Path)
    args = parser.parse_args()
    for path in generate(args.output_directory):
        print(path)
