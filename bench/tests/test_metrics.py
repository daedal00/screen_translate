import pytest
from sacrebleu.metrics import CHRF

import run


def test_chrf_identical_is_100():
    s = ["The cat sat on the mat.", "Open the file menu."]
    assert run.chrf_pp(s, s) == pytest.approx(100.0)


def test_chrf_empty_lists_zero():
    assert run.chrf_pp([], []) == 0.0


def test_chrf_matches_sacrebleu_word_order_2():
    hyps = ["The cat sat on a mat.", "Open file menu", "", "Good evening everyone"]
    refs = ["The cat sat on the mat.", "Open the file menu.", "Save the document", "Good evening, everybody"]
    expected = CHRF(word_order=2).corpus_score(hyps, [refs]).score
    assert run.chrf_pp(hyps, refs) == pytest.approx(expected)
    assert 0 < expected < 100


def test_chrf_all_empty_hyps_is_zero():
    assert run.chrf_pp(["", ""], ["Hello", "World"]) == pytest.approx(0.0)


def test_percentile_known_values():
    v = [1.0, 2.0, 3.0, 4.0, 5.0]
    assert run.percentile(v, 0) == 1.0
    assert run.percentile(v, 50) == 3.0
    assert run.percentile(v, 90) == pytest.approx(4.6)
    assert run.percentile(v, 100) == 5.0


def test_percentile_unsorted_input():
    assert run.percentile([5.0, 1.0, 3.0, 2.0, 4.0], 50) == 3.0


def test_percentile_interpolates():
    assert run.percentile([1.0, 2.0], 50) == pytest.approx(1.5)
    assert run.percentile([10.0, 20.0], 25) == pytest.approx(12.5)


def test_percentile_single_and_empty():
    assert run.percentile([7.0], 0) == 7.0
    assert run.percentile([7.0], 90) == 7.0
    assert run.percentile([7.0], 100) == 7.0
    assert run.percentile([], 50) == 0.0


# aggregate
MODELS = ["M1", "M2"]
PROMPTS = ["app", "native"]
LANGS = ["jpn", "fra"]


def rec(item_id, set_, lang, model, prompt, ref, output, lat, tps, failure=None):
    return {
        "item_id": item_id, "set": set_, "lang": lang, "model": model, "prompt": prompt,
        "src": "src " + item_id, "ref": ref, "raw": output, "output": output,
        "latency_s": lat, "tokens_per_s": tps, "failure": failure,
    }


def build_records():
    out = []
    for m in MODELS:
        for p in PROMPTS:
            for lang in LANGS:
                f_ref, s_ref = f"The {lang} cat sat on the mat", f"Open the {lang} file menu"
                out.append(rec(f"flores-{lang}-0", "flores", lang, m, p, f_ref, f_ref, 1.0, 10.0))
                kind = "chatter" if (m, lang) == ("M2", "fra") else "empty"
                out.append(rec(f"screen-{lang}-0", "screen", lang, m, p, s_ref, "", 3.0, 20.0, kind))
    out.append({"type": "model_run", "model": "M1", "load_s": 5.5, "peak_rss_mb": 4000})
    out.append({"type": "model_run", "model": "M2", "load_s": 9.25, "peak_rss_mb": 7000})
    return out


def build_comet():
    c = {}
    for lang in LANGS:
        c[f"flores-{lang}-0"] = {"M1|app": 0.8, "M2|app": 0.5, "M2|native": 0.9}
        c[f"screen-{lang}-0"] = {"M2|app": 0.7, "M2|native": 0.1}  # M1|app screen unscored, M1|native never
    return c


def find(rows, model, prompt, group, key="lang"):
    hits = [r for r in rows if r["model"] == model and r["prompt"] == prompt and r.get(key) == group]
    assert len(hits) == 1, (model, prompt, group, len(hits))
    return hits[0]


def test_aggregate_row_count_and_ignores_model_run_rows():
    rows = run.aggregate(build_records(), build_comet())
    assert len(rows) == 2 * 2 * (2 + 1)  # model x prompt x (langs + "all")
    assert all("type" not in r or r["type"] != "model_run" for r in rows)


def test_aggregate_counts_and_chrf():
    rows = run.aggregate(build_records(), build_comet())
    g = find(rows, "M1", "app", "jpn")
    assert g["n"] == 2
    hyps = ["The jpn cat sat on the mat", ""]
    refs = ["The jpn cat sat on the mat", "Open the jpn file menu"]
    assert g["chrf"] == pytest.approx(run.chrf_pp(hyps, refs))
    a = find(rows, "M1", "app", "all")
    assert a["n"] == 4
    all_h = ["The jpn cat sat on the mat", "", "The fra cat sat on the mat", ""]
    all_r = ["The jpn cat sat on the mat", "Open the jpn file menu", "The fra cat sat on the mat", "Open the fra file menu"]
    assert a["chrf"] == pytest.approx(run.chrf_pp(all_h, all_r))


def test_aggregate_latency_tps():
    rows = run.aggregate(build_records(), build_comet())
    g = find(rows, "M1", "app", "jpn")
    assert g["latency_mean"] == pytest.approx(2.0)
    assert g["latency_p90"] == pytest.approx(2.8)
    assert g["tokens_per_s"] == pytest.approx(15.0)
    a = find(rows, "M1", "app", "all")
    assert a["latency_mean"] == pytest.approx(2.0)
    assert a["latency_p90"] == pytest.approx(run.percentile([1.0, 3.0, 1.0, 3.0], 90))


def test_aggregate_failures():
    rows = run.aggregate(build_records(), build_comet())
    g = find(rows, "M1", "app", "jpn")
    assert g["failures"] == 1 and g["failure_kinds"] == {"empty": 1}
    a = find(rows, "M1", "app", "all")
    assert a["failures"] == 2 and a["failure_kinds"] == {"empty": 2}
    m2 = find(rows, "M2", "app", "all")
    assert m2["failures"] == 2 and m2["failure_kinds"] == {"empty": 1, "chatter": 1}


def test_aggregate_failed_items_still_count_with_empty_output():
    rows = run.aggregate(build_records(), build_comet())
    g = find(rows, "M1", "app", "jpn")
    assert g["n"] == 2 and g["chrf"] < 100


def test_aggregate_comet_means_x100_ignoring_unscored():
    rows = run.aggregate(build_records(), build_comet())
    assert find(rows, "M1", "app", "jpn")["comet"] == pytest.approx(80.0)  # screen item unscored
    assert find(rows, "M2", "app", "jpn")["comet"] == pytest.approx(60.0)
    assert find(rows, "M2", "native", "fra")["comet"] == pytest.approx(50.0)
    assert find(rows, "M1", "app", "all")["comet"] == pytest.approx(80.0)
    assert find(rows, "M2", "app", "all")["comet"] == pytest.approx(60.0)


def test_aggregate_comet_none_when_nothing_scored():
    rows = run.aggregate(build_records(), build_comet())
    for lang in (*LANGS, "all"):
        assert find(rows, "M1", "native", lang)["comet"] is None


def test_aggregate_comet_all_none_when_comet_dict_empty():
    rows = run.aggregate(build_records(), {})
    assert all(r["comet"] is None for r in rows)


def test_aggregate_carries_model_run_data():
    rows = run.aggregate(build_records(), build_comet())
    for r in rows:
        want = (5.5, 4000) if r["model"] == "M1" else (9.25, 7000)
        assert (r["load_s"], r["peak_rss_mb"]) == want


def test_aggregate_by_set():
    rows = run.aggregate(build_records(), build_comet(), by="set")
    assert len(rows) == 2 * 2 * 3
    fl = find(rows, "M1", "app", "flores", key="set")
    sc = find(rows, "M1", "app", "screen", key="set")
    al = find(rows, "M1", "app", "all", key="set")
    assert (fl["n"], sc["n"], al["n"]) == (2, 2, 4)
    assert fl["comet"] == pytest.approx(80.0)
    assert sc["comet"] is None
    assert fl["chrf"] == pytest.approx(100.0)
    assert sc["chrf"] == pytest.approx(0.0)
    assert fl["failures"] == 0 and sc["failures"] == 2
    assert find(rows, "M2", "app", "screen", key="set")["comet"] == pytest.approx(70.0)
    assert fl["load_s"] == 5.5
