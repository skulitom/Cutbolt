"""Wall-clock budgets of acceptance fixtures.

Thorough verification enforces every budget on a quiet machine. Quick verification runs fixtures
concurrently, where elapsed time measures the machine as much as the engine, so it sets
CUTBOLT_BUDGETS=record: budgets are reported but not enforced. Correctness and memory checks are
ordinary assertions and are never relaxed.
"""
import os
import sys

ENFORCE = os.environ.get("CUTBOLT_BUDGETS", "enforce") != "record"


def check(within, detail):
    if within:
        return
    if ENFORCE:
        raise AssertionError(detail)
    print(f"CUTBOLT_BUDGET_MISS {detail}", file=sys.stderr, flush=True)
