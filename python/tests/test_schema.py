"""Tests for iw_research.schema: slugify/make_id, normalized, check_integrity."""

import logging

import pytest
from pydantic import ValidationError

from iw_research.schema import (
    Claim,
    Entity,
    Evidence,
    ExtractionPayload,
    Source,
    make_id,
    slugify,
)


def test_slugify_lowercases_and_collapses_non_alphanumeric() -> None:
    assert (
        slugify("Semiconductor Industry in China!") == "semiconductor-industry-in-china"
    )


def test_slugify_strips_leading_and_trailing_dashes() -> None:
    assert slugify("  --Extreme UV--  ") == "extreme-uv"


def test_slugify_collapses_runs_of_separators() -> None:
    assert slugify("a___b   c") == "a-b-c"


def test_make_id_builds_prefixed_slug() -> None:
    assert make_id("src", "wikipedia-Semiconductor industry in China") == (
        "src:wikipedia-semiconductor-industry-in-china"
    )


def test_make_id_truncates_to_sixty_chars_and_strips_trailing_dash() -> None:
    long_text = "word " * 30  # slugifies to a long run of "word-word-word-..."
    result = make_id("clm", long_text)
    body = result.removeprefix("clm:")
    assert len(body) <= 60
    assert not body.endswith("-")


def _minimal_payload(**overrides: object) -> ExtractionPayload:
    data: dict[str, object] = {
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
    return ExtractionPayload.model_validate(data)


def test_normalized_dedupes_by_id_keeping_first() -> None:
    payload = _minimal_payload(
        entities=[
            {
                "id": "ent:china",
                "name": "China",
                "kind": "country",
                "description": "first",
            },
            {
                "id": "ent:china",
                "name": "China",
                "kind": "country",
                "description": "second",
            },
        ]
    )
    normalized = payload.normalized()
    assert len(normalized.entities) == 1
    assert normalized.entities[0].description == "first"


def test_normalized_drops_dangling_subject_id_with_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        claims=[
            {
                "id": "clm:one",
                "text": "Some claim",
                "kind": "hypothesis",
                "subject_ids": ["ent:china", "ent:nonexistent"],
            }
        ]
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.claims[0].subject_ids == ["ent:china"]
    assert any("ent:nonexistent" in record.message for record in caplog.records)


def test_normalized_drops_dangling_actor_id_with_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        events=[
            {
                "id": "evt:one",
                "name": "Event",
                "occurred_at": "2022-01-01",
                "description": "d",
                "actor_ids": ["ent:china", "ent:ghost"],
            }
        ]
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.events[0].actor_ids == ["ent:china"]
    assert any("ent:ghost" in record.message for record in caplog.records)


def test_normalized_drops_evidence_with_unknown_claim_id(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        evidence=[
            {
                "id": "evd:one",
                "claim_id": "clm:missing",
                "source_id": "src:one",
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ]
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.evidence == []
    assert any("clm:missing" in record.message for record in caplog.records)


def test_normalized_drops_evidence_with_unknown_source_id(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        evidence=[
            {
                "id": "evd:one",
                "claim_id": "clm:one",
                "source_id": "src:missing",
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ]
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.evidence == []
    assert any("src:missing" in record.message for record in caplog.records)


def test_normalized_drops_causal_link_with_unknown_endpoint(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        causal_links=[
            {
                "cause_id": "clm:one",
                "effect_id": "clm:missing",
                "mechanism": "m",
                "confidence": "low",
            }
        ]
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.causal_links == []
    assert any("clm:missing" in record.message for record in caplog.records)


def test_normalized_drops_temporal_relation_with_unknown_endpoint(
    caplog: pytest.LogCaptureFixture,
) -> None:
    payload = _minimal_payload(
        events=[
            {
                "id": "evt:one",
                "name": "Event",
                "occurred_at": "2022-01-01",
                "description": "d",
                "actor_ids": [],
            }
        ],
        temporal_relations=[
            {"before_id": "evt:one", "after_id": "evt:missing", "relation": "before"}
        ],
    )
    with caplog.at_level(logging.WARNING):
        normalized = payload.normalized()
    assert normalized.temporal_relations == []
    assert any("evt:missing" in record.message for record in caplog.records)


def test_check_integrity_raises_on_dangling_reference() -> None:
    payload = _minimal_payload(
        evidence=[
            {
                "id": "evd:one",
                "claim_id": "clm:missing",
                "source_id": "src:one",
                "stance": "supports",
                "excerpt": "x",
                "quality": 0.5,
            }
        ]
    )
    with pytest.raises(ValueError, match="unknown reference"):
        payload.check_integrity()


def test_check_integrity_raises_on_duplicate_id() -> None:
    payload = _minimal_payload(
        entities=[
            {"id": "ent:china", "name": "China", "kind": "country", "description": "a"},
            {"id": "ent:china", "name": "China", "kind": "country", "description": "b"},
        ]
    )
    with pytest.raises(ValueError, match="duplicate id"):
        payload.check_integrity()


def test_check_integrity_passes_for_a_clean_payload() -> None:
    payload = _minimal_payload()
    payload.check_integrity()


def test_entity_rejects_bad_enum_kind() -> None:
    with pytest.raises(ValidationError):
        Entity(id="ent:x", name="X", kind="alien", description="d")  # type: ignore[arg-type]


def test_source_rejects_bad_enum_provider() -> None:
    with pytest.raises(ValidationError):
        Source(
            id="src:x",
            title="T",
            url="https://example.com",
            provider="bing",  # type: ignore[arg-type]
            published="",
            retrieved_at="2026-09-14T00:00:00Z",
        )


def test_claim_rejects_bad_enum_kind() -> None:
    with pytest.raises(ValidationError):
        Claim(id="clm:x", text="t", kind="opinion", subject_ids=[])  # type: ignore[arg-type]


def test_evidence_rejects_bad_enum_stance() -> None:
    with pytest.raises(ValidationError):
        Evidence(
            id="evd:x",
            claim_id="clm:x",
            source_id="src:x",
            stance="neutral",  # type: ignore[arg-type]
            excerpt="e",
            quality=0.5,
        )
