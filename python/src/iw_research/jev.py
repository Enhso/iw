"""HTTP client for the TypeSafe Jev evaluation endpoint (`/v1/systemone`).

See contracts.md B3. Every gate built on top of `JevClient` must fail open
(a missing key, timeout, HTTP error, or malformed answer proceeds as if the
gate did not exist); that behavior lives in `iw_research.gates`, not here.
This module only makes the call and raises `JevError` on any failure.
"""

import logging
import os
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

import httpx
import orjson

logger = logging.getLogger(__name__)

JEV_URL = "https://api.typesafe.ai/v1/systemone"
DEFAULT_MODEL = "jev-latest"
DEFAULT_TIMEOUT = 20.0
_MAX_ATTEMPTS = 2  # one request plus one retry, per contracts.md B3
_DEFAULT_RETRY_AFTER = 1.0


class JevError(Exception):
    """Raised when a Jev evaluation request fails after its retry."""


@dataclass(frozen=True)
class NoulQuestion:
    """A yes/no Jev question; see TypeSafe API reference, Noul."""

    instructions: object
    criteria: dict[str, object] | None = None

    def to_json(self) -> dict[str, object]:
        """Render this question in the `/v1/systemone` request shape."""
        body: dict[str, object] = {"type": "noul", "instructions": self.instructions}
        if self.criteria is not None:
            body["criteria"] = self.criteria
        return body


@dataclass(frozen=True)
class ChoiceQuestion:
    """A single-choice Jev question; see TypeSafe API reference, Choice."""

    instructions: object
    criteria: dict[str, object | None]

    def to_json(self) -> dict[str, object]:
        """Render this question in the `/v1/systemone` request shape."""
        return {
            "type": "choice",
            "instructions": self.instructions,
            "criteria": self.criteria,
        }


@dataclass(frozen=True)
class ScoreQuestion:
    """An ordered-rubric Jev question; see TypeSafe API reference, Score."""

    instructions: object
    criteria: list[object]

    def to_json(self) -> dict[str, object]:
        """Render this question in the `/v1/systemone` request shape."""
        return {
            "type": "score",
            "instructions": self.instructions,
            "criteria": self.criteria,
        }


Question = NoulQuestion | ChoiceQuestion | ScoreQuestion


class Jev(Protocol):
    """Anything that can evaluate a batch of Jev questions against a state."""

    def ask(
        self, state: object, questions: dict[str, Question]
    ) -> dict[str, dict[str, object]]:
        """Evaluate `questions` against `state`, returning the raw answers map."""
        ...


class JevClient:
    """Live client for one `/v1/systemone` call, with one retry on 429/5xx."""

    def __init__(
        self,
        api_key: str,
        model: str = DEFAULT_MODEL,
        client: httpx.Client | None = None,
        timeout: float = DEFAULT_TIMEOUT,
        base_url: str = JEV_URL,
    ) -> None:
        """Configure a Jev client.

        Args:
            api_key: Bearer token sent in the `Authorization` header.
            model: The Jev model to request, e.g. `"jev-latest"`.
            client: An existing `httpx.Client` to reuse, or `None` to
                create one scoped to `timeout`.
            timeout: Request timeout in seconds, used only when `client`
                is `None`.
            base_url: The `/v1/systemone` endpoint url.
        """
        self._api_key = api_key
        self._model = model
        self._base_url = base_url
        self._client = client if client is not None else httpx.Client(timeout=timeout)

    def ask(
        self, state: object, questions: dict[str, Question]
    ) -> dict[str, dict[str, object]]:
        """Evaluate `questions` against `state` in a single request.

        Args:
            state: The content to evaluate (Item B3/B4's per-gate state).
            questions: A map of caller-chosen question id to `Question`.

        Returns:
            The response's `answers` map, keyed by the same question ids.

        Raises:
            JevError: If the request fails (after one retry on 429 or a
                5xx status, honouring `retry-after`), or the response is
                missing or malformed `answers`.
        """
        body = {
            "state": state,
            "model": self._model,
            "questions": {key: q.to_json() for key, q in questions.items()},
        }
        response = self._post_with_retry(body)
        try:
            answers = response.json()["answers"]
        except (KeyError, TypeError) as exc:
            raise JevError(f"jev response malformed: missing answers: {exc}") from exc
        if not isinstance(answers, dict):
            raise JevError("jev response malformed: answers is not an object")
        return answers

    def _post_with_retry(self, body: dict[str, object]) -> httpx.Response:
        response: httpx.Response | None = None
        for attempt in range(_MAX_ATTEMPTS):
            try:
                response = self._client.post(
                    self._base_url,
                    headers={"Authorization": f"Bearer {self._api_key}"},
                    json=body,
                )
            except httpx.HTTPError as exc:
                raise JevError(f"jev request failed: {exc}") from exc

            if response.status_code == 200:
                return response
            if response.status_code == 429 or response.status_code >= 500:
                if attempt < _MAX_ATTEMPTS - 1:
                    time.sleep(_retry_after_seconds(response))
                    continue
            raise JevError(
                f"jev request failed: {response.status_code} {response.text[:200]}"
            )
        assert response is not None  # loop always runs at least once
        raise JevError(
            f"jev request failed: {response.status_code} {response.text[:200]}"
        )


def _retry_after_seconds(response: httpx.Response) -> float:
    """Read `Retry-After` (seconds) from `response`, defaulting to 1.0."""
    header = response.headers.get("retry-after")
    if header is None:
        return _DEFAULT_RETRY_AFTER
    try:
        return max(0.0, float(header))
    except ValueError:
        return _DEFAULT_RETRY_AFTER


def _state_key(state: object) -> str:
    """Derive a fixture lookup key from a gate's `state` (see `FixtureJevClient`).

    Args:
        state: The `state` passed to `ask`, one of contracts.md B4's three
            shapes: `{"question", "passage": {"title", "text"}}` (relevance
            gate, keyed by passage title), `{"claim", "excerpts"}` (support
            gate, keyed by claim text), or a family-classify state carrying
            `"question"` (keyed by question text).

    Returns:
        The lookup key.

    Raises:
        JevError: If no known field is present.
    """
    if isinstance(state, dict):
        passage = state.get("passage")
        if isinstance(passage, dict) and "title" in passage:
            return str(passage["title"])
        if "claim" in state:
            return str(state["claim"])
        if "question" in state:
            return str(state["question"])
    raise JevError(f"cannot derive fixture key from state: {state!r}")


class FixtureJevClient:
    """A `Jev` that returns canned answers loaded from a fixture file.

    Each call's lookup key is derived from its `state` by `_state_key`, so
    the fixture file the caller supplies must have one entry per distinct
    passage title, claim text, or question the fixture run will ask about.
    """

    def __init__(self, answers: dict[str, dict[str, dict[str, object]]]) -> None:
        """Store the fixture answers map.

        Args:
            answers: Lookup key (`_state_key`) -> that call's raw `answers`
                map, in the same shape a live response's `answers` field
                would have.
        """
        self._answers = answers

    def ask(
        self, state: object, questions: dict[str, Question]
    ) -> dict[str, dict[str, object]]:
        """Return the canned answers for `state`'s fixture key.

        Args:
            state: The content to evaluate; only used to derive the lookup
                key, not sent anywhere.
            questions: Ignored; present to satisfy the `Jev` protocol.

        Returns:
            The canned `answers` map for `state`'s fixture key.

        Raises:
            JevError: If no fixture entry exists for that key.
        """
        key = _state_key(state)
        try:
            return self._answers[key]
        except KeyError as exc:
            raise JevError(f"no fixture jev answer for key {key!r}") from exc


def jev_client_from_env() -> JevClient | None:
    """Build a `JevClient` from `TYPESAFE_API_KEY`/`JEV_MODEL`.

    Returns:
        A configured `JevClient`, or `None` if `TYPESAFE_API_KEY` is unset.
        Every gate in `iw_research.gates` treats `None` as "skip this gate
        and log `status: skipped`" (contracts.md B3's fail-open contract).
    """
    api_key = os.environ.get("TYPESAFE_API_KEY")
    if not api_key:
        return None
    model = os.environ.get("JEV_MODEL", DEFAULT_MODEL)
    return JevClient(api_key=api_key, model=model)


def jev_fixture_client_from_dir(fixture_dir: Path) -> FixtureJevClient:
    """Build a `FixtureJevClient` from `<fixture_dir>/jev_answers.json`.

    Args:
        fixture_dir: The offline fixture directory (e.g. `fixtures/offline`).

    Returns:
        A `FixtureJevClient` loaded from that file.
    """
    raw = orjson.loads((fixture_dir / "jev_answers.json").read_bytes())
    return FixtureJevClient(raw)
