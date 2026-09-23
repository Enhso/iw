"""Tests for iw_research.request: request models and default_providers."""

import pytest
from pydantic import ValidationError

from iw_research.request import (
    ClassifyFamilyRequest,
    FamilyOption,
    QuestionContext,
    ResearchRequest,
    default_providers,
)


def test_default_providers_without_asknews_key(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ASKNEWS_API_KEY", raising=False)
    assert default_providers() == ["wikipedia", "arxiv"]


def test_default_providers_with_asknews_key(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ASKNEWS_API_KEY", "k")
    assert default_providers() == ["asknews_news", "asknews_wiki"]


def test_research_request_only_requires_question() -> None:
    request = ResearchRequest.model_validate({"question": "Q?"})
    assert request.question == "Q?"
    assert request.question_id is None
    assert request.context is None
    assert request.family is None
    assert request.providers is None
    assert request.news_since is None
    assert request.max_news == 12
    assert request.max_wiki == 3


def test_research_request_rejects_unknown_provider() -> None:
    with pytest.raises(ValidationError):
        ResearchRequest.model_validate({"question": "Q?", "providers": ["bing"]})


def test_research_request_parses_full_payload() -> None:
    request = ResearchRequest.model_validate(
        {
            "question": "Q?",
            "question_id": "metaculus:1",
            "context": {
                "resolution_criteria": "c",
                "fine_print": "f",
                "background": "b",
            },
            "family": {
                "id": "fam:x",
                "label": "X",
                "last_seen": "2026-09-10T08:00:00Z",
            },
            "providers": ["asknews_news", "asknews_wiki"],
            "news_since": "2026-09-10T08:00:00Z",
            "max_news": 5,
            "max_wiki": 1,
        }
    )
    assert request.question_id == "metaculus:1"
    assert isinstance(request.context, QuestionContext)
    assert request.context.resolution_criteria == "c"
    assert request.family is not None
    assert request.family.id == "fam:x"
    assert request.providers == ["asknews_news", "asknews_wiki"]
    assert request.max_news == 5
    assert request.max_wiki == 1


def test_classify_family_request_defaults_families_to_empty_list() -> None:
    request = ClassifyFamilyRequest.model_validate({"question": "Q?"})
    assert request.families == []


def test_classify_family_request_parses_families() -> None:
    request = ClassifyFamilyRequest.model_validate(
        {
            "question": "Q?",
            "families": [{"id": "fam:x", "label": "X", "description": "d"}],
        }
    )
    assert request.families == [FamilyOption(id="fam:x", label="X", description="d")]
