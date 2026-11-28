"""Shared helpers for the bench tests (offline, no network, no comet/anthropic).

Contracts the spec leaves open, assumed by these tests (implementer: follow or tell the tester):
- `aggregate` records carry `ref` (and `src`) next to `output`; chrF++ is `chrf_pp` over the group's
  outputs vs refs (corpus level, also for the "all" lang row).
- `aggregate(..., by="set")` rows put the group value ("flores"/"screen"/"all") under key "set"
  (rows from by="lang" use key "lang"). Both carry model, prompt, n, chrf, comet, ...
- `render_report(rows, examples, meta)`: rows = aggregate(by="lang") rows (incl. lang "all") plus
  optionally aggregate(by="set") rows (they have a "set" key and no "lang" key); examples is a list of
  `{"id", "src", "ref", "outputs": {candidate_id: text}}`; meta keys: "date", "llama_cpp", "comet_model",
  "comet_status", "skipped" (dict key -> reason), "sizes" (dict model key -> bytes). Missing meta keys
  must not crash.
"""
import re
import sys
from pathlib import Path

import pytest

BENCH = Path(__file__).resolve().parents[1]
REPO = BENCH.parent
sys.path.insert(0, str(BENCH))

LOCAL_RS = REPO / "crates/core/src/translate/local.rs"
MODELS_RS = REPO / "crates/core/src/manager/models.rs"


@pytest.fixture(scope="session")
def local_rs() -> str:
    return LOCAL_RS.read_text(encoding="utf-8")


@pytest.fixture(scope="session")
def models_rs() -> str:
    return MODELS_RS.read_text(encoding="utf-8")
