"""Offline Qwen3-TTS CustomVoice worker for the production coordinator.

Runs inside the separately installed speech environment (for example WSL with the
qwen-tts packages on PYTHONPATH), never inside the engine. It loads the model once,
synthesizes every requested line in batches, and publishes each take as a new PCM16
WAV plus one receipt. It never downloads: the Hugging Face offline flags are set before
any model code is imported, and a missing model directory fails before loading.

usage: python3 qwen_tts_worker.py <request.json>

request: {"schema": "cutbolt-tts-request-1", "model": <absolute model dir>, "revision": str,
          "speaker": str, "language": str, "seed": int, "instruct": str | null,
          "batch_size": 1-32, "output_dir": <absolute, existing, empty directory>,
          "lines": [{"id": str, "text": str, "file": "<name>.wav",
                     "parts": [str, ...] (optional), "pause": seconds (optional)}]}

A line with `parts` is spoken part by part (normally one sentence each) and joined with `pause`
seconds of silence. Every part of every line goes into the same batches, and a batch takes as long
as its longest part, so splitting long lines into sentences shortens the whole run.

The receipt (receipt.json in output_dir, and the last stdout line) lists every line's
file, sample rate, channels, samples, SHA-256, bytes and float peak, plus load and
generation times, the batch layout and peak GPU memory. Neural output is not promised
to be identical across batch layouts, devices or package versions.
"""
import hashlib
import json
import os
import sys
import time
import wave
from pathlib import Path

os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"

SCHEMA = "cutbolt-tts-request-1"
SAFE = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.")


def fail(code, message):
    print(json.dumps({"ok": False, "error": {"code": code, "message": message}}), flush=True)
    sys.exit(1)


def load_request(path):
    request = json.loads(Path(path).read_text(encoding="utf-8"))
    if request.get("schema") != SCHEMA:
        fail("INVALID_REQUEST", f"schema must be {SCHEMA}")
    model = Path(request["model"])
    if not model.is_absolute() or not (model / "config.json").is_file():
        fail("MODEL_MISSING", f"model directory {model} has no config.json; run the explicit setup, nothing is downloaded")
    out = Path(request["output_dir"])
    if not out.is_absolute() or not out.is_dir() or any(out.iterdir()):
        fail("INVALID_REQUEST", f"output_dir {out} must be an existing empty absolute directory")
    lines = request["lines"]
    if not 1 <= len(lines) <= 200:
        fail("INVALID_REQUEST", "1-200 lines required")
    names = set()
    for line in lines:
        name = line["file"]
        if not name.endswith(".wav") or set(name) - SAFE or name in names or name.startswith("."):
            fail("INVALID_REQUEST", f"unsafe or repeated file name {name!r}")
        names.add(name)
        if not isinstance(line["text"], str) or not line["text"].strip() or len(line["text"]) > 2000:
            fail("INVALID_REQUEST", f"line {line['id']!r} needs 1-2000 characters of text")
        parts = line.get("parts", [line["text"]])
        if not isinstance(parts, list) or not 1 <= len(parts) <= 32 or any(not isinstance(t, str) or not t.strip() for t in parts):
            fail("INVALID_REQUEST", f"line {line['id']!r}: parts must be 1-32 nonblank texts")
        pause = line.get("pause", 0)
        if not isinstance(pause, (int, float)) or not 0 <= pause <= 2:
            fail("INVALID_REQUEST", f"line {line['id']!r}: pause must be 0-2 seconds")
    batch = request.get("batch_size", 8)
    if type(batch) is not int or not 1 <= batch <= 32:
        fail("INVALID_REQUEST", "batch_size must be 1-32")
    return request, model, out


def publish(out, name, pcm, rate):
    """Write a new mono PCM16 WAV beside its final name, then link it into place without replacing anything."""
    final = out / name
    partial = out / f".{name}.partial"
    with open(partial, "xb") as raw, wave.open(raw, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(rate))
        w.writeframes(pcm.tobytes())
        raw.flush()
        os.fsync(raw.fileno())
    os.link(partial, final)
    os.unlink(partial)
    data = final.read_bytes()
    return hashlib.sha256(data).hexdigest(), len(data)


def main():
    if len(sys.argv) != 2:
        fail("INVALID_REQUEST", "usage: qwen_tts_worker.py <request.json>")
    request, model_dir, out = load_request(sys.argv[1])
    started = time.perf_counter()
    import numpy as np
    import torch
    from qwen_tts import Qwen3TTSModel

    model = Qwen3TTSModel.from_pretrained(str(model_dir), device_map="cuda:0", dtype=torch.bfloat16, attn_implementation="sdpa")
    load_s = time.perf_counter() - started
    # The talker generates a whole batch at once, which is where the time goes. Decoding a padded
    # batch of codes trips a masking bug with torch 2.3 (an in-place op on an expanded mask), so the
    # speech tokenizer decodes each line's codes on its own; that step is short.
    tokenizer = model.model.speech_tokenizer
    batch_decode = tokenizer.decode

    def decode_each(items):
        wavs, rate = [], None
        for item in items:
            one, rate = batch_decode([item])
            wavs.extend(one)
        return wavs, rate

    tokenizer.decode = decode_each
    speakers = [s.lower() for s in model.get_supported_speakers()]
    if request["speaker"].lower() not in speakers:
        fail("INVALID_REQUEST", f"speaker {request['speaker']!r} is not one of {speakers}")
    receipt = {
        "schema": "cutbolt-tts-receipt-1", "model": str(model_dir), "revision": request["revision"],
        "speaker": request["speaker"], "language": request["language"], "seed": request["seed"],
        "instruct": request.get("instruct"), "batch_size": request.get("batch_size", 8),
        "offline": {"HF_HUB_OFFLINE": os.environ["HF_HUB_OFFLINE"], "TRANSFORMERS_OFFLINE": os.environ["TRANSFORMERS_OFFLINE"]},
        "torch": torch.__version__, "cuda_device": torch.cuda.get_device_name(0),
        "load_seconds": round(load_s, 2), "batches": [], "lines": [],
    }
    torch.cuda.reset_peak_memory_stats()
    # Every part of every line, longest first, so each batch holds parts of similar length and pads little.
    items = [(line, i, text) for line in request["lines"] for i, text in enumerate(line.get("parts", [line["text"]]))]
    items.sort(key=lambda item: -len(item[2]))
    size = request.get("batch_size", 8)
    torch.manual_seed(request["seed"])
    audio, rate = {}, None
    for first in range(0, len(items), size):
        batch = items[first:first + size]
        t0 = time.perf_counter()
        kwargs = {}
        if request.get("instruct"):
            kwargs["instruct"] = [request["instruct"]] * len(batch)
        wavs, rate = model.generate_custom_voice(
            text=[text for _, _, text in batch], speaker=[request["speaker"]] * len(batch),
            language=[request["language"]] * len(batch), **kwargs)
        elapsed = time.perf_counter() - t0
        receipt["batches"].append({"parts": [f"{line['id']}#{i}" for line, i, _ in batch], "seconds": round(elapsed, 2)})
        for (line, i, _), samples in zip(batch, wavs):
            part = np.asarray(samples, dtype=np.float32).reshape(-1)
            if part.size == 0:
                fail("EMPTY_TAKE", f"line {line['id']!r} part {i} produced no audio")
            audio[(line["id"], i)] = part
        print(json.dumps({"progress": len(audio), "of": len(items), "batch_seconds": round(elapsed, 2)}), file=sys.stderr, flush=True)
    for line in request["lines"]:
        count = len(line.get("parts", [line["text"]]))
        gap = np.zeros(int(round(line.get("pause", 0) * rate)), dtype=np.float32)
        pieces = []
        for i in range(count):
            if i:
                pieces.append(gap)
            pieces.append(audio[(line["id"], i)])
        joined = np.concatenate(pieces)
        peak = float(np.max(np.abs(joined)))
        pcm = np.clip(np.round(joined * 32767.0), -32768, 32767).astype("<i2")
        digest, size_bytes = publish(out, line["file"], pcm, rate)
        receipt["lines"].append({
            "id": line["id"], "text": line["text"], "file": line["file"], "sample_rate": int(rate), "channels": 1,
            "samples": int(pcm.size), "seconds": round(pcm.size / rate, 3), "float_peak": round(peak, 4),
            "parts": count, "part_seconds": [round(audio[(line["id"], i)].size / rate, 3) for i in range(count)],
            "pause": line.get("pause", 0), "sha256": digest, "bytes": size_bytes})
    receipt["peak_gpu_allocated_mib"] = round(torch.cuda.max_memory_allocated() / 2**20, 1)
    receipt["total_seconds"] = round(time.perf_counter() - started, 2)
    with open(out / "receipt.json", "x", encoding="utf-8") as file:
        json.dump(receipt, file, indent=1)
    print(json.dumps({"ok": True, "result": receipt}), flush=True)


if __name__ == "__main__":
    main()
