import re

import pytest

import run


def rust_system_template(src: str) -> str:
    seg = src.split("let system = format!(", 1)[1].split("language_name(target)", 1)[0]
    lit = re.search(r'"((?:[^"\\]|\\.)*)"', seg, re.S).group(1)
    lit = re.sub(r"\\\n\s*", "", lit)  # rust line continuation
    return lit.replace('\\"', '"').replace("\\\\", "\\")


def rust_language_table(src: str) -> dict[str, str]:
    body = src.split("pub fn language_name", 1)[1].split("\n}\n", 1)[0]
    return dict(re.findall(r'"([A-Z-]+)"\s*=>\s*"([^"]+)"', body))


def test_language_table_matches_local_rs(local_rs):
    table = rust_language_table(local_rs)
    assert table, "failed to parse language_name from local.rs"
    assert run.LANGUAGE_NAMES == table


def test_language_name_default_english(local_rs):
    for code, name in rust_language_table(local_rs).items():
        assert run.language_name(code) == name
    assert run.language_name("EN-US") == "English"
    assert run.language_name("xx") == "English"


def test_system_prompt_matches_local_rs(local_rs):
    template = rust_system_template(local_rs)
    assert template.count("{}") == 2
    targets = [*rust_language_table(local_rs), "EN-US"]
    for t in targets:
        name = run.language_name(t)
        assert run.app_system_prompt(t) == template.replace("{}", name)


def test_user_wrapper_matches_local_rs(local_rs):
    assert 'format!("<text>\\n{text}\\n</text>")' in local_rs
    body = run.app_body("Hello there", "DE")
    assert body["messages"][1] == {"role": "user", "content": "<text>\nHello there\n</text>"}


def test_app_body_shape(local_rs):
    for t in [*rust_language_table(local_rs), "EN-US"]:
        body = run.app_body("x", t)
        assert body == {
            "messages": [
                {"role": "system", "content": run.app_system_prompt(t)},
                {"role": "user", "content": "<text>\nx\n</text>"},
            ],
            "temperature": 0,
            "stream": False,
        }
        assert "model" not in body


def test_app_body_temperature_is_int_zero_and_stream_false():
    body = run.app_body("x", "EN-US")
    assert body["temperature"] == 0 and body["stream"] is False


# clean: ported from src-tauri/tests/local.rs clean_* tests
CLEAN_CASES = [
    ("think block", "hi", "<think>reasoning</think>\n\n Hallo", "Hallo"),
    ("unterminated think", "hi", "<think>unterminated", ""),
    ("label Translation:", "hi", "Translation: Hallo", "Hallo"),
    ("label translation:", "hi", "translation: Hallo", "Hallo"),
    ("label here is", "hi", "Here is the translation:\nHallo", "Hallo"),
    ("label kept when source has it", "Translation: hello", "Translation: Hallo", "Translation: Hallo"),
    ("strip ascii quotes", "hi", '"Hallo"', "Hallo"),
    ("strip curly quotes", "hi", "“Hallo”", "Hallo"),
    ("strip corner quotes", "hi", "「こんにちは」", "こんにちは"),
    ("keep quotes source ascii", '"hi"', '"Hallo"', '"Hallo"'),
    (
        "keep quotes source corner",
        "「hi」",
        "「こんにちは」",
        "「こんにちは」",
    ),
    (
        "keep quotes source wrapped by any pair",
        "「こんにちは」と言った。「さようなら」",
        '"Hello," he said. "Goodbye."',
        '"Hello," he said. "Goodbye."',
    ),
    ("keep quotes source other pair", "「こんにちは」", '"Hello"', '"Hello"'),
    ("keep inner quotes", "こんにちは", '"A" and "B"', '"A" and "B"'),
    ("strip simple quotes plain source", "こんにちは", '"Hello"', "Hello"),
    ("passthrough", "hi", "Hallo", "Hallo"),
    ("trim", "hi", "  Hallo \n", "Hallo"),
]


@pytest.mark.parametrize("name,source,output,expected", CLEAN_CASES, ids=[c[0] for c in CLEAN_CASES])
def test_clean(name, source, output, expected):
    assert run.clean(source, output) == expected


# native prompts
def test_native_hymt2_exact():
    assert run.native_body("hymt2", "こんにちは", "jpn") == {
        "messages": [
            {
                "role": "user",
                "content": "Translate the following text into English. Note that you should only output the translated result without any additional explanation:\n\nこんにちは",
            }
        ],
        "temperature": 0,
        "stream": False,
    }


def test_native_hunyuan_exact():
    assert run.native_body("hunyuan-mt", "Bonjour", "fra") == {
        "messages": [
            {
                "role": "user",
                "content": "Translate the following segment into English, without additional explanation.\n\nBonjour",
            }
        ],
        "temperature": 0,
        "stream": False,
    }


@pytest.mark.parametrize("src,code", [("jpn", "ja"), ("kor", "ko"), ("fra", "fr"), ("deu", "de"), ("spa", "es")])
def test_native_translategemma_raw_text_plus_template_kwargs(src, code):
    text = "Some source text\nwith two lines"
    body = run.native_body("translategemma", text, src)
    assert body == {
        "messages": [{"role": "user", "content": text}],
        "temperature": 0,
        "stream": False,
        "chat_template_kwargs": {"source_lang_code": code, "target_lang_code": "en"},
    }


def test_native_translategemma_content_not_stripped_or_wrapped():
    assert run.native_body("translategemma", "  x \n", "jpn")["messages"][0]["content"] == "  x \n"


def test_translategemma_user_constant_removed():
    assert not hasattr(run, "TRANSLATEGEMMA_USER")


def test_other_native_styles_have_no_template_kwargs():
    for style in ("hymt2", "hunyuan-mt"):
        assert "chat_template_kwargs" not in run.native_body(style, "x", "jpn")


def test_native_no_system_message():
    for style in ("hymt2", "hunyuan-mt", "translategemma"):
        roles = [m["role"] for m in run.native_body(style, "x", "jpn")["messages"]]
        assert roles == ["user"]


def test_native_unknown_style_raises():
    with pytest.raises(ValueError):
        run.native_body("nope", "x", "jpn")


def test_src_names_and_codes():
    assert run.SRC_NAMES == {"jpn": "Japanese", "kor": "Korean", "fra": "French", "deu": "German", "spa": "Spanish"}
    assert run.SRC_CODES == {"jpn": "ja", "kor": "ko", "fra": "fr", "deu": "de", "spa": "es"}


def test_importing_run_does_not_need_comet_or_anthropic():
    import sys

    assert "comet" not in sys.modules
    assert "anthropic" not in sys.modules
