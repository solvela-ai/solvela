"""Shared pytest config: bootstrap import paths so tests can `from common import …`.

The audit-docs package is intentionally not pip-installable. We add the parent
directory to sys.path here so test modules see the same names that `audit.py`
sees at runtime.
"""

from __future__ import annotations

import sys
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_AUDIT_DOCS = _HERE.parent
sys.path.insert(0, str(_AUDIT_DOCS))
