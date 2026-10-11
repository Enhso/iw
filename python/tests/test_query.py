"""Tests for iw_research.sources.query: the shared keyword-query helper."""

from iw_research.sources.query import keyword_query


def test_keyword_query_drops_stopwords_and_preserves_order() -> None:
    assert (
        keyword_query("Will the ECB cut its deposit rate at the October meeting?")
        == "ecb cut deposit rate october meeting"
    )


def test_keyword_query_falls_back_to_question_when_all_stopwords() -> None:
    assert keyword_query("is it") == "is it"


def test_keyword_query_splits_punctuation_and_keeps_digits() -> None:
    assert (
        keyword_query(
            "Will Donald Trump and Vladimir Putin meet in person before "
            "December 1, 2026?"
        )
        == "donald trump vladimir putin meet person before december 1 2026"
    )
