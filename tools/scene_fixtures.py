"""Original synthetic scene sources; generated PNG/WAV/recipes stay outside Git."""
from fractions import Fraction
from pathlib import Path
import json
import struct
import wave
from PIL import Image
from pixelforge_handoff import identity

ROWS = [
    ["........", "..RRT...", "..RRT...", "........", "........", "........"],
    ["........", "...B....", "..BBB...", "...B....", "........", "........"],
    ["........", "...TT...", "...RR...", "...RR...", "........", "........"],
]
PALETTE = {"R": (240, 96, 48, 255), "T": (32, 208, 192, 128), "B": (48, 80, 224, 255), ".": (0, 0, 0, 0)}

def time(n, d=1):
    value = Fraction(n, d)
    return {"num": value.numerator, "den": value.denominator}

def recipe():
    return {"version": 1, "name": "CutboltTiming", "width": 8, "height": 6,
            "palette": {k: "#" + "".join(f"{v:02x}" for v in rgba) for k, rgba in PALETTE.items() if k != "."},
            "anchor": [4, 3],
            "frames": [{"name": name, "duration": hold, "pixels": [{"x": x, "y": y, "color": c} for y, row in enumerate(rows) for x, c in enumerate(row) if c != "."]} for name, hold, rows in zip("abc", (40, 80, 120), ROWS)],
            "animations": {"pulse": {"frames": ["c", "a", "b", "a"], "loop": True}}, "sheet": {"trim": True, "scale": 1}}

def wav(path, rate, channels, seconds=1):
    count = rate * seconds + 7
    values = [0] * count * channels
    for n, value in [(0, 10000), (rate // 4, 32767), (rate // 2, -32768), (count - 1, 7000)]:
        for ch in range(channels):
            values[n * channels + ch] = value if ch == 0 else -(value // 2)
    with wave.open(str(path), "wb") as out:
        out.setnchannels(channels); out.setsampwidth(2); out.setframerate(rate)
        out.writeframes(struct.pack("<" + "h" * len(values), *values))
    return values

def generate(root):
    root = Path(root).resolve()
    repo = Path(__file__).resolve().parents[1]
    if root == repo or repo in root.parents:
        raise ValueError("Fixtures belong outside repository")
    root.mkdir(parents=True, exist_ok=True)
    frames = []
    for index, rows in enumerate(ROWS):
        image = Image.new("RGBA", (8, 6))
        image.putdata([PALETTE[c] for row in rows for c in row])
        path = root / f"{index}.png"; image.save(path)
        frames.append({"image": identity(path, root), "hold": time((40, 80, 120)[index], 1000), "offset": [0, 0], "anchor": [4, 3]})
    # Distinct placements, every quarter-turn, crop and partial off-canvas coverage.
    layers = []
    for index, position in enumerate(([85, 90], [92, 92], [230, 90], [-2, 8])):
        layers.append({"id": f"layer-{index}", "canvas": [8, 6], "start": time(1,25) if index == 1 else time(0), "duration": time(124,25) if index == 1 else time(5),
                       "frames": [frames[k] for k in [2, 0, 1, 0]], "timing": "strict", "end": "loop",
                       "transform": {"position": position, "crop": [1, 0, 6, 5], "scale": 12,
                                     "quarter_turns": index, "opacity": (255, 128, 230, 255)[index]}})
    wav(root / "narration.wav", 24000, 1)
    scene = {"schema_version": 1, "id": "pixel-scene", "width": 320, "height": 180, "output_scale": 6,
             "duration": time(5), "background": [12, 20, 36], "color": "srgb_straight_encoded", "layers": layers,
             "audio": {"file": identity(root / "narration.wav", root), "start": time(1, 5), "channels": "duplicate_mono", "resampling": "linear", "padding": "silence"}}
    (root / "original-recipe.json").write_text(json.dumps(recipe(), indent=2) + "\n", encoding="utf-8")
    return scene
