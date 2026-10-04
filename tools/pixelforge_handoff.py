"""Read PixelForge's public atlas/file handoff; never import companion implementation.

This optional development adapter needs Pillow. The Rust engine accepts ordinary PNG/WAV
without PixelForge, this adapter, Python, or a network connection.
"""
import argparse
import hashlib
import json
from pathlib import Path
from fractions import Fraction
from PIL import Image


def identity(path, root):
    root = Path(root).resolve(strict=True)
    path = Path(path).resolve(strict=True)
    relative = path.relative_to(root)
    data = path.read_bytes()
    return {"path": relative.as_posix(), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}


def load_animation(atlas_path, input_root, animation):
    root = Path(input_root).resolve(strict=True)
    atlas_path = Path(atlas_path).resolve(strict=True)
    atlas_identity = identity(atlas_path, root)
    if atlas_identity["bytes"] > 4 * 1024 * 1024:
        raise ValueError("Atlas exceeds 4 MiB")
    atlas = json.loads(atlas_path.read_text(encoding="utf-8"))
    if atlas["meta"]["app"] != "PixelForge" or atlas["meta"]["version"] != "1":
        raise ValueError("Expected the tested PixelForge v1 atlas contract")
    selected = atlas["animations"][animation]
    order = selected["frames"]
    if not isinstance(order, list) or not 1 <= len(order) <= 1024 or type(selected["loop"]) is not bool:
        raise ValueError("Invalid animation order or loop policy")
    frames = []
    canvas = None
    metadata = []
    for name in order:
        if not isinstance(name, str) or not name or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-" for c in name):
            raise ValueError("Unsafe frame name")
        entry = atlas["frames"][name]
        if entry["rotated"] is not False:
            raise ValueError("Rotated atlas entries are unsupported")
        size = [entry["sourceSize"][k] for k in ("w", "h")]
        if any(type(n) is not int or not 1 <= n <= 512 for n in size) or (canvas and size != canvas):
            raise ValueError("Source canvas is inconsistent or exceeds 512 squared")
        canvas = size
        anchor = [entry["anchor"][k] for k in ("x", "y")]
        if any(type(n) is not int or not -4096 <= n <= 4096 for n in anchor):
            raise ValueError("Invalid anchor")
        duration = entry["duration"]
        if type(duration) is not int or not 1 <= duration <= 60000:
            raise ValueError("Invalid frame duration")
        path = atlas_path.parent / "frames" / (name + ".png")
        image_identity = identity(path, root)
        with Image.open(root / image_identity["path"]) as image:
            if image.format != "PNG" or image.mode not in ("RGB", "RGBA") or list(image.size) != canvas or getattr(image, "n_frames", 1) != 1:
                raise ValueError("Expected full-canvas individual RGB/RGBA PNG")
        hold = Fraction(duration, 1000)
        frames.append({"image": image_identity, "hold": {"num": hold.numerator, "den": hold.denominator}, "offset": [0, 0], "anchor": anchor})
        metadata.append({"name": name, "trimmed": entry["trimmed"], "spriteSourceSize": entry["spriteSourceSize"]})
    return {"canvas": canvas, "frames": frames, "end": "loop" if selected["loop"] else "hold_last",
            "provenance": {"adapter": "cutbolt-pixelforge-files-v1", "atlas": atlas_identity, "animation": animation,
                           "order": order, "metadata": metadata, "frame_storage": "full_canvas_png"}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--atlas", required=True, type=Path)
    parser.add_argument("--input-root", required=True, type=Path)
    parser.add_argument("--animation", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    repo = Path(__file__).resolve().parents[1]
    if output == repo or repo in output.parents:
        raise SystemExit("Generated handoff belongs outside the repository")
    result = load_animation(args.atlas, args.input_root, args.animation)
    with output.open("x", encoding="utf-8") as file:
        json.dump(result, file, indent=2)
        file.write("\n")
