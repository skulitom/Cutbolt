"""Local production coordinator: a manifest in, a reviewed delivery out.

The coordinator drives three independent tools through their public interfaces: PixelForge's
CLI for pixel art, a separately installed Qwen3-TTS worker for narration, and the Cutbolt
engine's JSON CLI and job queue for everything else. It keeps its own durable stage receipts
beside the production; it adds nothing to PixelForge recipes or Cutbolt snapshots, opens no
listening socket and makes no network request. See docs/PRODUCTION.md.
"""

CONTRACT_VERSION = "cutbolt-production-1"
COORDINATOR_VERSION = "1"
