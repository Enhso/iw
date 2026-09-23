"""Tests for iw_research.llm: ChatClient, FixtureChatClient, client_from_env."""

import logging
from pathlib import Path

import orjson
import pytest
from pytest_httpx import HTTPXMock

from iw_research.llm import (
    ChatClient,
    FallbackChatClient,
    FixtureChatClient,
    LlmError,
    client_from_env,
)

BASE_URL = "https://example.com/v1"


def test_complete_json_sends_bearer_auth_and_expected_body(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions",
        method="POST",
        match_headers={"Authorization": "Bearer secret-token"},
        match_json={
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "sys prompt"},
                {"role": "user", "content": "user prompt"},
            ],
            "temperature": 0,
            "response_format": {"type": "json_object"},
        },
        json={"choices": [{"message": {"content": "{}"}}]},
    )
    client = ChatClient(base_url=BASE_URL, api_key="secret-token", model="gpt-test")
    assert client.complete_json("sys prompt", "user prompt") == {}


def test_complete_json_parses_plain_json_body(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions",
        json={"choices": [{"message": {"content": '{"a": 1}'}}]},
    )
    client = ChatClient(base_url=BASE_URL, api_key="k", model="m")
    assert client.complete_json("sys", "user") == {"a": 1}


def test_complete_json_strips_json_code_fence(httpx_mock: HTTPXMock) -> None:
    content = '```json\n{"a": 2}\n```'
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions",
        json={"choices": [{"message": {"content": content}}]},
    )
    client = ChatClient(base_url=BASE_URL, api_key="k", model="m")
    assert client.complete_json("sys", "user") == {"a": 2}


def test_complete_json_raises_llm_error_on_http_failure(httpx_mock: HTTPXMock) -> None:
    httpx_mock.add_response(url=f"{BASE_URL}/chat/completions", status_code=500)
    client = ChatClient(base_url=BASE_URL, api_key="k", model="m")
    with pytest.raises(LlmError):
        client.complete_json("sys", "user")


def test_complete_json_raises_llm_error_on_malformed_content(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions",
        json={"choices": [{"message": {"content": "not json"}}]},
    )
    client = ChatClient(base_url=BASE_URL, api_key="k", model="m")
    with pytest.raises(LlmError):
        client.complete_json("sys", "user")


def test_api_key_never_appears_in_logged_error(
    httpx_mock: HTTPXMock, caplog: pytest.LogCaptureFixture
) -> None:
    httpx_mock.add_response(url=f"{BASE_URL}/chat/completions", status_code=500)
    client = ChatClient(base_url=BASE_URL, api_key="super-secret-key", model="m")
    logger = logging.getLogger("iw_research.test")
    with caplog.at_level(logging.ERROR):
        try:
            client.complete_json("sys", "user")
        except LlmError as exc:
            logger.error("call failed: %s", exc, exc_info=True)
    assert "super-secret-key" not in caplog.text


def test_client_from_env_raises_naming_missing_variables(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("LLM_API_KEY", raising=False)
    monkeypatch.delenv("LLM_MODEL", raising=False)
    with pytest.raises(LlmError) as exc_info:
        client_from_env()
    assert "LLM_API_KEY" in str(exc_info.value)
    assert "LLM_MODEL" in str(exc_info.value)


def test_client_from_env_builds_client_when_env_present(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("LLM_API_KEY", "k")
    monkeypatch.setenv("LLM_MODEL", "m")
    monkeypatch.delenv("LLM_API_BASE", raising=False)
    client = client_from_env()
    assert isinstance(client, ChatClient)


def test_fixture_chat_client_returns_file_content_regardless_of_input(
    tmp_path: Path,
) -> None:
    path = tmp_path / "fixture.json"
    path.write_bytes(orjson.dumps({"hello": "world"}))
    client = FixtureChatClient(path)
    assert client.complete_json("ignored", "also ignored") == {"hello": "world"}
    assert client.complete_json("different", "again") == {"hello": "world"}


def test_client_from_env_returns_fallback_chat_client_for_comma_separated_models(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("LLM_API_KEY", "k")
    monkeypatch.setenv("LLM_MODEL", "google/gemma-4-31b-it:free, openai/gpt-6-luna")
    client = client_from_env()
    assert isinstance(client, FallbackChatClient)


@pytest.mark.parametrize(
    "model",
    [
        "anthropic/claude-opus-4",
        "anthropic/claude-sonnet-5",
        "anthropic/claude-fable-5-1",
        "openai/gpt-6-astra",
        "openai/gpt-6-sol",
        "openai/gpt-5.5-turbo",
        "some-vendor/model-pro",
    ],
)
def test_client_from_env_refuses_frontier_models_in_the_chain(
    model: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LLM_API_KEY", "k")
    monkeypatch.setenv("LLM_MODEL", f"google/gemma-4-31b-it:free,{model}")
    with pytest.raises(LlmError) as exc_info:
        client_from_env()
    assert model in str(exc_info.value)


def test_fallback_chat_client_tries_next_model_on_failure(
    httpx_mock: HTTPXMock,
) -> None:
    # pytest-httpx serves matching responses in registration order, so the
    # first request (from `failing`) consumes the 500 and the second (from
    # `succeeding`) gets the JSON response.
    httpx_mock.add_response(url=f"{BASE_URL}/chat/completions", status_code=500)
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions",
        json={"choices": [{"message": {"content": "{}"}}]},
    )
    failing = ChatClient(base_url=BASE_URL, api_key="k", model="m1")
    succeeding = ChatClient(base_url=BASE_URL, api_key="k", model="m2")
    chain = FallbackChatClient([failing, succeeding])
    assert chain.complete_json("sys", "user") == {}


def test_fallback_chat_client_raises_last_error_when_every_model_fails(
    httpx_mock: HTTPXMock,
) -> None:
    httpx_mock.add_response(
        url=f"{BASE_URL}/chat/completions", status_code=500, is_reusable=True
    )
    chain = FallbackChatClient(
        [
            ChatClient(base_url=BASE_URL, api_key="k", model="m1"),
            ChatClient(base_url=BASE_URL, api_key="k", model="m2"),
        ]
    )
    with pytest.raises(LlmError):
        chain.complete_json("sys", "user")


def test_fallback_chat_client_rejects_empty_chain() -> None:
    with pytest.raises(LlmError):
        FallbackChatClient([])
