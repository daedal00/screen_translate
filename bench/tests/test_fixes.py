"""Regression tests for the review findings (bench-review-report.md)."""
import socket
import sys

import pytest

import run


# 5. weights fit
@pytest.mark.parametrize(
    "size,want",
    [(1_000_000_000, "yes"), (5.5e9, "yes"), (5.5e9 + 1, "tight"), (6.5e9, "tight"), (6.5e9 + 1, "no"), (9e9, "no")],
)
def test_weights_fit_8gb(size, want):
    assert run.weights_fit_8gb(int(size)) == want


def test_report_header_says_weights_fit():
    row = {
        "model": "alpha", "prompt": "app", "lang": "all", "n": 1, "chrf": 1.0, "comet": 50.0, "latency_mean": 1.0,
        "latency_p90": 1.0, "tokens_per_s": 1.0, "failures": 0, "failure_kinds": {}, "load_s": 1.0, "peak_rss_mb": 1,
    }
    rep = run.render_report([row], [], {"sizes": {"alpha": 4_000_000_000}})
    assert "Weights fit 8 GB" in rep


# 6. rows
def r(item, model, prompt, failure=None, output="x", **kw):
    return {"item_id": item, "model": model, "prompt": prompt, "failure": failure, "output": output, **kw}


def test_done_keys_excludes_request_error():
    rows = [r("a", "m", "app"), r("b", "m", "app", "request_error"), r("c", "m", "native", "empty")]
    assert run.done_keys(rows) == {("a", "m", "app"), ("c", "m", "native")}


def test_done_keys_ignores_model_run_rows():
    rows = [r("a", "m", "app"), {"type": "model_run", "model": "m", "load_s": 1.0, "peak_rss_mb": 5}]
    assert run.done_keys(rows) == {("a", "m", "app")}


def test_dedupe_rows_keeps_last():
    rows = [r("a", "m", "app", "request_error", output="old"), r("b", "m", "app"), r("a", "m", "app", output="new")]
    out = run.dedupe_rows(rows)
    assert len(out) == 2
    a = [x for x in out if x["item_id"] == "a"][0]
    assert a["output"] == "new"


def test_dedupe_rows_keeps_all_model_run_rows():
    runs = [{"type": "model_run", "model": "m", "load_s": i, "peak_rss_mb": i} for i in range(3)]
    out = run.dedupe_rows(runs + [r("a", "m", "app")])
    assert [x for x in out if x.get("type") == "model_run"] == runs


def test_dedupe_distinguishes_model_and_prompt():
    rows = [r("a", "m", "app"), r("a", "m", "native"), r("a", "n", "app")]
    assert len(run.dedupe_rows(rows)) == 3


def _rec(model="M"):
    return {
        "item_id": "i1", "set": "flores", "lang": "jpn", "model": model, "prompt": "app", "src": "s", "ref": "ref",
        "raw": "ref", "output": "ref", "latency_s": 1.0, "tokens_per_s": 1.0, "failure": None,
    }


def test_aggregate_load_last_peak_max():
    recs = [
        _rec(),
        {"type": "model_run", "model": "M", "load_s": 9.0, "peak_rss_mb": 7000},
        {"type": "model_run", "model": "M", "load_s": 2.0, "peak_rss_mb": 3000},
    ]
    row = [x for x in run.aggregate(recs, {}) if x["lang"] == "all"][0]
    assert row["load_s"] == 2.0
    assert row["peak_rss_mb"] == 7000


def test_aggregate_peak_max_is_per_model():
    recs = [
        _rec("A"), _rec("B"),
        {"type": "model_run", "model": "A", "load_s": 1.0, "peak_rss_mb": 100},
        {"type": "model_run", "model": "B", "load_s": 2.0, "peak_rss_mb": 50},
        {"type": "model_run", "model": "B", "load_s": 3.0, "peak_rss_mb": 40},
    ]
    rows = {x["model"]: x for x in run.aggregate(recs, {}) if x["lang"] == "all"}
    assert (rows["A"]["load_s"], rows["A"]["peak_rss_mb"]) == (1.0, 100)
    assert (rows["B"]["load_s"], rows["B"]["peak_rss_mb"]) == (3.0, 50)


# 7. port_free
def test_port_free_false_when_bound():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    s.listen()
    try:
        assert run.port_free(s.getsockname()[1]) is False
    finally:
        s.close()


def test_port_free_true_when_free_and_releases():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    assert run.port_free(port) is True
    assert run.port_free(port) is True  # probe did not leave the port bound


# 8. comet_triples
def test_comet_triples_prefers_row_fields():
    rows = [{"item_id": "a", "src": "rs", "ref": "rr", "output": "o"}]
    assert run.comet_triples(rows, {"a": {"src": "cs", "ref": "cr"}}) == [("rs", "o", "rr")]


def test_comet_triples_falls_back_to_items():
    rows = [{"item_id": "a", "output": "o"}]
    assert run.comet_triples(rows, {"a": {"src": "cs", "ref": "cr"}}) == [("cs", "o", "cr")]


def test_comet_triples_unknown_id_without_fields_skipped():
    rows = [{"item_id": "gone", "output": "o"}, {"item_id": "a", "src": "s", "ref": "r", "output": "p"}]
    assert run.comet_triples(rows, {}) == [("s", "p", "r")]


def test_comet_triples_skips_model_run_rows_and_keeps_order():
    rows = [
        {"type": "model_run", "model": "m", "load_s": 1, "peak_rss_mb": 1},
        {"item_id": "a", "src": "s1", "ref": "r1", "output": "o1"},
        {"item_id": "b", "src": "s2", "ref": "r2", "output": ""},
    ]
    assert run.comet_triples(rows, {}) == [("s1", "o1", "r1"), ("s2", "", "r2")]


# 9. markdown cell escaping
def test_cell_escapes_html_pipe_newline():
    out = run._cell("<text>a|b\nc</text>")
    assert "<" not in out and ">" not in out
    assert "&lt;text&gt;" in out and "&lt;/text&gt;" in out
    assert "a\\|b" in out
    assert "\n" not in out


def test_report_escapes_example_output():
    ex = [{"id": "i", "src": "s", "ref": "r", "outputs": {"alpha|app": "<text>\nHi\n</text>"}}]
    row = {
        "model": "alpha", "prompt": "app", "lang": "all", "n": 1, "chrf": 1.0, "comet": 50.0, "latency_mean": 1.0,
        "latency_p90": 1.0, "tokens_per_s": 1.0, "failures": 0, "failure_kinds": {}, "load_s": 1.0, "peak_rss_mb": 1,
    }
    rep = run.render_report([row], ex, {})
    assert "<text>" not in rep and "</text>" not in rep
    assert "&lt;text&gt;" in rep


# 10. lazy comet
def test_comet_predict_factory_does_not_import_comet(monkeypatch):
    monkeypatch.setitem(sys.modules, "comet", None)  # any `import comet` now raises ImportError
    predict = run.comet_predict_factory()
    assert callable(predict)
    with pytest.raises(ImportError):
        predict([{"src": "s", "mt": "m", "ref": "r"}])


def test_score_with_cache_never_calls_predict_when_all_cached(tmp_path):
    cache = run.ScoreCache(tmp_path / "c.json")
    cache.put_many({run.comet_key("s", "m", "r"): 0.5})

    def boom(_):
        raise AssertionError("predict called")

    assert run.score_with_cache([("s", "m", "r")], cache, boom) == [0.5]


# 11. download
@pytest.mark.parametrize("bad", [None, ""])
def test_require_oid_rejects_missing(bad):
    with pytest.raises(ValueError):
        run.require_oid(bad)


def test_require_oid_passes_through():
    oid = "a" * 64
    assert run.require_oid(oid) == oid


def test_bench_part_suffix():
    assert run.BENCH_PART_SUFFIX == ".bench-part"
