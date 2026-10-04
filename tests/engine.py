"""Engine executable used by the fixtures.

The verifier runs fixtures against a run-specific copy (CUTBOLT_EXE) so the development build in
target/debug stays free to rebuild while verification runs; without it, the development build is used.
"""
import os
from pathlib import Path

ENGINE = Path(os.environ.get("CUTBOLT_EXE") or Path(__file__).resolve().parents[1] / "target" / "debug" / ("cutbolt.exe" if os.name == "nt" else "cutbolt"))
