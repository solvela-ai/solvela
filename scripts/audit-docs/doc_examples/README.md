# Doc-example doctest guard

Type-checks **opt-in** fenced code blocks in the docs against the **in-repo SDK
sources**, so a PR that changes an SDK's public API breaks the matching doc
example in the *same* PR.

This closes a long-standing gap: the doc code examples were type-checked **once,
by hand** (PR #369, 2026-05-21), but nothing guarded them afterward, so they
could silently drift as the SDKs evolved.

## The marker convention

A code block is checked only when its fence info string carries the `doctest`
token:

````md
```typescript doctest
import { SolvelaClient } from '@solvela/sdk';
const client = new SolvelaClient({ config: { gatewayUrl: 'https://api.solvela.ai' } });
```
````

Opt-in is deliberate. Most fences in the docs are **intentional fragments** —
placeholders (`yourWalletAdapter`), bare `interface` declarations referencing
un-imported types, JSON response bodies. Marking *every* fence would flood CI
with false failures. Only mark a block `doctest` when it is a complete example
that should compile against the real SDK.

The `doctest` token is ignored by the docs renderer (Fumadocs/Shiki treat
unknown fence metadata as a no-op), so it has no visual effect on the published
page.

### Rules for a `doctest` block

- It must be self-contained: every identifier is either imported or declared in
  the block. (No placeholders like `yourWalletAdapter`.)
- It is checked as an ES module against the SDK **source** (`sdks/<lang>/`), not
  a published build — so it reflects the API as it exists in this repo.
- If an example is meant to be illustrative/partial, **don't** mark it
  `doctest`.

## Running locally

```bash
# one-time: install the TS SDK's own deps so its types resolve
( cd sdks/typescript && npm ci )

python3 scripts/audit-docs/doc_examples/run_ts.py
```

Exit `0` = all marked blocks type-check (or none exist). Exit `1` = at least one
failed; errors are reported at the **doc** `file:line`, e.g.:

```
dashboard/content/docs/quickstart.mdx:59  error TS2305: Module '"@solvela/sdk"' has no exported member 'Foo'.
```

## Currently covered

- **TypeScript** — blocks importing `@solvela/sdk`, checked against
  `sdks/typescript/src` (`run_ts.py`).

## Adding a language

`extract.py` is language-agnostic; it already recognizes `python`, `go`, and
`rust` doctest blocks. To enforce one, add a `run_<lang>.py` that:

1. calls `extract_blocks(DOC_ROOTS, REPO_ROOT, lang="<lang>")`,
2. writes each block to a temp file and runs that toolchain's type-checker
   against the in-repo SDK (`mypy`/`pyright` for Python, `go build` for Go,
   `cargo check` with a path dep for Rust),
3. maps errors back to `block.file:block.line + <error line>`,

then add a job to `.github/workflows/docs-doctest.yml`. Examples that pull in
many external deps (`ai`, `viem`, `zod`, …) need those installed before they can
be enforced; start with SDK-only examples.
