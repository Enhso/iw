"""Tests for iw_research.family: classify-family routing and minting (A3/A4)."""

from iw_research.family import run_classify_family
from iw_research.jev import Jev, JevError, Question
from iw_research.llm import Completer, LlmError
from iw_research.request import ClassifyFamilyRequest, FamilyOption


class _StubJev:
    def __init__(
        self, answer: dict[str, object] | None = None, error: bool = False
    ) -> None:
        self._answer = answer
        self._error = error

    def ask(
        self, state: object, questions: dict[str, Question]
    ) -> dict[str, dict[str, object]]:
        if self._error:
            raise JevError("boom")
        assert self._answer is not None
        return {"family": self._answer}


class _StubCompleter:
    def __init__(
        self, result: dict[str, object] | None = None, error: bool = False
    ) -> None:
        self._result = result
        self._error = error

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        if self._error:
            raise LlmError("mint boom")
        assert self._result is not None
        return self._result


def _request(**overrides: object) -> ClassifyFamilyRequest:
    data: dict[str, object] = {
        "question": "Will the ECB cut its deposit rate at the October 2026 meeting?",
        "families": [
            FamilyOption(
                id="fam:ecb-rate-decisions",
                label="ECB rate decisions",
                description="Questions on ECB monetary-policy decisions and rate paths",
            )
        ],
    }
    data.update(overrides)
    return ClassifyFamilyRequest.model_validate(data)


def _never_called() -> Completer:
    raise AssertionError("make_completer should not have been called")


def test_matches_when_top_choice_is_not_none_and_probability_at_least_half() -> None:
    jev: Jev = _StubJev(
        {
            "choice": "fam:ecb-rate-decisions",
            "probabilities": {"fam:ecb-rate-decisions": 0.83, "none": 0.17},
            "confidence": 0.71,
        }
    )
    result = run_classify_family(_request(), jev, _never_called)
    assert result["decision"] == "matched"
    assert result["family_id"] == "fam:ecb-rate-decisions"
    assert result["probability"] == 0.83
    assert result["jev_confidence"] == 0.71
    assert result["method"] == "jev"
    assert result["gate_log"][0]["status"] == "ok"


def test_mints_when_top_choice_is_none() -> None:
    jev: Jev = _StubJev(
        {
            "choice": "none",
            "probabilities": {"fam:ecb-rate-decisions": 0.21, "none": 0.79},
            "confidence": 0.65,
        }
    )
    completer = _StubCompleter({"label": "ECB rate decisions", "description": "d"})
    result = run_classify_family(_request(), jev, lambda: completer)
    assert result["decision"] == "minted"
    assert result["label"] == "ECB rate decisions"
    assert result["method"] == "jev+mint"
    assert result["probability"] == 0.79


def test_mints_when_top_choice_probability_is_below_threshold() -> None:
    jev: Jev = _StubJev(
        {
            "choice": "fam:ecb-rate-decisions",
            "probabilities": {"fam:ecb-rate-decisions": 0.4, "none": 0.6},
            "confidence": 0.3,
        }
    )
    completer = _StubCompleter({"label": "L", "description": "D"})
    result = run_classify_family(_request(), jev, lambda: completer)
    assert result["decision"] == "minted"
    assert result["method"] == "jev+mint"


def test_skips_straight_to_minting_when_jev_is_none() -> None:
    completer = _StubCompleter({"label": "L", "description": "D"})
    result = run_classify_family(_request(), None, lambda: completer)
    assert result["decision"] == "minted"
    assert result["method"] == "mint"
    assert result["probability"] is None
    assert result["jev_confidence"] is None
    assert result["gate_log"][0]["status"] == "skipped"


def test_skips_straight_to_minting_when_there_are_no_live_families() -> None:
    completer = _StubCompleter({"label": "L", "description": "D"})
    jev: Jev = _StubJev({"choice": "none"})
    result = run_classify_family(_request(families=[]), jev, lambda: completer)
    assert result["decision"] == "minted"
    assert result["method"] == "mint"
    assert result["gate_log"][0]["detail"] == "no live families"


def test_skips_straight_to_minting_when_jev_errors() -> None:
    jev: Jev = _StubJev(error=True)
    completer = _StubCompleter({"label": "L", "description": "D"})
    result = run_classify_family(_request(), jev, lambda: completer)
    assert result["decision"] == "minted"
    assert result["method"] == "mint"
    assert result["gate_log"][0]["status"] == "failed"


def test_returns_none_failed_when_minting_also_fails() -> None:
    completer = _StubCompleter(error=True)
    result = run_classify_family(_request(), None, lambda: completer)
    assert result == {
        "decision": "none",
        "method": "failed",
        "gate_log": [
            {
                "gate": "family_routing",
                "status": "skipped",
                "detail": "no TYPESAFE_API_KEY",
            },
            {"gate": "family_mint", "status": "failed", "detail": "mint failed"},
        ],
    }


def test_returns_none_failed_when_mint_response_missing_label() -> None:
    completer = _StubCompleter({"description": "D"})  # no "label" key
    result = run_classify_family(_request(), None, lambda: completer)
    assert result["decision"] == "none"
    assert result["method"] == "failed"


def test_returns_none_failed_when_mint_response_has_empty_label() -> None:
    completer = _StubCompleter({"label": "  ", "description": "D"})
    result = run_classify_family(_request(), None, lambda: completer)
    assert result["decision"] == "none"
    assert result["method"] == "failed"
