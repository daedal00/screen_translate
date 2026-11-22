# bench/data

## flores_devtest.jsonl (250 rows)

- Source: FLORES-200 devtest, https://dl.fbaipublicfiles.com/nllb/flores200_dataset.tar.gz
- License: CC BY-SA 4.0. Citation: NLLB Team et al., "No Language Left Behind: Scaling Human-Centered Machine Translation", 2022 (arXiv:2207.04672). FLORES-101 (Goyal et al., 2022) and Guzman et al., 2019 are the upstream releases.
- Rows: 50 sentences per source language (jpn, kor, fra, deu, spa). Same indices for every language (FLORES is multi-parallel): devtest lines 0, 20, 40, ... 980 (0-based, every 20th line, first 50). `ref` is the `eng_Latn` line at that index. Ids are `flores-<lang>-<index:04d>` (e.g. `flores-jpn-0020`).
- Modifications: none beyond whitespace stripping and selecting lines. Upstream quirk kept as is: `kor_Hang` has CSV-style doubled quotes (`""`) on some lines, 4 of them selected (`flores-kor-0000`, `0160`, `0320`, `0960`); do not fix by hand. Derived file is shared under the same CC BY-SA 4.0 license.

Regenerate (stdlib only):

```
curl -LO https://dl.fbaipublicfiles.com/nllb/flores200_dataset.tar.gz
uv run bench/data/make_flores.py flores200_dataset.tar.gz
```

## screen.jsonl (125 rows)

- 25 items per language (jpn, kor, fra, deu, spa) across categories: ui, menu, error, game, chat, signage, manga (4/4/4/4/3/3/3 per language, same slot order in every language).
- Structure: 8 parallel anchor scenarios (slots 002 discard confirm, 010 `{filename}` open error, 012 storage numbers, 014 HP recovery, 019 polite "I'll send the materials tomorrow", 021 sale dates, 023 and 024 manga) localised per language, with numbers, dates and codes varied per language. The other 17 slots per language are language-specific content chosen for native phenomena (e.g. keigo and subject drop for jpn, banmal/jondaemal and Konglish for kor, tu/vous and dropped "ne" and inclusive writing for fra, compounds and separable verbs across a line break and Sie/du for deu, Latin-American vs Spain usage and `¡¿...?!` for spa).
- All source texts and English references drafted by Claude (AI), not human-verified; use as indicative. No real people's data; names are fictional or generic.
- Hard cases: honorifics and titles, numbers/currencies, line breaks inside an item, `{placeholders}`, error codes, mixed English UI words, onomatopoeia. Dates in references are deliberately reformatted to US style (e.g. `25/12` becomes `12/25`, `25.12.` becomes `Dec. 25`) because the target is EN-US.
