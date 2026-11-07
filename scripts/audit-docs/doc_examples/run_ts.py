#!/usr/bin/env python3
"""Type-check ``doctest``-marked TypeScript doc examples against the in-repo SDK.

Each marked ```typescript doctest``` block is written to a temp file inside
``sdks/typescript/`` (so Node module resolution finds the SDK's own
node_modules) and compiled with ``tsc --noEmit``. The bare specifier
``@solvela/sdk`` is mapped to ``sdks/typescript/src/index.ts`` via tsconfig
``paths`` — so the doc example is checked against the SDK *source*, not a
possibly-stale published build. A renamed/removed export breaks the example
here, in the same PR that changed the SDK.

Exit code 0 = all marked TS blocks type-check (or none exist).
Exit code 1 = at least one block failed (errors are mapped to doc file:line).

Usage:
    python3 scripts/audit-docs/doc_examples/run_ts.py
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[2]  # scripts/audit-docs/doc_examples -> repo root
sys.path.insert(0, str(HERE.parent))

from doc_examples.extract import extract_blocks  # noqa: E402

DOC_ROOTS = [
    REPO_ROOT / "dashboard" / "content" / "docs",
    REPO_ROOT / "docs",
]
TS_SDK = REPO_ROOT / "sdks" / "typescript"
TMP_DIR = TS_SDK / ".doctest-tmp"

TSCONFIG = {
    "compilerOptions": {
        "target": "ES2022",
        "module": "ESNext",
        "moduleResolution": "bundler",
        "strict": True,
        "esModuleInterop": True,
        "skipLibCheck": True,
        "noEmit": True,
        "forceConsistentCasingInFileNames": True,
        "resolveJsonModule": True,
        "types": ["node"],
        "baseUrl": ".",
        "paths": {
            "@solvela/sdk": ["../src/index.ts"],
            "@solvela/sdk/*": ["../src/*"],
        },
    },
    "include": ["*.ts"],
}

_TSC_ERR = re.compile(
    r"^(?P<file>block_\d+\.ts)\((?P<line>\d+),(?P<col>\d+)\):\s*(?P<msg>.*)$"
)


def main() -> int:
    blocks = extract_blocks(DOC_ROOTS, REPO_ROOT, lang="typescript")
    if not blocks:
        print("doctest(ts): no ```typescript doctest``` blocks found — nothing to check.")
        return 0

    if not (TS_SDK / "node_modules").exists():
        print(
            "doctest(ts): sdks/typescript/node_modules missing — run `npm ci` "
            "in sdks/typescript first.",
            file=sys.stderr,
        )
        return 2

    if TMP_DIR.exists():
        shutil.rmtree(TMP_DIR)
    TMP_DIR.mkdir(parents=True)

    # Map temp filename -> (doc location, fence body line offset).
    index: dict[str, object] = {}
    try:
        for i, b in enumerate(blocks):
            fname = f"block_{i}.ts"
            # Force module scope so each block is isolated (no global collisions
            # between same-named top-level consts across examples).
            (TMP_DIR / fname).write_text(b.code + "\nexport {};\n", encoding="utf-8")
            index[fname] = b

        (TMP_DIR / "tsconfig.json").write_text(json.dumps(TSCONFIG, indent=2))

        proc = subprocess.run(
            ["npx", "tsc", "-p", "tsconfig.json", "--pretty", "false"],
            cwd=TMP_DIR,
            capture_output=True,
            text=True,
        )

        failures: list[str] = []
        for raw in (proc.stdout + proc.stderr).splitlines():
            m = _TSC_ERR.match(raw.strip())
            if not m:
                if raw.strip():
                    failures.append(raw.rstrip())
                continue
            b = index[m.group("file")]
            # body line L (1-based) -> doc line = fence line + L
            doc_line = b.line + int(m.group("line"))
            failures.append(f"{b.file}:{doc_line}  {m.group('msg')}")

        if proc.returncode == 0 and not failures:
            print(f"doctest(ts): OK — {len(blocks)} block(s) type-check against @solvela/sdk source.")
            return 0

        print(f"doctest(ts): FAILED — {len(blocks)} block(s) checked, errors below:\n")
        for f in failures:
            print(f"  {f}")
        print(
            "\nThese doc examples no longer type-check against sdks/typescript/src. "
            "Either fix the example or, if the snippet is intentionally a fragment, "
            "remove its `doctest` marker."
        )
        return 1
    finally:
        shutil.rmtree(TMP_DIR, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
