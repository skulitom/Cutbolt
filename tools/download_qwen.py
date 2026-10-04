"""Explicit setup only: download and verify the pinned Qwen TTS snapshot externally.

Requires the separately installed huggingface-hub package. Never called by the
engine or verification suite; normal production runs must use local model paths.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import json
from pathlib import Path
import sys
from urllib.request import urlopen

REPO_ID = "Qwen/Qwen3-TTS-12Hz-0.6B-Base"
REVISION = "5d83992436eae1d760afd27aff78a71d676296fc"
ROOT = Path(__file__).resolve().parents[1]


def inspect_file(path, entry):
    """Check upstream LFS SHA-256 or Git blob SHA-1, plus size; record SHA-256."""
    size = path.stat().st_size
    if size != entry["size"]:
        raise ValueError(f"Size mismatch: {entry['rfilename']}")
    sha256 = hashlib.sha256()
    blob = hashlib.sha1(f"blob {size}\0".encode())
    with path.open("rb") as stream:
        while chunk := stream.read(8 * 1024 * 1024):
            sha256.update(chunk)
            blob.update(chunk)
    lfs = entry.get("lfs")
    expected = lfs["sha256"] if lfs else entry["blobId"]
    actual = sha256.hexdigest() if lfs else blob.hexdigest()
    if actual != expected:
        raise ValueError(f"Upstream digest mismatch: {entry['rfilename']}; existing files are not replaced")
    return {"path": entry["rfilename"], "bytes": size, "sha256": sha256.hexdigest(),
            "upstream_digest_kind": "sha256" if lfs else "git-blob-sha1",
            "upstream_digest": expected}


def download(output_root):
    if not output_root.is_absolute():
        raise ValueError("--output-root must be an absolute external directory")
    output_root = output_root.resolve()
    destination = (output_root / REPO_ID.split("/")[1] / REVISION).resolve()
    if not destination.is_relative_to(output_root) or destination.is_relative_to(ROOT):
        raise ValueError("Model destination must stay inside the external output root and outside this repository")
    from huggingface_hub import hf_hub_download

    metadata_url = f"https://huggingface.co/api/models/{REPO_ID}/revision/{REVISION}?blobs=true"
    with urlopen(metadata_url, timeout=30) as response:
        metadata = json.load(response)
    if metadata["sha"] != REVISION:
        raise ValueError("Upstream revision did not match the pinned revision")
    entries = sorted(metadata["siblings"], key=lambda entry: entry["rfilename"])
    if not entries or not any(e["rfilename"] == "speech_tokenizer/model.safetensors" for e in entries):
        raise ValueError("Snapshot is missing its bundled speech tokenizer")
    paths = []
    for entry in entries:
        path = (destination / entry["rfilename"]).resolve()
        if not path.is_relative_to(destination) or path.is_relative_to(ROOT):
            raise ValueError("Snapshot path escapes the external destination")
        paths.append(path)
        if path.exists():
            inspect_file(path, entry)
    destination.mkdir(parents=True, exist_ok=True)
    receipt_path = destination / "download-receipt.json"
    previous = None
    if receipt_path.exists():
        previous = json.loads(receipt_path.read_text(encoding="utf-8"))
        if previous.get("repository") != REPO_ID or previous.get("revision") != REVISION:
            raise ValueError("Existing receipt belongs to another snapshot")
    print(f"Pinned snapshot: {REPO_ID}@{REVISION}", flush=True)
    print(f"Destination: {destination}", flush=True)
    print(f"Snapshot bytes: {sum(e['size'] for e in entries)}; speech tokenizer included", flush=True)
    records = []
    for entry, path in zip(entries, paths):
        print(f"Checking/downloading {entry['rfilename']} ({entry['size']} bytes)", flush=True)
        if not path.exists():
            hf_hub_download(repo_id=REPO_ID, filename=entry["rfilename"], revision=REVISION,
                            local_dir=destination, token=False)
        records.append(inspect_file(path, entry))
        print(f"Verified {entry['rfilename']}", flush=True)
    if previous is not None:
        if previous.get("files") != records:
            raise ValueError("Existing receipt differs from the verified snapshot")
    else:
        receipt = {"repository": REPO_ID, "revision": REVISION,
                   "verified_at_utc": datetime.now(timezone.utc).isoformat(),
                   "reported_license": metadata.get("cardData", {}).get("license"),
                   "download_tool": {"name": "huggingface-hub",
                                     "version": importlib.metadata.version("huggingface-hub")},
                   "model_directory": str(destination), "speech_tokenizer": "speech_tokenizer",
                   "inference_tested": False, "files": records}
        with receipt_path.open("x", encoding="utf-8") as stream:
            json.dump(receipt, stream, indent=2)
            stream.write("\n")
    print(f"All {len(records)} files verified. Receipt: {receipt_path}", flush=True)
    print("Weights are ready; inference and Cutbolt compatibility have not been tested.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        download(args.output_root)
    except (ValueError, ImportError) as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception as error:
        # Avoid printing signed CDN URLs or credentials from network exceptions.
        print(f"Download failed ({type(error).__name__}). Retry the same command to resume; no completion receipt was newly issued.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
