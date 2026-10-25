"""List Cutbolt's MCP tools in each --tools mode over stdio and compare the sets.

Usage: python -I cutbolt_tools_modes.py <cutbolt.exe> <fresh-workspace> <out.json>
Modes: default (no flag), core (what the Windows MCPB bundle's manifest passes) and full.
"""
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_session import Session  # noqa: E402

exe, ws, out_path = sys.argv[1:4]
modes = {"default": [], "core": ["--tools", "core"], "full": ["--tools", "full"]}
result = {"engine": exe, "modes": {}}
for mode, extra in modes.items():
    s = Session([exe, "--workspace", ws, "mcp"] + extra)
    s.initialize()
    msg, line, ms = s.request("tools/list", {})
    tools = (msg.get("result") or {}).get("tools", [])
    names = sorted(t["name"] for t in tools)
    result["modes"][mode] = {
        "tools": len(names),
        "wire_bytes": len(line),
        "names_sha256": hashlib.sha256("\n".join(names).encode()).hexdigest(),
        "names": names,
    }
    s.close()
d, c = set(result["modes"]["default"]["names"]), set(result["modes"]["core"]["names"])
result["default_equals_core"] = d == c
result["only_default"] = sorted(d - c)
result["only_core"] = sorted(c - d)
with open(out_path, "w", encoding="utf-8") as f:
    json.dump(result, f, indent=1)
print(json.dumps({m: (v["tools"], v["wire_bytes"]) for m, v in result["modes"].items()}),
      "default==core:", result["default_equals_core"])
