"""Generate a reproducible scope score from explicit, verified acceptance checks."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def source_hashes():
    files = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "build.rs", ROOT / "docs/MCP_EVALUATION.md"]
    files.extend(sorted((ROOT / "examples").glob("*-template.json")))
    files.extend(sorted((ROOT / "examples").glob("*.rs")))
    for directory, pattern in [("src", "*.rs"), ("tools", "*.py"), ("tests", "*.py"), ("tests", "*.rs")]:
        files.extend(sorted((ROOT / directory).rglob(pattern)))
    return {str(path.relative_to(ROOT)).replace("\\", "/"): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}


def generate():
    data = json.loads((ROOT / "progress/capabilities.json").read_text(encoding="utf-8"))
    report = json.loads((ROOT / "verification/latest.json").read_text(encoding="utf-8"))
    if report["baseline"] != data["baseline"]:
        raise ValueError("Scope baseline and verification baseline disagree")
    if report["source_hashes"] != source_hashes():
        raise ValueError("Verification is stale. Run python tools/verify.py before scoring changed code.")
    features = data["features"]
    if len(features) != 50 or len({item["id"] for item in features}) != 50:
        raise ValueError("Baseline v1 requires exactly 50 unique capabilities (100 acceptance points). Version scope changes explicitly.")
    known = set(report["passed"])

    def verified(evidence):
        if set(evidence) - known:
            raise ValueError(f"Claim lacks passing evidence: {set(evidence) - known}")
        return bool(evidence)

    total = 0
    rows = []
    groups = {}
    for feature in features:
        score = sum(verified(feature.get(key + "_evidence", [])) for key in ["basic", "extended"])
        total += score
        earned, possible = groups.get(feature["category"], (0, 0))
        groups[feature["category"]] = (earned + score, possible + 2)
        rows.append(f"| {feature['id']} | {feature['name']} | {score}/2 | {feature['basic']} | {feature['extended']} |")
    agent_total = sum(verified(item["evidence"]) for item in data["agent_checks"])
    text = ["# Progress against the selected editing capabilities", "",
            f"**Verified checklist coverage: {total:.1f}% ({total}/100 acceptance points).**", "",
            f"Baseline: `{data['baseline']}`. Verified: {report['date']}. All {len(features)} capability groups remain in the denominator.", "",
            "This measures our explicitly scoped checklist, not equal engineering effort, cross-product equivalence or completion of every professional editing feature. Each capability has two explicit checkpoints worth one point each. A working narrow subset earns one; its broader acceptance cases must pass to earn the second. Plans and code without passing evidence earn zero.", "",
            "## Accepted exclusions", ""]
    text.extend(f"- {item}" for item in data["exclusions"])
    text += ["", "Offline AI-assisted editing and project interoperability remain in scope. Third-party plugin loading and proprietary implementations are not required; original equivalent editing behavior is the target. Excluding GUI workflows does not exclude the editing operations behind them.", "",
             "## Coverage by area", "", "| Area | Verified points | Coverage |", "| --- | ---: | ---: |"]
    text.extend(f"| {name} | {earned}/{possible} | {100 * earned / possible:.1f}% |" for name, (earned, possible) in groups.items())
    text += ["", "## Acceptance checklist", "", "A score of 1/2 means the basic checkpoint is verified; the broader checkpoint remains open.", "",
             "| ID | Capability | Points | Basic checkpoint | Broader checkpoint |", "| --- | --- | ---: | --- | --- |"] + rows
    text += ["", "## Agent interface (separate score)", "", f"**{agent_total}/{len(data['agent_checks'])} foundational checks verified.** These do not add points to editing coverage.", ""]
    for item in data["agent_checks"]:
        text.append(f"- [{'x' if item['evidence'] else ' '}] {item['name']}" + (f" — {item['acceptance']}" if item.get('acceptance') else ""))
    text += ["", "## Updating this tracker", "",
             "1. Implement a defined checkpoint and add meaningful acceptance coverage.",
             "2. Add passing check IDs to `progress/capabilities.json` only when the checkpoint's stated behavior is actually covered.",
             "3. Run `python tools/verify.py`; it runs formatting, lint, unit/crash tests, session, MCP/job and render integrations, repository checks, and regenerates this file.",
             "4. Record a change in `docs/PROGRESS_HISTORY.md`. Change the baseline ID and log old/new scope if exclusions or weights change; never silently shrink the denominator.", "",
             "The generator checks source fingerprints and evidence IDs. Human review is still required to judge whether a test demonstrates the full checkpoint. Evidence is in [verification/latest.json](../verification/latest.json); criteria are in [progress/capabilities.json](../progress/capabilities.json).", "",
             "## Scope references", "",
             "The original acceptance criteria are in [progress/capabilities.json](../progress/capabilities.json). Product-specific research and source mappings remain private under the [research boundary](RESEARCH.md); they do not establish engine compatibility or add implementation points.", ""]
    return "\n".join(text), total


def generate_research(core_score):
    data = json.loads((ROOT / "progress/research_implementation.json").read_text(encoding="utf-8"))
    report = json.loads((ROOT / "verification/latest.json").read_text(encoding="utf-8"))
    additions = data["supplemental"]
    if data["core_baseline"] != report["baseline"] or data["core_points"] != 100:
        raise ValueError("Research implementation baseline must retain the current 100 core points")
    if len(additions) != 4 or {x["id"] for x in additions} != {"X01", "X02", "X03", "X04"}:
        raise ValueError("Research implementation v1 requires four supplemental groups (eight points)")
    known = set(report["passed"])
    extra = 0
    rows = []
    for feature in additions:
        score = 0
        for key in ("basic", "extended"):
            evidence = feature.get(key + "_evidence", [])
            if set(evidence) - known:
                raise ValueError("Supplemental claim lacks passing evidence")
            score += bool(evidence)
        extra += score
        rows.append(f"| {feature['id']} | {feature['name']} | {score}/2 | {feature['basic']} | {feature['extended']} |")
    combined = core_score + extra
    text = ["# Research implementation progress", "",
            f"**Total verified implementation: {combined}/108 ({100 * combined / 108:.1f}%).**", "",
            f"Baseline: `{data['baseline']}`. Verified: {report['date']}.", "",
            f"- Core engine: **{core_score}/100 ({core_score:.1f}%)**, under the unchanged `{data['core_baseline']}` acceptance scope.",
            f"- Supplemental research-derived capabilities: **{extra}/8**.",
            "- Agent foundations are reported separately in the core tracker and add no editing points.", "",
            "The 108-point total adds eight explicitly declared checkpoints to the original core denominator; it does not award points for investigations or planning. This is verified acceptance coverage, not equal effort, full product equivalence or a delivery-time estimate. All unimplemented checkpoints remain in scope.", "",
            "See the [eight-phase roadmap](IMPLEMENTATION_ROADMAP.md), [core checklist](PROGRESS.md) and [scope history](PROGRESS_HISTORY.md).", "",
            "| ID | Capability | Points | Basic checkpoint | Broader checkpoint |",
            "| --- | --- | ---: | --- | --- |", *rows, "",
            "Generated by `tools/progress.py` after verification. Evidence comes from `verification/latest.json`; supplemental criteria are in `progress/research_implementation.json`. Exact product-specific research mappings and raw evidence remain private.", ""]
    return "\n".join(text), combined


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    content, score = generate()
    research, combined = generate_research(score)
    for name, text in [("PROGRESS.md", content), ("IMPLEMENTATION_PROGRESS.md", research)]:
        path = ROOT / "docs" / name
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                raise SystemExit("Progress document is stale; run python tools/progress.py")
        else:
            path.write_text(text, encoding="utf-8")
    print(f"Verified scoped coverage: {score:.1f}% ({score}/100)")
    print(f"Verified research implementation: {100 * combined / 108:.1f}% ({combined}/108)")
