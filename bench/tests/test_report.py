import pytest

import run
from test_metrics import build_comet, build_records

META = {
    "date": "2026-11-21",
    "llama_cpp": "version: 1234 (abcdef)",
    "comet_model": "Unbabel/wmt22-comet-da",
    "comet_status": "scored",
    "skipped": {"skipped-model-x": "repo not found on Hugging Face"},
    "sizes": {"M1": 2_000_000_000, "M2": 5_000_000_000},
}


def row(model, prompt, comet, chrf, lang="all"):
    return {
        "model": model, "prompt": prompt, "lang": lang, "n": 10, "chrf": chrf, "comet": comet,
        "latency_mean": 1.5, "latency_p90": 2.5, "tokens_per_s": 30.0, "failures": 0, "failure_kinds": {},
        "load_s": 4.0, "peak_rss_mb": 3000,
    }


def table_order(report, names):
    lines = [ln for ln in report.splitlines() if ln.lstrip().startswith("|")]
    first = {}
    for n in names:
        for i, ln in enumerate(lines):
            if n in ln:
                first[n] = i
                break
        assert n in first, f"{n} not in any table row"
    return sorted(names, key=lambda n: first[n])


def test_report_header_and_notes():
    rep = run.render_report([row("alpha", "app", 80.0, 50.0)], [], META)
    assert any(ln.lstrip().startswith("|") and "Model" in ln and "COMET" in ln and "chrF++" in ln for ln in rep.splitlines())
    assert "M1 Pro" in rep and "RTX 3070" in rep
    assert "2026-11-21" in rep
    assert "Unbabel/wmt22-comet-da" in rep


def test_report_ranked_by_comet_desc():
    rows = [
        row("alpha", "app", 60.0, 90.0),
        row("bravo", "app", 85.0, 10.0),
        row("charlie", "app", 70.0, 50.0),
    ]
    rep = run.render_report(rows, [], META)
    assert table_order(rep, ["alpha", "bravo", "charlie"]) == ["bravo", "charlie", "alpha"]


def test_report_falls_back_to_chrf_when_comet_none():
    rows = [
        row("alpha", "app", None, 30.0),
        row("bravo", "app", None, 55.0),
        row("charlie", "app", None, 42.0),
    ]
    rep = run.render_report(rows, [], {**META, "comet_status": "skipped (--no-comet)"})
    assert table_order(rep, ["alpha", "bravo", "charlie"]) == ["bravo", "charlie", "alpha"]


def test_report_skipped_candidates_listed():
    rep = run.render_report([row("alpha", "app", 80.0, 50.0)], [], META)
    assert "skipped-model-x" in rep
    assert "repo not found on Hugging Face" in rep


def test_report_no_skipped_ok():
    run.render_report([row("alpha", "app", 80.0, 50.0)], [], {**META, "skipped": {}})


def test_report_no_em_dash():
    rows = [row("alpha", "app", 80.0, 50.0), row("alpha", "native", None, 40.0)]
    examples = [{"id": "i1", "src": "s", "ref": "r", "outputs": {"alpha|app": "out"}}]
    assert "—" not in run.render_report(rows, examples, META)


def test_report_includes_example_text():
    examples = [{"id": "flores-jpn-0", "src": "SRC_SENTENCE", "ref": "REF_SENTENCE", "outputs": {"alpha|app": "OUT_SENTENCE"}}]
    rep = run.render_report([row("alpha", "app", 80.0, 50.0)], examples, META)
    for s in ("SRC_SENTENCE", "REF_SENTENCE", "OUT_SENTENCE"):
        assert s in rep


def test_report_from_real_aggregate_output():
    rows = run.aggregate(build_records(), build_comet()) + run.aggregate(build_records(), build_comet(), by="set")
    rep = run.render_report(rows, [], {**META, "sizes": {}})
    assert "M1" in rep and "M2" in rep
    assert "—" not in rep
