"""Engine executable and build directory used by the fixtures.

The verifier runs fixtures against a run-specific copy (CUTBOLT_EXE) so the development build in
target/debug stays free to rebuild while verification runs; without it, the development build is used.
BUILD holds other build artifacts (examples, library rlibs) and follows CARGO_TARGET_DIR, which
background verification of a separate worktree points at a shared build directory.
"""
import os
from pathlib import Path

# MCP tools the engine lists; fixtures compare tools/list against this one number.
MCP_TOOLS = 69

BUILD = Path(os.environ.get("CARGO_TARGET_DIR") or Path(__file__).resolve().parents[1] / "target") / "debug"
ENGINE = Path(os.environ.get("CUTBOLT_EXE") or Path(__file__).resolve().parents[1] / "target" / "debug" / ("cutbolt.exe" if os.name == "nt" else "cutbolt"))
