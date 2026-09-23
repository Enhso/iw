"""Tests for iw_research.jev: JevClient, FixtureJevClient, env constructors."""

from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from iw_research.jev import (
    JEV_URL,
    ChoiceQuestion,
    FixtureJevClient,
    JevClient,
    JevError,
    NoulQuestion,
    ScoreQuestion,
    jev_client_from_env,
    jev_fixture_client_from_dir,
)


def test_noul_question_to_json_omits_criteria_when_absent() -> None:
    q = NoulQuestion(instructions="Is this urgent?")
    assert q.to_json() == {"type": "noul", "instructions": "Is this urgent?"}


def test_noul_question_to_json_includes_criteria_when_present() -> None:
    q = NoulQuestion(
        instructions="Is this urgent?", criteria={"true": "yes", "false": "no"}
    )
    assert q.to_json() == {
        "type": "noul",
        "instructions": "Is this urgent?",
        "criteria": {"true": "yes", "false": "no"},
    }


def test_choice_question_to_json() -> None:
    q = ChoiceQuestion(instructions="Pick one", criteria={"a": "A", "b": None})
    assert q.to_json() == {
        "type": "choice",
        "instructions": "Pick one",
        "criteria": {"a": "A", "b": None},
    }


def test_score_question_to_json() -> None:
    q = ScoreQuestion(instructions="Rate it", criteria=["low", "high"])
    assert q.to_json() == {
        "type": "score",
        "instructions": "Rate it",
        "criteria": ["low", "high"],
    }


def test_ask_sends_bearer_auth_and_expected_body(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=JEV_URL,
        method="POST",
        match_headers={"Authorization": "Bearer secret"},
        match_json={
            "state": "hello",
            "model": "jev-latest",
            "questions": {"q": {"type": "noul", "instructions": "Is this a greeting?"}},
        },
        json={"model": "jev-1.13.0", "answers": {"q": {"type": "noul", "noul": 0.9}}},
    )
    client = JevClient(api_key="secret")
    answers = client.ask(
        "hello", {"q": NoulQuestion(instructions="Is this a greeting?")}
    )
    assert answers == {"q": {"type": "noul", "noul": 0.9}}


def test_ask_raises_on_missing_answers_field(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(url=JEV_URL, json={"model": "jev-1.13.0"})
    client = JevClient(api_key="k")
    with pytest.raises(JevError, match="missing answers"):
        client.ask("s", {})


def test_ask_raises_when_answers_is_not_an_object(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=JEV_URL, json={"model": "m", "answers": "not an object"}
    )
    client = JevClient(api_key="k")
    with pytest.raises(JevError, match="answers is not an object"):
        client.ask("s", {})


def test_ask_retries_once_on_429_then_succeeds(
    httpx_mock: HTTPXMock, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("iw_research.jev.time.sleep", lambda _seconds: None)
    httpx_mock.add_response(url=JEV_URL, status_code=429, headers={"retry-after": "0"})
    httpx_mock.add_response(url=JEV_URL, json={"model": "m", "answers": {}})
    client = JevClient(api_key="k")
    assert client.ask("s", {}) == {}


def test_ask_retries_once_on_5xx_then_succeeds(
    httpx_mock: HTTPXMock, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("iw_research.jev.time.sleep", lambda _seconds: None)
    httpx_mock.add_response(url=JEV_URL, status_code=503)
    httpx_mock.add_response(url=JEV_URL, json={"model": "m", "answers": {}})
    client = JevClient(api_key="k")
    assert client.ask("s", {}) == {}


def test_ask_raises_after_exhausting_the_single_retry(
    httpx_mock: HTTPXMock, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("iw_research.jev.time.sleep", lambda _seconds: None)
    httpx_mock.add_response(url=JEV_URL, status_code=429, is_reusable=True)
    client = JevClient(api_key="k")
    with pytest.raises(JevError):
        client.ask("s", {})


def test_ask_raises_immediately_on_non_retryable_status(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(url=JEV_URL, status_code=422)
    client = JevClient(api_key="k")
    with pytest.raises(JevError):
        client.ask("s", {})


def test_ask_raises_on_transport_error() -> None:
    def raise_transport_error(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("boom", request=request)

    transport = httpx.MockTransport(raise_transport_error)
    client = JevClient(api_key="k", client=httpx.Client(transport=transport))
    with pytest.raises(JevError, match="jev request failed"):
        client.ask("s", {})


def test_fixture_jev_client_keys_by_passage_title() -> None:
    client = FixtureJevClient(
        {"Some Title": {"relevant": {"type": "noul", "noul": 0.8}}}
    )
    state = {"question": "Q?", "passage": {"title": "Some Title", "text": "..."}}
    assert client.ask(state, {}) == {"relevant": {"type": "noul", "noul": 0.8}}


def test_fixture_jev_client_keys_by_claim_text() -> None:
    client = FixtureJevClient({"A claim": {"support": {"type": "score", "score": 3.0}}})
    state = {"claim": "A claim", "excerpts": ["x"]}
    assert client.ask(state, {}) == {"support": {"type": "score", "score": 3.0}}


def test_fixture_jev_client_keys_by_question() -> None:
    client = FixtureJevClient({"Q?": {"family": {"type": "choice", "choice": "none"}}})
    assert client.ask({"question": "Q?"}, {}) == {
        "family": {"type": "choice", "choice": "none"}
    }


def test_fixture_jev_client_raises_on_missing_key() -> None:
    client = FixtureJevClient({})
    with pytest.raises(JevError, match="no fixture jev answer"):
        client.ask({"question": "unknown"}, {})


def test_fixture_jev_client_raises_when_state_has_no_known_field() -> None:
    client = FixtureJevClient({})
    with pytest.raises(JevError, match="cannot derive fixture key"):
        client.ask({"unrelated": "value"}, {})


def test_jev_client_from_env_returns_none_without_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("TYPESAFE_API_KEY", raising=False)
    assert jev_client_from_env() is None


def test_jev_client_from_env_defaults_to_jev_latest(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TYPESAFE_API_KEY", "k")
    monkeypatch.delenv("JEV_MODEL", raising=False)
    client = jev_client_from_env()
    assert isinstance(client, JevClient)
    assert client._model == "jev-latest"  # noqa: SLF001


def test_jev_client_from_env_honors_jev_model_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TYPESAFE_API_KEY", "k")
    monkeypatch.setenv("JEV_MODEL", "jev-1.13")
    client = jev_client_from_env()
    assert isinstance(client, JevClient)
    assert client._model == "jev-1.13"  # noqa: SLF001


def test_jev_fixture_client_from_dir_loads_answers_json(tmp_path: Path) -> None:
    (tmp_path / "jev_answers.json").write_text('{"Q?": {"family": {"noul": 1}}}')
    client = jev_fixture_client_from_dir(tmp_path)
    assert client.ask({"question": "Q?"}, {}) == {"family": {"noul": 1}}
