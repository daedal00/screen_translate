import dataclasses
import re

import pytest

import run

SPEC = {
    "qwen2.5-3b": ("bartowski/Qwen2.5-3B-Instruct-GGUF", "Qwen2.5-3B-Instruct-Q4_K_M.gguf", None),
    "qwen2.5-7b": ("bartowski/Qwen2.5-7B-Instruct-GGUF", "Qwen2.5-7B-Instruct-Q4_K_M.gguf", None),
    "hy-mt2-1.8b": ("tencent/Hy-MT2-1.8B-GGUF", "Hy-MT2-1.8B-Q4_K_M.gguf", "hymt2"),
    "hy-mt2-7b": ("tencent/Hy-MT2-7B-GGUF", "Hy-MT2-7B-Q4_K_M.gguf", "hymt2"),
    "hunyuan-mt-7b": ("mradermacher/Hunyuan-MT-7B-GGUF", "Hunyuan-MT-7B.Q4_K_M.gguf", "hunyuan-mt"),
    "translategemma-4b": ("mradermacher/translategemma-4b-it-GGUF", "translategemma-4b-it.Q4_K_M.gguf", "translategemma"),
    "translategemma-12b": ("mradermacher/translategemma-12b-it-GGUF", "translategemma-12b-it.Q4_K_M.gguf", "translategemma"),
    "qwen3-8b": ("Qwen/Qwen3-8B-GGUF", "Qwen3-8B-Q4_K_M.gguf", None),
    "gemma-3-12b": ("unsloth/gemma-3-12b-it-GGUF", "gemma-3-12b-it-Q4_K_M.gguf", None),
    "glm-4-9b": ("bartowski/THUDM_GLM-4-9B-0414-GGUF", "THUDM_GLM-4-9B-0414-Q4_K_M.gguf", None),
    "gemma-4-12b": ("google/gemma-4-12B-it-qat-q4_0-gguf", "gemma-4-12b-it-qat-q4_0.gguf", None),
    "qwen3.5-9b": ("unsloth/Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf", None),
}
BY_KEY = lambda: {c.key: c for c in run.CANDIDATES}  # noqa: E731


def test_keys_present_and_unique():
    keys = [c.key for c in run.CANDIDATES]
    assert len(keys) == len(set(keys))
    assert set(SPEC) <= set(keys)


def test_repo_file_native_match_spec():
    by = BY_KEY()
    for key, (repo, file, native) in SPEC.items():
        c = by[key]
        assert (c.repo, c.file, c.native) == (repo, file, native), key


def test_files_are_gguf_and_unique():
    files = [c.file for c in run.CANDIDATES]
    assert all(f.endswith(".gguf") for f in files)
    assert len(files) == len(set(files))


def test_fields_sane():
    for c in run.CANDIDATES:
        assert isinstance(c.size_bytes, int) and c.size_bytes > 100_000_000, c.key
        assert c.label and c.quant
        assert isinstance(c.server_args, tuple)
        assert c.kind in ("baseline", "mt", "general")


def test_candidate_is_frozen():
    with pytest.raises(dataclasses.FrozenInstanceError):
        run.CANDIDATES[0].key = "x"


def test_baselines_equal_app_catalog(models_rs):
    entries = re.findall(
        r'id:\s*"([^"]+)".*?source:\s*"([^":]+):([^"]+)".*?size:\s*([\d_]+)', models_rs.split("CATALOG", 1)[1], re.S
    )
    assert len(entries) >= 2
    by = BY_KEY()
    for mid, repo, quant, size in entries:
        c = by[mid]
        assert c.repo == repo
        assert c.quant == quant
        assert c.kind == "baseline"
        assert c.size_bytes == int(size.replace("_", ""))
        assert c.file == SPEC[mid][1]


def test_native_only_on_mt_models():
    for c in run.CANDIDATES:
        assert (c.native is not None) == (c.kind == "mt"), c.key
    mt = {c.key for c in run.CANDIDATES if c.kind == "mt"}
    assert mt == {"hy-mt2-1.8b", "hy-mt2-7b", "hunyuan-mt-7b", "translategemma-4b", "translategemma-12b"}


def test_kinds():
    by = BY_KEY()
    assert by["qwen2.5-3b"].kind == by["qwen2.5-7b"].kind == "baseline"
    for k in ("qwen3-8b", "gemma-3-12b", "glm-4-9b", "gemma-4-12b", "qwen3.5-9b"):
        assert by[k].kind == "general"


def test_reasoning_flag_on_thinking_models_only():
    for c in run.CANDIDATES:
        if c.key in ("qwen3-8b", "qwen3.5-9b", "gemma-4-12b"):
            assert c.server_args == ("--reasoning-budget", "0")
        else:
            assert c.server_args == (), c.key


def test_notes_and_quant():
    by = BY_KEY()
    assert "community" in by["hunyuan-mt-7b"].note
    assert "gated" in by["translategemma-4b"].note
    assert "tight" in by["translategemma-12b"].note
    assert "tight" in by["gemma-3-12b"].note
    assert by["gemma-4-12b"].quant == "QAT Q4_0"
    for k, c in BY_KEY().items():
        if k != "gemma-4-12b":
            assert c.quant == "Q4_K_M", k


def test_server_args_for_default():
    c = BY_KEY()["qwen2.5-7b"]
    assert run.server_args_for(c, "/m/x.gguf", 18190) == [
        "-m", "/m/x.gguf", "--host", "127.0.0.1", "--port", "18190", "-c", "4096",
    ]


def test_server_args_for_includes_extras():
    c = BY_KEY()["qwen3-8b"]
    assert run.server_args_for(c, "/m/q.gguf", 19000) == [
        "-m", "/m/q.gguf", "--host", "127.0.0.1", "--port", "19000", "-c", "4096", "--reasoning-budget", "0",
    ]


def test_default_port():
    assert run.DEFAULT_PORT == 18190
    run.check_port(run.DEFAULT_PORT)


@pytest.mark.parametrize("port", [18080, 1420])
def test_check_port_rejects_app_ports(port):
    with pytest.raises(ValueError):
        run.check_port(port)


@pytest.mark.parametrize("port", [18190, 18191, 9000])
def test_check_port_accepts_others(port):
    run.check_port(port)
