"""Tests for iw_research.cli: subcommand dispatch and fixture-mode pipelines."""

import io
import logging
from pathlib import Path

import orjson
import pytest

from iw_research.cli import main

QUESTION = (
    "Can export controls durably slow China's access to advanced semiconductor "
    "manufacturing capability?"
)


def _run(monkeypatch: pytest.MonkeyPatch, argv: list[str], stdin_obj: object) -> int:
    stdin = io.TextIOWrapper(io.BytesIO(orjson.dumps(stdin_obj)))
    monkeypatch.setattr("sys.stdin", stdin)
    return main(argv)


def test_research_fixture_mode_matches_committed_payload(
    monkeypatch: pytest.MonkeyPatch,
    fixture_dir: Path,
    payload_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = _run(
        monkeypatch,
        ["research", "--fixture-dir", str(fixture_dir)],
        {"question": QUESTION},
    )

    assert exit_code == 0
    captured = capsys.readouterr()
    assert orjson.loads(captured.out) == orjson.loads(payload_path.read_bytes())


def test_research_stdout_contains_only_the_json_document(
    monkeypatch: pytest.MonkeyPatch,
    fixture_dir: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = _run(
        monkeypatch,
        ["research", "--fixture-dir", str(fixture_dir)],
        {"question": QUESTION},
    )

    assert exit_code == 0
    captured = capsys.readouterr()
    # The entire stdout stream must parse as exactly one JSON document, with
    # nothing else interleaved in it.
    orjson.loads(captured.out)


def test_research_missing_fixture_dir_returns_1_and_logs_error(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    missing = tmp_path / "does-not-exist"
    with caplog.at_level(logging.ERROR):
        exit_code = _run(
            monkeypatch,
            ["research", "--fixture-dir", str(missing)],
            {"question": QUESTION},
        )

    assert exit_code == 1
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_research_invalid_stdin_json_returns_1_and_logs_error(
    monkeypatch: pytest.MonkeyPatch, fixture_dir: Path, caplog: pytest.LogCaptureFixture
) -> None:
    stdin = io.TextIOWrapper(io.BytesIO(b"not json"))
    monkeypatch.setattr("sys.stdin", stdin)
    with caplog.at_level(logging.ERROR):
        exit_code = main(["research", "--fixture-dir", str(fixture_dir)])

    assert exit_code == 1
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_research_missing_question_returns_1_and_logs_error(
    monkeypatch: pytest.MonkeyPatch, fixture_dir: Path, caplog: pytest.LogCaptureFixture
) -> None:
    with caplog.at_level(logging.ERROR):
        exit_code = _run(
            monkeypatch, ["research", "--fixture-dir", str(fixture_dir)], {}
        )

    assert exit_code == 1
    assert any(record.levelno == logging.ERROR for record in caplog.records)


def test_classify_family_fixture_mode_matches_via_jev(
    monkeypatch: pytest.MonkeyPatch,
    fixture_dir: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = _run(
        monkeypatch,
        ["classify-family", "--fixture-dir", str(fixture_dir)],
        {
            "question": (
                "Will the ECB cut its deposit rate at the October 2026 meeting?"
            ),
            "families": [
                {
                    "id": "fam:ecb-rate-decisions",
                    "label": "ECB rate decisions",
                    "description": (
                        "Questions on ECB monetary-policy decisions and rate paths"
                    ),
                }
            ],
        },
    )

    assert exit_code == 0
    result = orjson.loads(capsys.readouterr().out)
    assert result["decision"] == "matched"
    assert result["family_id"] == "fam:ecb-rate-decisions"
    assert result["probability"] == pytest.approx(0.83)
    assert result["method"] == "jev"


def test_classify_family_fixture_mode_mints_when_jev_says_none(
    monkeypatch: pytest.MonkeyPatch,
    fixture_dir: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = _run(
        monkeypatch,
        ["classify-family", "--fixture-dir", str(fixture_dir)],
        {
            "question": "Will Brent crude settle above $90 in December 2026?",
            "families": [
                {
                    "id": "fam:ecb-rate-decisions",
                    "label": "ECB rate decisions",
                    "description": (
                        "Questions on ECB monetary-policy decisions and rate paths"
                    ),
                }
            ],
        },
    )

    assert exit_code == 0
    result = orjson.loads(capsys.readouterr().out)
    assert result["decision"] == "minted"
    assert result["label"] == "Brent crude monthly settlement"
    assert result["method"] == "jev+mint"


def test_classify_family_fixture_mode_with_no_families_mints_directly(
    monkeypatch: pytest.MonkeyPatch,
    fixture_dir: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    exit_code = _run(
        monkeypatch,
        ["classify-family", "--fixture-dir", str(fixture_dir)],
        {
            "question": "Will Brent crude settle above $90 in December 2026?",
            "families": [],
        },
    )

    assert exit_code == 0
    result = orjson.loads(capsys.readouterr().out)
    assert result["decision"] == "minted"
    assert result["method"] == "mint"
