import pytest

import run

JA = "こんにちは世界"
KO = "안녕하세요"


def cf(lang, src, raw, cleaned=None):
    return run.classify_failure(lang, src, raw, raw if cleaned is None else cleaned)


# empty
def test_empty():
    assert cf("jpn", JA, "", "") == "empty"
    assert cf("jpn", JA, "  \n", "  \n") == "empty"
    assert cf("jpn", JA, "<think>x</think>", "") == "empty"


def test_not_empty():
    assert cf("fra", "Bonjour", "Hello", "Hello") is None


# refusal (must START with the phrase and mention the request)
@pytest.mark.parametrize(
    "text",
    [
        "I can't assist with that.",
        "I cannot translate this.",
        "I'm sorry, but I can't assist with that request.",
        "As an AI, I can't provide this content.",
        "I am unable to translate this text.",
        "I'm unable to process this request.",
    ],
)
def test_refusal_phrases(text):
    assert cf("fra", "Bonjour", text) == "refusal"


def test_refusal_case_insensitive():
    assert cf("fra", "Bonjour", "I CANNOT TRANSLATE this.") == "refusal"


def test_refusal_requires_request_mention():
    assert cf("fra", "Bonjour", "I'm sorry, but the login session has expired.") is None
    assert cf("fra", "Bonjour", "I can't wait to see you.") is None


def test_refusal_must_start_the_output():
    assert cf("fra", "Bonjour", "He said I can't translate this content.") is None


def test_refusal_sorry_translate_content():
    assert cf("fra", "Bonjour", "I'm sorry, I can't translate this content.") == "refusal"


def test_refusal_negative():
    assert cf("fra", "Bonjour", "The sorted list is ready.") is None


# chatter
@pytest.mark.parametrize(
    "raw",
    [
        "Here is the translation: Good morning",
        "Here's the translation:\nGood morning",
        "Translation: Good morning",
        "Certainly! Good morning",
        "Sure! Good morning",
        "The translation is: Good morning",
        "Note: formal tone.\nGood morning",
    ],
)
def test_chatter_first_line(raw):
    assert cf("fra", "Bonjour", raw, "Good morning") == "chatter"


@pytest.mark.parametrize("last", ["Note: this is formal.", "(Note: this is formal.)", "note: lowercase"])
def test_chatter_trailing_note(last):
    assert cf("fra", "Bonjour", f"Good morning\n\n{last}", "Good morning") == "chatter"


def test_chatter_detected_from_raw_even_when_cleaned_is_fine():
    assert cf("fra", "Bonjour", "Translation: Good morning", "Good morning") == "chatter"


def test_chatter_negative():
    assert cf("fra", "Bonjour", "Good morning", "Good morning") is None
    assert cf("fra", "Bonjour", "Good morning\nHave a nice day", "Good morning\nHave a nice day") is None


# wrong_language: jpn/kor
def test_wrong_language_japanese_output():
    assert cf("jpn", "Hello", JA) == "wrong_language"


def test_wrong_language_korean_output():
    assert cf("kor", "Hello", KO) == "wrong_language"


def test_cjk_threshold():
    # 3 CJK of 10 non-space chars = 30% -> wrong
    assert cf("jpn", "Hello", "Hello 日本語ab") == "wrong_language"
    # 2 of 10 = 20%: not more than 20% -> ok
    assert cf("jpn", "Hello", "Hellowor日本") is None
    # 1 of 9 -> ok
    assert cf("jpn", "Hello", "Hello wor日") is None


def test_cjk_spaces_ignored_in_denominator():
    # 3 CJK + 7 latin non-space chars, lots of spaces: still 30%
    assert cf("jpn", "Hello", "H e l l o  w o 日 本 語") == "wrong_language"


def test_japanese_plain_english_ok():
    assert cf("jpn", JA, "Hello world") is None
    assert cf("kor", KO, "Hello, how are you?") is None


def test_hangul_counts_as_cjk_for_japanese_rule():
    assert cf("jpn", "Hello", KO) == "wrong_language"


# wrong_language: fra/deu/spa stopwords
def test_french_output_detected():
    assert cf("fra", "Bonjour", "Le chat est sur la table dans la maison") == "wrong_language"


def test_german_output_detected():
    assert cf("deu", "Hallo", "Der Hund ist in dem Haus und die Katze") == "wrong_language"


def test_spanish_output_detected():
    assert cf("spa", "Hola", "El gato está en la casa de mi amigo") == "wrong_language"


@pytest.mark.parametrize("lang", ["fra", "deu", "spa"])
def test_english_output_ok(lang):
    assert cf(lang, "Quelque chose", "The cat is on the table in the house") is None


def test_stopword_rule_needs_four_words():
    assert cf("fra", "Bonjour", "Le chat est") is None


def test_more_english_than_source_stopwords_ok():
    # english: the, is, on, the (4) vs french: de, la (2)
    assert cf("fra", "Bonjour", "The cat is on the table de la") is None


def test_stopword_rule_not_applied_to_cjk_sources():
    assert cf("jpn", JA, "Le chat est sur la table dans la maison") is None


# untranslated echo
def test_echo_non_ascii_source():
    assert cf("fra", "café", "café") == "wrong_language"
    assert cf("deu", "Menü", " Menü ", "Menü") == "wrong_language"
    assert cf("jpn", JA, JA) == "wrong_language"


def test_echo_ignores_source_whitespace():
    assert cf("fra", "  café\n", "café") == "wrong_language"


def test_echo_ascii_source_is_not_flagged():
    assert cf("fra", "OK", "OK") is None
    assert cf("spa", "Menu 123", "Menu 123") is None


# priority
def test_priority_empty_over_chatter_and_refusal():
    assert cf("fra", "Bonjour", "Sure, I'm sorry I can't translate", "") == "empty"


def test_priority_refusal_over_chatter():
    cleaned = "I'm sorry, I can't translate this content."
    assert cf("fra", "Bonjour", "Sure! " + cleaned, cleaned) == "refusal"


def test_priority_chatter_over_wrong_language():
    assert cf("jpn", "Hello", "Translation: " + JA, JA) == "chatter"


def test_priority_refusal_over_wrong_language():
    src = "Bonjour tout le monde, comment ça va aujourd'hui ?"
    assert cf("fra", src, "I cannot", "I cannot") == "refusal"


# regression: references must never be flagged
def _data_rows():
    import json
    from pathlib import Path

    d = Path(__file__).resolve().parents[1] / "data"
    out = []
    for name in ("flores_devtest.jsonl", "screen.jsonl"):
        out += [json.loads(ln) for ln in (d / name).read_text(encoding="utf-8").splitlines() if ln.strip()]
    return out


def test_references_are_never_failures():
    rows = _data_rows()
    assert len(rows) > 300
    bad = [r["id"] for r in rows if run.classify_failure(r["lang"], r["src"], r["ref"], run.clean(r["src"], r["ref"])) is not None]
    assert bad == []


# prompt_echo
@pytest.mark.parametrize(
    "out",
    [
        "<text>\nHello\n</text>",
        "Hello </text>",
        "You are a translation engine. Hello",
        "Translate the following text into English. Hello",
        "TRANSLATION ENGINE: Hello",
    ],
)
def test_prompt_echo_positive(out):
    assert cf("fra", "Bonjour", out) == "prompt_echo"


def test_prompt_echo_negative_when_source_has_it():
    assert cf("fra", "Voir <text> ici", "See <text> here") is None
    assert cf("fra", "Translate the following, please", "Translate the following, please") != "prompt_echo"


def test_prompt_echo_negative_plain():
    assert cf("fra", "Bonjour", "Hello there") is None


def test_prompt_echo_after_empty_before_refusal_and_chatter():
    assert cf("fra", "Bonjour", "<text>", "") == "empty"
    both = "I'm sorry, I can't translate this content. <text>"
    assert cf("fra", "Bonjour", both) == "prompt_echo"
    assert cf("fra", "Bonjour", "Sure! <text>Hello</text>", "<text>Hello</text>") == "prompt_echo"


# refusal precision (review note 1)
LONG_SRC = "\u3053\u308c\u306f\u3068\u3066\u3082\u9577\u3044\u30c6\u30ad\u30b9\u30c8\u3067\u3001\u5341\u4e94\u6587\u5b57\u3092\u8d85\u3048\u307e\u3059\u3002\u3053\u308c\u306f\u3068\u3066\u3082\u9577\u3044\u30c6\u30ad\u30b9\u30c8\u3067\u3059"


def test_refusal_first_sentence_only_provide_is_not_a_request_word():
    src = "\u9045\u304f\u306a\u3063\u3066\u3059\u307f\u307e\u305b\u3093\u3001\u8cc7\u6599\u306f\u660e\u65e5\u304a\u9001\u308a\u3057\u307e\u3059"
    out = "I'm sorry for the delay. I will provide the documents tomorrow."
    assert cf("jpn", src, out) is None


def test_refusal_help_is_not_a_request_word():
    src = "\u4ed5\u65b9\u306a\u3044\u3088\u3001\u5927\u597d\u304d\u306a\u3093\u3060\u3082\u3093\uff01"
    assert cf("jpn", src, "I can't help it, I love you!") is None


def test_request_word_in_later_sentence_does_not_count():
    assert cf("fra", "Traduis ce texte s'il te plait", "I'm sorry. I can't translate this request.") is None


def test_bare_refusal_with_short_source_is_not_flagged():
    assert LONG_SRC and len(LONG_SRC) > 15
    assert cf("jpn", "\u7121\u7406\u2026!", "I can't.") is None
    assert cf("jpn", "\u7121\u7406\u2026!", "I cannot") is None


def test_bare_refusal_with_long_source_is_flagged():
    for out in ("I cannot.", "I can't", "I am unable.", "I'm unable!"):
        assert cf("jpn", LONG_SRC, out) == "refusal", out


def test_bare_refusal_source_length_boundary():
    assert cf("jpn", "x" * 15, "I cannot.") is None
    assert cf("jpn", "x" * 16, "I cannot.") == "refusal"


def test_refusal_french_request_sentence():
    assert cf("fra", "Traduis ce texte s'il te pla\u00eet", "I'm sorry, I can't translate this request.") == "refusal"
