"""Extract opt-in fenced code blocks from Markdown / MDX docs.

A block is a *doctest* when the fence info string carries the ``doctest``
token, e.g.::

    ```typescript doctest
    import { SolvelaClient } from '@solvela/sdk';
    ...
    ```

Opt-in is deliberate: most fences in the docs are intentional fragments
(placeholders like ``yourWalletAdapter``, bare ``interface`` declarations,
JSON response bodies). Only blocks an author marks ``doctest`` are expected
to be complete and to type-check against the real in-repo SDK sources.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

# Languages we treat as the same toolchain bucket.
LANG_ALIASES = {
    "ts": "typescript",
    "typescript": "typescript",
    "js": "typescript",
    "javascript": "typescript",
    "py": "python",
    "python": "python",
    "go": "go",
    "rust": "rust",
    "rs": "rust",
}

_FENCE = re.compile(
    r"^(?P<indent>[ \t]*)```(?P<info>[^\n]*)\n(?P<body>.*?)^(?P=indent)```",
    re.S | re.M,
)


@dataclass(frozen=True)
class Block:
    file: str          # path relative to repo root
    line: int          # 1-based line of the opening fence
    lang: str          # normalized language bucket (typescript / python / ...)
    raw_lang: str      # language token as written
    meta: str          # full fence info string after the language token
    code: str          # block body (no fences)

    @property
    def location(self) -> str:
        return f"{self.file}:{self.line}"


def _info_tokens(info: str) -> tuple[str, list[str]]:
    parts = info.strip().split()
    if not parts:
        return "", []
    return parts[0], parts[1:]


def is_doctest(meta_tokens: list[str]) -> bool:
    return "doctest" in meta_tokens


def extract_blocks(
    roots: list[Path],
    repo_root: Path,
    lang: str | None = None,
) -> list[Block]:
    """Return all ``doctest``-marked blocks under ``roots``.

    ``lang`` (normalized, e.g. "typescript") filters to one toolchain bucket.
    """
    out: list[Block] = []
    seen: set[Path] = set()
    for root in roots:
        if not root.exists():
            continue
        for path in sorted([*root.rglob("*.md"), *root.rglob("*.mdx")]):
            if path in seen:
                continue
            seen.add(path)
            text = path.read_text(encoding="utf-8", errors="replace")
            for m in _FENCE.finditer(text):
                raw_lang, tokens = _info_tokens(m.group("info"))
                if not is_doctest(tokens):
                    continue
                norm = LANG_ALIASES.get(raw_lang.lower())
                if norm is None:
                    raise ValueError(
                        f"{path}: ```{raw_lang} doctest``` — unsupported "
                        f"doctest language '{raw_lang}'"
                    )
                if lang is not None and norm != lang:
                    continue
                line = text[: m.start()].count("\n") + 1
                out.append(
                    Block(
                        file=str(path.relative_to(repo_root)),
                        line=line,
                        lang=norm,
                        raw_lang=raw_lang,
                        meta=m.group("info").strip(),
                        code=m.group("body"),
                    )
                )
    return out
