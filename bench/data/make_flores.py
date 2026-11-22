# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Rebuild flores_devtest.jsonl from the FLORES-200 tarball.

Usage: uv run bench/data/make_flores.py PATH/TO/flores200_dataset.tar.gz [OUT.jsonl]
Takes every 20th devtest line (indices 0, 20, ... 980; 50 lines), the same
indices for all five source languages. The reference is the eng_Latn line.
"""
import json
import sys
import tarfile
from pathlib import Path

LANGS = {"jpn": "jpn_Jpan", "kor": "kor_Hang", "fra": "fra_Latn", "deu": "deu_Latn", "spa": "spa_Latn"}
INDICES = range(0, 1000, 20)


def read_lines(tar: tarfile.TarFile, code: str) -> list[str]:
    name = f"flores200_dataset/devtest/{code}.devtest"
    member = next(m for m in tar.getmembers() if m.name.removeprefix("./") == name)
    f = tar.extractfile(member)
    assert f is not None, code
    return f.read().decode("utf-8").split("\n")


def main() -> None:
    tarball = sys.argv[1]
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else Path(__file__).with_name("flores_devtest.jsonl")
    with tarfile.open(tarball) as tar:
        ref = read_lines(tar, "eng_Latn")
        rows = []
        for lang, code in LANGS.items():
            src = read_lines(tar, code)
            for i in INDICES:
                rows.append({"id": f"flores-{lang}-{i:04d}", "set": "flores", "lang": lang,
                             "src": src[i].strip(), "ref": ref[i].strip()})
    with out.open("w", encoding="utf-8", newline="\n") as w:
        for r in rows:
            w.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"wrote {len(rows)} rows to {out}")


if __name__ == "__main__":
    main()
