"""Doc-example compile/type-check guard.

Extracts opt-in fenced code blocks from the docs and type-checks them against
the in-repo SDK sources, so a PR that changes an SDK's public API breaks the
matching doc example in the *same* PR.

See README.md for the marker convention and how to add languages.
"""
