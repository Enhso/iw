"""OpenAI-compatible chat-completions client, plus a fixture stand-in."""

import os
from pathlib import Path
from typing import Protocol

import httpx
import orjson


class LlmError(Exception):
    """Raised when a chat-completions request or its response fails."""


class Completer(Protocol):
    """Anything that can turn a system/user prompt pair into JSON."""

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        """Return a parsed JSON object for the given prompts."""
        ...


def _parse_json_content(content: str) -> dict[str, object]:
    """Parse a chat completion's message content as a JSON object.

    Args:
        content: The raw message content, optionally wrapped in a
            ```json fenced code block.

    Returns:
        The parsed JSON object.

    Raises:
        LlmError: If the content is not valid JSON or not a JSON object.
    """
    stripped = content.strip()
    if stripped.startswith("```"):
        lines = stripped.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].startswith("```"):
            lines = lines[:-1]
        stripped = "\n".join(lines)
    try:
        result = orjson.loads(stripped)
    except orjson.JSONDecodeError as exc:
        raise LlmError(f"chat completion content is not valid JSON: {exc}") from exc
    if not isinstance(result, dict):
        raise LlmError("chat completion content is not a JSON object")
    return result


class ChatClient:
    """OpenAI-compatible chat-completions client that always asks for JSON."""

    def __init__(
        self,
        base_url: str,
        api_key: str,
        model: str,
        client: httpx.Client | None = None,
        timeout: float = 120.0,
    ) -> None:
        """Configure a chat-completions client.

        Args:
            base_url: The API base url, e.g. `https://api.openai.com/v1`.
            api_key: Bearer token sent in the `Authorization` header.
            model: Model name to request.
            client: An existing `httpx.Client` to reuse, or `None` to
                create one scoped to `timeout`.
            timeout: Request timeout in seconds, used only when `client`
                is `None`.
        """
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._model = model
        self._client = client if client is not None else httpx.Client(timeout=timeout)

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        """Request a JSON chat completion and parse its content.

        Args:
            system: The system prompt.
            user: The user prompt.

        Returns:
            The parsed JSON object from the completion's message content.

        Raises:
            LlmError: If the request fails or the response cannot be
                parsed as a JSON object.
        """
        try:
            response = self._client.post(
                f"{self._base_url}/chat/completions",
                headers={"Authorization": f"Bearer {self._api_key}"},
                json={
                    "model": self._model,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user},
                    ],
                    "temperature": 0,
                    "response_format": {"type": "json_object"},
                },
            )
            response.raise_for_status()
        except httpx.HTTPError as exc:
            raise LlmError(f"chat completion request failed: {exc}") from exc

        try:
            content = response.json()["choices"][0]["message"]["content"]
        except (KeyError, IndexError, TypeError) as exc:
            raise LlmError(f"chat completion response malformed: {exc}") from exc
        return _parse_json_content(content)


class FixtureChatClient:
    """A `Completer` that returns a fixture file's JSON regardless of input."""

    def __init__(self, path: Path) -> None:
        """Store the fixture file path.

        Args:
            path: Path to a JSON file containing the fixture LLM output.
        """
        self._path = path

    def complete_json(self, system: str, user: str) -> dict[str, object]:
        """Return the fixture file's parsed JSON content.

        Args:
            system: Ignored; present to satisfy the `Completer` protocol.
            user: Ignored; present to satisfy the `Completer` protocol.

        Returns:
            The parsed JSON object read from the fixture file.

        Raises:
            LlmError: If the fixture file's content is not a JSON object.
        """
        result = orjson.loads(self._path.read_bytes())
        if not isinstance(result, dict):
            raise LlmError(f"fixture {self._path} does not contain a JSON object")
        return result


def client_from_env() -> ChatClient:
    """Build a `ChatClient` from `LLM_API_BASE`, `LLM_API_KEY`, `LLM_MODEL`.

    Returns:
        A configured `ChatClient`.

    Raises:
        LlmError: If `LLM_API_KEY` or `LLM_MODEL` is unset, naming the
            missing variable(s).
    """
    base_url = os.environ.get("LLM_API_BASE", "https://api.openai.com/v1")
    api_key = os.environ.get("LLM_API_KEY")
    model = os.environ.get("LLM_MODEL")

    missing = [
        name
        for name, value in (("LLM_API_KEY", api_key), ("LLM_MODEL", model))
        if not value
    ]
    if missing:
        raise LlmError(
            f"missing required environment variable(s): {', '.join(missing)}"
        )

    return ChatClient(base_url=base_url, api_key=api_key or "", model=model or "")
