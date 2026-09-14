"""Tests for ExtractionPayload.from_untrusted: the full Rust contract.

Each test feeds one class of violation an LLM could plausibly produce and
asserts it is repaired or dropped (with a `logger.warning`) rather than
failing the whole extraction, and that every id, date, and quality in the
result passes `is_valid_id`, `is_valid_date`, and `is_valid_quality`.
"""

import logging

import pytest

from iw_research.schema import (
    ExtractionPayload,
    is_valid_date,
    is_valid_id,
    is_valid_quality,
)


def _base_raw(**overrides: object) -> dict[str, object]:
    data: dict[str, object] = {
        "schema_version": 1,
        "question": "Q?",
        "entities": [
            {"id": "ent:china", "name": "China", "kind": "country", "description": "d"}
        ],
        "sources": [
            {
                "id": "src:one",
                "title": "T",
                "url": "https://example.com",
                "provider": "wikipedia",
                "published": "",
                "retrieved_at": "2026-09-14T00:00:00Z",
            }
        ],
        "claims": [
            {
                "id": "clm:one",
                "text": "Some claim",
                "kind": "hypothesis",
                "subject_ids": ["ent:china"],
            }
        ],
    }
    data.update(overrides)
    return data


def _assert_every_id_date_and_quality_is_valid(payload: ExtractionPayload) -> None:
    for entity in payload.entities:
        assert is_valid_id(entity.id)
    for event in payload.events:
        assert is_valid_id(event.id)
        assert is_valid_date(event.occurred_at)
    for source in payload.sources:
        assert is_valid_id(source.id)
        assert is_valid_date(source.published)
    for claim in payload.claims:
        assert is_valid_id(claim.id)
    for item in payload.evidence:
        assert is_valid_id(item.id)
        assert is_valid_quality(item.quality)


def test_entity_with_bad_enum_kind_is_dropped() -> None:
    raw = _base_raw(
        entities=[
            {"id": "ent:china", "name": "China", "kind": "country", "description": "d"},
            {"id": "ent:acme", "name": "Acme", "kind": "company", "description": "d"},
        ],
        claims=[
            {
                "id": "clm:one",
                "text": "Some claim",
                "kind": "hypothesis",
                "subject_ids": ["ent:china", "ent:acme"],
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert [entity.id for entity in payload.entities] == ["ent:china"]
    assert payload.claims[0].subject_ids == ["ent:china"]
    _assert_every_id_date_and_quality_is_valid(payload)


def test_malformed_entity_id_is_repaired_and_references_remapped() -> None:
    raw = _base_raw(
        entities=[
            {
                "id": "ent:U.S._Gov",
                "name": "U.S. Gov",
                "kind": "organization",
                "description": "d",
            }
        ],
        claims=[
            {
                "id": "clm:one",
                "text": "Some claim",
                "kind": "hypothesis",
                "subject_ids": ["ent:U.S._Gov"],
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert [entity.id for entity in payload.entities] == ["ent:u-s-gov"]
    assert payload.claims[0].subject_ids == ["ent:u-s-gov"]
    _assert_every_id_date_and_quality_is_valid(payload)


def test_long_claim_id_is_repaired_within_80_chars_and_evidence_remapped() -> None:
    long_id = "clm:" + "a" * 90
    assert len(long_id) == 94
    raw = _base_raw(
        claims=[
            {
                "id": long_id,
                "text": "Some claim",
                "kind": "hypothesis",
                "subject_ids": ["ent:china"],
            }
        ],
        evidence=[
            {
                "id": "evd:one",
                "claim_id": long_id,
                "source_id": "src:one",
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert len(payload.claims) == 1
    repaired_id = payload.claims[0].id
    assert len(repaired_id) <= 80
    assert repaired_id != long_id
    assert len(payload.evidence) == 1
    assert payload.evidence[0].claim_id == repaired_id
    _assert_every_id_date_and_quality_is_valid(payload)


def test_invalid_occurred_at_is_blanked() -> None:
    raw = _base_raw(
        events=[
            {
                "id": "evt:one",
                "name": "Event",
                "occurred_at": "October 2022",
                "description": "d",
                "actor_ids": [],
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert len(payload.events) == 1
    assert payload.events[0].occurred_at == ""
    _assert_every_id_date_and_quality_is_valid(payload)


def test_evidence_with_out_of_range_quality_is_dropped() -> None:
    raw = _base_raw(
        evidence=[
            {
                "id": "evd:one",
                "claim_id": "clm:one",
                "source_id": "src:one",
                "stance": "supports",
                "excerpt": "x",
                "quality": 7,
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert payload.evidence == []
    _assert_every_id_date_and_quality_is_valid(payload)


def test_forecasting_language_drops_claim_and_causal_link() -> None:
    raw = _base_raw(
        claims=[
            {
                "id": "clm:one",
                "text": "There is a 70% chance controls hold",
                "kind": "hypothesis",
                "subject_ids": ["ent:china"],
            },
            {
                "id": "clm:two",
                "text": "A claim without forecasting language",
                "kind": "hypothesis",
                "subject_ids": ["ent:china"],
            },
            {
                "id": "clm:three",
                "text": "Another claim without forecasting language",
                "kind": "hypothesis",
                "subject_ids": ["ent:china"],
            },
        ],
        causal_links=[
            {
                "cause_id": "clm:two",
                "effect_id": "clm:three",
                "mechanism": "High likelihood of a durable effect",
                "confidence": "low",
            }
        ],
    )
    payload = ExtractionPayload.from_untrusted(raw)
    assert [claim.id for claim in payload.claims] == ["clm:two", "clm:three"]
    assert payload.causal_links == []
    _assert_every_id_date_and_quality_is_valid(payload)


def test_duplicate_entity_id_keeps_first_and_logs_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    raw = _base_raw(
        entities=[
            {"id": "ent:china", "name": "First", "kind": "country", "description": "d"},
            {
                "id": "ent:china",
                "name": "Second",
                "kind": "country",
                "description": "d",
            },
        ],
    )
    with caplog.at_level(logging.WARNING):
        payload = ExtractionPayload.from_untrusted(raw)
    assert [entity.name for entity in payload.entities] == ["First"]
    assert any(
        "duplicate" in record.message and "ent:china" in record.message
        for record in caplog.records
    )
    _assert_every_id_date_and_quality_is_valid(payload)
