"""Pydantic request models for the worker's stdin JSON (contracts.md A1/A3)."""

import os
from typing import Literal

from pydantic import BaseModel, Field

_DEFAULT_PROVIDERS_WITH_KEY: tuple[str, ...] = (
    "asknews_news",
    "google_news",
    "bing_news",
    "wikipedia",
)
_DEFAULT_PROVIDERS_WITHOUT_KEY: tuple[str, ...] = (
    "google_news",
    "bing_news",
    "wikipedia",
    "arxiv",
)

ProviderName = Literal[
    "wikipedia",
    "arxiv",
    "asknews_news",
    "asknews_wiki",
    "google_news",
    "bing_news",
]


def default_providers() -> list[str]:
    """The `research` request's default `providers` list (contracts.md A1).

    Free news sources are blended into every research; `asknews_news` is
    added only when it can be billed. `asknews_wiki` remains a valid provider
    name for explicit requests but is not a default.

    Returns:
        `["asknews_news", "google_news", "bing_news", "wikipedia"]` if
        `ASKNEWS_API_KEY` is set, else
        `["google_news", "bing_news", "wikipedia", "arxiv"]`.
    """
    if os.environ.get("ASKNEWS_API_KEY"):
        return list(_DEFAULT_PROVIDERS_WITH_KEY)
    return list(_DEFAULT_PROVIDERS_WITHOUT_KEY)


class QuestionContext(BaseModel):
    """Optional question context, shared by both subcommands (A1/A3)."""

    resolution_criteria: str | None = None
    fine_print: str | None = None
    background: str | None = None


class FamilyRef(BaseModel):
    """The `research` request's optional current-family context (A1).

    Accepted for forward compatibility and logging; Rust computes
    `news_since` from the family's `last_seen` before calling the worker
    (contracts.md C2), so this field does not otherwise steer extraction.
    """

    id: str
    label: str
    last_seen: str | None = None


class FamilyOption(BaseModel):
    """One live family offered to `classify-family` (A3)."""

    id: str
    label: str
    description: str


class ResearchRequest(BaseModel):
    """The `research` subcommand's stdin request (contracts.md A1)."""

    question: str
    question_id: str | None = None
    context: QuestionContext | None = None
    family: FamilyRef | None = None
    providers: list[ProviderName] | None = None
    # Part of the request contract (A1); no provider reads it now that AskNews
    # runs fixed-window `latest news` searches.
    news_since: str | None = None
    max_news: int = 12
    max_wiki: int = 3
    extra_queries: list[str] = Field(default_factory=list)


class ClassifyFamilyRequest(BaseModel):
    """The `classify-family` subcommand's stdin request (contracts.md A3)."""

    question: str
    context: QuestionContext | None = None
    families: list[FamilyOption] = Field(default_factory=list)
