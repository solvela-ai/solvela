"""Live test configuration — skipped unless SOLVELA_LIVE_TESTS=1."""

from __future__ import annotations

import os

import pytest

from solvela.client import SolvelaClient
from solvela.config import ClientConfig

GATEWAY_URL = os.environ.get("SOLVELA_GATEWAY_URL", "http://localhost:8402")


@pytest.fixture
def live_client():
    config = ClientConfig(gateway_url=GATEWAY_URL)
    return SolvelaClient(config=config)


def pytest_collection_modifyitems(config, items):
    if os.environ.get("SOLVELA_LIVE_TESTS") != "1":
        skip = pytest.mark.skip(reason="Set SOLVELA_LIVE_TESTS=1 to run")
        for item in items:
            if "live" in str(item.fspath):
                item.add_marker(skip)
