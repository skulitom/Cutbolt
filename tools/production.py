"""Build a narrated explainer from a production manifest: `python tools/production.py build <manifest> --root <dir>`.

See docs/PRODUCTION.md. The coordinator lives in tools/cutbolt_production/.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cutbolt_production.cli import main  # noqa: E402

if __name__ == "__main__":
    main()
