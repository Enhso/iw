"""Tests for iw_research.cli: end-to-end fixture-mode pipeline."""

import logging
from pathlib import Path

import orjson
import pytest

from iw_research.cli import main

QUESTION = (
    "Can export controls durably slow China's access to advanced semiconductor "
    "manufacturing capability?"
)


def test_main_fixture_mode_matches_committed_payload(
    fixture_dir: Path, payload_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    exit_code = main(["--question", QUESTION, "--fixture-dir", str(fixture_dir)])

    assert exit_code == 0
    captured = capsys.readouterr()
    assert orjson.loads(captured.out) == orjson.loads(payload_path.read_bytes())


def test_main_stdout_contains_only_the_json_document(
    fixture_dir: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    exit_code = main(["--question", QUESTION, "--fixture-dir", str(fixture_dir)])

    assert exit_code == 0
    captured = capsys.readouterr()
    # The entire stdout stream must parse as exactly one JSON document, with
    # nothing else interleaved in it.
    orjson.loads(captured.out)


def test_main_missing_fixture_dir_returns_1_and_logs_error(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    missing = tmp_path / "does-not-exist"
    with caplog.at_level(logging.ERROR):
        exit_code = main(["--question", QUESTION, "--fixture-dir", str(missing)])

    assert exit_code == 1
    assert any(record.levelno == logging.ERROR for record in caplog.records)
