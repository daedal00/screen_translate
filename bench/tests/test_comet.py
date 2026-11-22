import json
import re

import pytest

import run


def test_comet_key_stable_hex():
    k = run.comet_key("src", "mt", "ref")
    assert k == run.comet_key("src", "mt", "ref")
    assert re.fullmatch(r"[0-9a-f]{64}", k)
    assert k == run.comet_key("src", "mt", "ref", run.COMET_MODEL)


@pytest.mark.parametrize("other", [("s2", "mt", "ref"), ("src", "m2", "ref"), ("src", "mt", "r2")])
def test_comet_key_changes_with_each_field(other):
    assert run.comet_key("src", "mt", "ref") != run.comet_key(*other)


def test_comet_key_changes_with_model_name():
    assert run.comet_key("a", "b", "c", "model-1") != run.comet_key("a", "b", "c", "model-2")


def test_comet_key_separator_unambiguous():
    assert run.comet_key("a", "bc", "x") != run.comet_key("ab", "c", "x")
    assert run.comet_key("a", "b", "cx") != run.comet_key("a", "bc", "x")
    assert run.comet_key("a", "", "bc") != run.comet_key("a", "b", "c")


# ScoreCache
def test_cache_missing_is_none(tmp_path):
    assert run.ScoreCache(tmp_path / "c.json").get("nope") is None


def test_cache_put_many_and_get(tmp_path):
    c = run.ScoreCache(tmp_path / "c.json")
    c.put_many({"a": 0.5, "b": 0.0})
    assert c.get("a") == 0.5
    assert c.get("b") == 0.0  # zero is a real score, not "missing"


def test_cache_persists_across_instances(tmp_path):
    p = tmp_path / "sub" / "c.json"
    p.parent.mkdir()
    run.ScoreCache(p).put_many({"a": 0.25})
    assert run.ScoreCache(p).get("a") == 0.25
    run.ScoreCache(p).put_many({"b": 0.75})
    c = run.ScoreCache(p)
    assert (c.get("a"), c.get("b")) == (0.25, 0.75)


def test_cache_file_is_json_and_atomic(tmp_path):
    p = tmp_path / "c.json"
    c = run.ScoreCache(p)
    c.put_many({"a": 0.1})
    assert json.loads(p.read_text()) == {"a": 0.1}
    assert [f.name for f in tmp_path.iterdir()] == ["c.json"]  # temp file renamed away


def test_cache_creates_missing_file_lazily(tmp_path):
    p = tmp_path / "new.json"
    c = run.ScoreCache(p)
    assert c.get("x") is None


# score_with_cache
def make_predict(table):
    calls = []

    def predict(items):
        calls.append([dict(i) for i in items])
        return [table[(i["src"], i["mt"], i["ref"])] for i in items]

    predict.calls = calls
    return predict


TABLE = {
    ("s1", "m1", "r1"): 0.1,
    ("s2", "m2", "r2"): 0.2,
    ("s3", "", "r3"): 0.3,
    ("s4", "m4", "r4"): 0.4,
}


def test_score_sends_only_missing_deduped_in_order(tmp_path):
    cache = run.ScoreCache(tmp_path / "c.json")
    pre = run.comet_key("s2", "m2", "r2")
    cache.put_many({pre: 0.2})
    pred = make_predict(TABLE)
    triples = [("s1", "m1", "r1"), ("s2", "m2", "r2"), ("s1", "m1", "r1"), ("s3", "", "r3")]
    out = pred_out = run.score_with_cache(triples, cache, pred)
    assert out == [0.1, 0.2, 0.1, 0.3]
    assert len(pred.calls) == 1
    assert pred.calls[0] == [
        {"src": "s1", "mt": "m1", "ref": "r1"},
        {"src": "s3", "mt": "", "ref": "r3"},
    ]
    assert pred_out is out


def test_score_empty_mt_is_scored(tmp_path):
    pred = make_predict(TABLE)
    out = run.score_with_cache([("s3", "", "r3")], run.ScoreCache(tmp_path / "c.json"), pred)
    assert out == [0.3]
    assert pred.calls == [[{"src": "s3", "mt": "", "ref": "r3"}]]


def test_score_second_call_uses_cache_only(tmp_path):
    cache = run.ScoreCache(tmp_path / "c.json")
    triples = [("s1", "m1", "r1"), ("s3", "", "r3")]
    first = make_predict(TABLE)
    a = run.score_with_cache(triples, cache, first)
    second = make_predict(TABLE)
    b = run.score_with_cache(triples, cache, second)
    assert a == b == [0.1, 0.3]
    assert second.calls == []


def test_score_cache_persisted_for_new_instance(tmp_path):
    p = tmp_path / "c.json"
    run.score_with_cache([("s1", "m1", "r1")], run.ScoreCache(p), make_predict(TABLE))
    pred = make_predict(TABLE)
    assert run.score_with_cache([("s1", "m1", "r1")], run.ScoreCache(p), pred) == [0.1]
    assert pred.calls == []


def test_score_new_triple_only_new_sent(tmp_path):
    cache = run.ScoreCache(tmp_path / "c.json")
    run.score_with_cache([("s1", "m1", "r1")], cache, make_predict(TABLE))
    pred = make_predict(TABLE)
    out = run.score_with_cache([("s1", "m1", "r1"), ("s4", "m4", "r4")], cache, pred)
    assert out == [0.1, 0.4]
    assert pred.calls == [[{"src": "s4", "mt": "m4", "ref": "r4"}]]


def test_score_empty_input(tmp_path):
    pred = make_predict(TABLE)
    assert run.score_with_cache([], run.ScoreCache(tmp_path / "c.json"), pred) == []
    assert pred.calls == []


# disputed_items
def test_disputed_items_by_spread_desc():
    rows = {
        "a": {"x": 0.5, "y": 0.6},
        "b": {"x": 0.1, "y": 0.9, "z": 0.5},
        "c": {"x": 0.3, "y": 0.7},
    }
    assert run.disputed_items(rows, 2) == ["b", "c"]


def test_disputed_items_ties_broken_by_id():
    rows = {"z": {"x": 0.0, "y": 0.5}, "a": {"x": 0.0, "y": 0.5}, "m": {"x": 0.0, "y": 0.5}, "q": {"x": 0.4, "y": 0.4}}
    assert run.disputed_items(rows, 3) == ["a", "m", "z"]


def test_disputed_items_k_larger_than_items():
    rows = {"a": {"x": 0.0, "y": 0.2}, "b": {"x": 0.0, "y": 0.1}}
    assert run.disputed_items(rows, 50) == ["a", "b"]


def test_disputed_items_empty_and_zero():
    assert run.disputed_items({}, 5) == []
    assert run.disputed_items({"a": {"x": 0.0, "y": 1.0}}, 0) == []


# render_disputed_html
EVIL = '<script>alert("x")</script> & "q" \'s\''


def html_for(items=None, outputs=None, scores=None):
    items = items or [{"id": "i1", "src": EVIL, "ref": EVIL}]
    outputs = outputs or {"i1": {"m|app": EVIL}}
    scores = scores or {"i1": {"m|app": 0.5}}
    return run.render_disputed_html(items, outputs, scores)


def test_html_escapes_everything():
    h = html_for()
    assert "<script" not in h
    assert "&lt;script&gt;" in h
    assert "&amp;" in h
    assert "&quot;q&quot;" in h
    assert EVIL not in h


def test_html_escapes_candidate_names_and_ids():
    h = html_for(
        items=[{"id": "<i1>", "src": "s", "ref": "r"}],
        outputs={"<i1>": {"<m>|app": "o"}},
        scores={"<i1>": {"<m>|app": 0.5}},
    )
    assert "<m>" not in h and "<i1>" not in h
    assert "&lt;m&gt;" in h


def test_html_self_contained():
    h = html_for()
    assert re.search(r'<meta\s+charset="utf-8"', h, re.I)
    assert "<style" in h
    assert "<link" not in h and "src=" not in h.replace("data-src", "")


def test_html_sorted_by_comet_desc():
    items = [{"id": "i1", "src": "SRC_TEXT", "ref": "REF_TEXT"}]
    outputs = {"i1": {"low|app": "OUT_LOW", "high|app": "OUT_HIGH", "mid|app": "OUT_MID"}}
    scores = {"i1": {"low|app": 0.3, "high|app": 0.9, "mid|app": 0.6}}
    h = run.render_disputed_html(items, outputs, scores)
    assert "SRC_TEXT" in h and "REF_TEXT" in h
    assert h.index("OUT_HIGH") < h.index("OUT_MID") < h.index("OUT_LOW")


def test_html_one_table_per_item():
    items = [{"id": "i1", "src": "s1", "ref": "r1"}, {"id": "i2", "src": "s2", "ref": "r2"}]
    outputs = {"i1": {"m|app": "o1"}, "i2": {"m|app": "o2"}}
    scores = {"i1": {"m|app": 0.5}, "i2": {"m|app": 0.4}}
    h = run.render_disputed_html(items, outputs, scores)
    assert h.lower().count("<table") == 2


def test_html_no_em_dash():
    assert "—" not in html_for()
