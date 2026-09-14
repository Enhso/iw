"""Tests that iw_research.schema mirrors Rust's ExtractionPayload contract.

Reads the same `fixtures/contract/vectors.json` shared with the Rust
`contract_vectors_classify_ids_dates_and_quality_like_rust_does` test in
`src/model.rs`, and asserts `is_valid_id`, `is_valid_date`, and
`is_valid_quality` classify every vector the same way Rust does.
"""

import json
from pathlib import Path

import pytest

from iw_research.schema import is_valid_date, is_valid_id, is_valid_quality, make_id

_VECTORS_PATH = (
    Path(__file__).resolve().parents[2] / "fixtures" / "contract" / "vectors.json"
)
_VECTORS = json.loads(_VECTORS_PATH.read_text())


@pytest.mark.parametrize("value", _VECTORS["ids"]["valid"])
def test_valid_ids_pass(value: str) -> None:
    assert is_valid_id(value)


@pytest.mark.parametrize("value", _VECTORS["ids"]["invalid"])
def test_invalid_ids_fail(value: str) -> None:
    assert not is_valid_id(value)


@pytest.mark.parametrize("value", _VECTORS["dates"]["valid"])
def test_valid_dates_pass(value: str) -> None:
    assert is_valid_date(value)


@pytest.mark.parametrize("value", _VECTORS["dates"]["invalid"])
def test_invalid_dates_fail(value: str) -> None:
    assert not is_valid_date(value)


@pytest.mark.parametrize("value", _VECTORS["quality"]["valid"])
def test_valid_quality_passes(value: float) -> None:
    assert is_valid_quality(value)


@pytest.mark.parametrize("value", _VECTORS["quality"]["invalid"])
def test_invalid_quality_fails(value: float) -> None:
    assert not is_valid_quality(value)


@pytest.mark.parametrize(
    ("prefix", "text"),
    [
        ("ent", "Societe Generale"),
        ("ent", "U.S._Gov"),
        ("ent", "海关总署"),
    ],
)
def test_make_id_always_returns_a_valid_id_or_an_empty_prefix_only_id(
    prefix: str, text: str
) -> None:
    result = make_id(prefix, text)
    assert is_valid_id(result) or result == f"{prefix}:"
