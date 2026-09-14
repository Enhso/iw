"""Shared pytest fixtures: paths to the repository's offline fixtures."""

from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def fixture_dir() -> Path:
    """The repository's `fixtures/offline` directory."""
    return _REPO_ROOT / "fixtures" / "offline"


@pytest.fixture
def payload_path() -> Path:
    """The committed canonical payload, `fixtures/payload/semiconductor.json`."""
    return _REPO_ROOT / "fixtures" / "payload" / "semiconductor.json"
