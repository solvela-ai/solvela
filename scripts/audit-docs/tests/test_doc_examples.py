"""Tests for the doc-example extractor (doc_examples.extract).

Stands up a tiny synthetic docs tree in a tmpdir and asserts which fenced
blocks the extractor treats as ``doctest`` blocks, that indented fences (as
used inside MDX ``<Tab>`` components) are handled, and that line numbers map
back to the source.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from doc_examples.extract import (
    LANG_ALIASES,
    extract_blocks,
    is_doctest,
)

DOC = """\
# Title

```typescript doctest
import { SolvelaClient } from '@solvela/sdk';
```

A plain, unmarked block (a fragment) must be ignored:

```typescript
const x = somePlaceholder;
```

An indented marked block inside a Tab:

<Tab>
    ```python doctest
    from solvela import SolvelaClient
    ```
</Tab>

A marked block in another language bucket:

```go doctest
package main
```
"""


def _write(tmp_path: Path, body: str) -> Path:
    root = tmp_path / "docs"
    root.mkdir()
    (root / "page.mdx").write_text(body, encoding="utf-8")
    return root


def test_only_marked_blocks_are_extracted(tmp_path: Path) -> None:
    root = _write(tmp_path, DOC)
    blocks = extract_blocks([root], tmp_path)
    langs = sorted(b.lang for b in blocks)
    # 3 marked blocks (ts, python, go); the unmarked ts fragment is skipped.
    assert langs == ["go", "python", "typescript"]


def test_language_filter(tmp_path: Path) -> None:
    root = _write(tmp_path, DOC)
    ts = extract_blocks([root], tmp_path, lang="typescript")
    assert len(ts) == 1
    assert ts[0].code.strip() == "import { SolvelaClient } from '@solvela/sdk';"


def test_indented_fence_is_handled(tmp_path: Path) -> None:
    root = _write(tmp_path, DOC)
    py = extract_blocks([root], tmp_path, lang="python")
    assert len(py) == 1
    # Body keeps its source indentation; content resolves after a strip.
    assert "from solvela import SolvelaClient" in py[0].code


def test_line_numbers_point_at_opening_fence(tmp_path: Path) -> None:
    root = _write(tmp_path, DOC)
    ts = extract_blocks([root], tmp_path, lang="typescript")[0]
    # The first ```typescript doctest fence is on line 3 of DOC.
    assert ts.line == 3
    assert ts.file == "docs/page.mdx"


def test_unmarked_block_not_doctest() -> None:
    assert is_doctest(["title=\"x\""]) is False
    assert is_doctest([]) is False
    assert is_doctest(["doctest"]) is True


def test_unsupported_doctest_language_raises(tmp_path: Path) -> None:
    root = _write(tmp_path, "```ruby doctest\nputs 1\n```\n")
    with pytest.raises(ValueError, match="unsupported doctest language"):
        extract_blocks([root], tmp_path)


def test_known_languages_normalize() -> None:
    assert LANG_ALIASES["ts"] == "typescript"
    assert LANG_ALIASES["js"] == "typescript"
    assert LANG_ALIASES["py"] == "python"
    assert LANG_ALIASES["rs"] == "rust"
