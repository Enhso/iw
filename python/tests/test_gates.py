"""Tests for iw_research.gates: relevance/injection filter, claim support."""

from iw_research.gates import (
    INJECTION_DROP_THRESHOLD,
    RELEVANCE_DROP_THRESHOLD,
    bounded_map,
    filter_sources,
    score_claim_support,
)
from iw_research.jev import Jev, JevError, Question
from iw_research.schema import (
    Claim,
    Evidence,
    ExtractionPayload,
    Source,
    content_sha256,
)
from iw_research.sources import SourceDocument


def _doc(**overrides: object) -> SourceDocument:
    defaults: dict[str, object] = {
        "id": "src:wikipedia:0000000000000000",
        "provider": "wikipedia",
        "title": "Title",
        "url": "https://example.com/one",
        "published": "",
        "retrieved_at": "2026-09-14T00:00:00Z",
        "text": "Some passage text.",
    }
    defaults.update(overrides)
    return SourceDocument(**defaults)  # type: ignore[arg-type]


class _StubJev:
    """A `Jev` returning pre-programmed answers, or raising on request."""

    def __init__(self, answers: dict[str, dict[str, dict[str, object]]]) -> None:
        self._answers = answers

    def ask(
        self, state: object, questions: dict[str, Question]
    ) -> dict[str, dict[str, object]]:
        assert isinstance(state, dict)
        key = (
            state.get("passage", {}).get("title")
            if "passage" in state
            else state.get("claim")
        )
        if key not in self._answers:
            raise JevError(f"no stub answer for {key!r}")
        return self._answers[key]


def test_bounded_map_preserves_order() -> None:
    assert bounded_map(lambda x: x * 2, [1, 2, 3, 4, 5]) == [2, 4, 6, 8, 10]


def test_bounded_map_empty_list() -> None:
    assert bounded_map(lambda x: x, []) == []


def test_filter_sources_skips_when_jev_is_none() -> None:
    docs = [_doc(url="https://a"), _doc(url="https://b")]
    kept, dropped, gate_log = filter_sources(docs, None, "question?")
    assert kept == docs
    assert dropped == []
    assert len(gate_log) == 1
    assert gate_log[0].status == "skipped"


def test_filter_sources_keeps_relevant_non_injecting_passage() -> None:
    doc = _doc(title="Relevant Doc")
    jev: Jev = _StubJev(
        {
            "Relevant Doc": {
                "relevant": {"type": "noul", "noul": 0.9},
                "injection": {"type": "noul", "noul": 0.01},
            }
        }
    )
    kept, dropped, gate_log = filter_sources([doc], jev, "question?")
    assert kept == [doc]
    assert dropped == []
    assert gate_log[0].status == "ok"
    assert "kept" in gate_log[0].detail


def test_filter_sources_drops_irrelevant_passage() -> None:
    doc = _doc(title="Off Topic", url="https://irrelevant")
    jev: Jev = _StubJev(
        {
            "Off Topic": {
                "relevant": {"type": "noul", "noul": RELEVANCE_DROP_THRESHOLD - 0.01},
                "injection": {"type": "noul", "noul": 0.0},
            }
        }
    )
    kept, dropped, gate_log = filter_sources([doc], jev, "question?")
    assert kept == []
    assert len(dropped) == 1
    assert dropped[0].reason == "irrelevant"
    assert dropped[0].url == "https://irrelevant"


def test_filter_sources_drops_injecting_passage() -> None:
    doc = _doc(title="Malicious", url="https://malicious")
    jev: Jev = _StubJev(
        {
            "Malicious": {
                "relevant": {"type": "noul", "noul": 0.9},
                "injection": {"type": "noul", "noul": INJECTION_DROP_THRESHOLD + 0.01},
            }
        }
    )
    kept, dropped, gate_log = filter_sources([doc], jev, "question?")
    assert kept == []
    assert len(dropped) == 1
    assert dropped[0].reason == "injection"


def test_filter_sources_fails_open_on_jev_error() -> None:
    doc = _doc(title="Errors Out")
    jev: Jev = _StubJev({})  # no stub answer -> JevError
    kept, dropped, gate_log = filter_sources([doc], jev, "question?")
    assert kept == [doc]
    assert dropped == []
    assert gate_log[0].status == "failed"


def _payload_with_claim(support_excerpts: list[str]) -> ExtractionPayload:
    source = Source(
        id="src:wikipedia:0000000000000000",
        title="T",
        url="https://example.com",
        provider="wikipedia",
        published="",
        retrieved_at="2026-09-14T00:00:00Z",
        content="c",
        content_hash=content_sha256("c"),
    )
    claim = Claim(
        id="clm:one", text="Some claim text", kind="hypothesis", subject_ids=[]
    )
    evidence = [
        Evidence(
            id=f"evd:{i}",
            claim_id="clm:one",
            source_id="src:wikipedia:0000000000000000",
            stance="supports",
            excerpt=excerpt,
            quality=0.5,
        )
        for i, excerpt in enumerate(support_excerpts)
    ]
    return ExtractionPayload(
        question="Q?", sources=[source], claims=[claim], evidence=evidence
    )


def test_score_claim_support_skips_when_jev_is_none() -> None:
    payload = _payload_with_claim(["an excerpt"])
    result, gate_log = score_claim_support(payload, None)
    assert result.claims[0].support is None
    assert result.claims[0].support_method == "none"
    assert len(gate_log) == 1
    assert gate_log[0].status == "skipped"


def test_score_claim_support_leaves_claims_without_evidence_untouched() -> None:
    payload = _payload_with_claim([])
    jev: Jev = _StubJev({})
    result, gate_log = score_claim_support(payload, jev)
    assert result.claims[0].support is None
    assert gate_log == []


def test_score_claim_support_sets_support_from_score_over_four() -> None:
    payload = _payload_with_claim(["an excerpt"])
    jev: Jev = _StubJev(
        {"Some claim text": {"support": {"type": "score", "score": 3.0}}}
    )
    result, gate_log = score_claim_support(payload, jev)
    assert result.claims[0].support == 0.75
    assert result.claims[0].support_method == "jev"
    assert gate_log[0].status == "ok"


def test_score_claim_support_clamps_support_to_unit_interval() -> None:
    payload = _payload_with_claim(["an excerpt"])
    jev: Jev = _StubJev(
        {"Some claim text": {"support": {"type": "score", "score": 4.4}}}
    )
    result, _ = score_claim_support(payload, jev)
    assert result.claims[0].support == 1.0


def test_score_claim_support_fails_open_on_jev_error() -> None:
    payload = _payload_with_claim(["an excerpt"])
    jev: Jev = _StubJev({})  # no stub answer -> JevError
    result, gate_log = score_claim_support(payload, jev)
    assert result.claims[0].support is None
    assert result.claims[0].support_method == "none"
    assert gate_log[0].status == "failed"
