"""Tests for iw_research.normalize: cleaning and de-duplicating documents."""

from iw_research.normalize import MAX_DOC_CHARS, normalize, to_documents, to_sources
from iw_research.schema import content_sha256
from iw_research.sources import SourceDocument


def _doc(**overrides: object) -> SourceDocument:
    defaults: dict[str, object] = {
        "id": "src:one",
        "provider": "wikipedia",
        "title": "Title",
        "url": "https://example.com/one",
        "published": "",
        "retrieved_at": "2026-09-14T00:00:00Z",
        "text": "Some text.",
    }
    defaults.update(overrides)
    return SourceDocument(**defaults)  # type: ignore[arg-type]


def test_normalize_drops_rows_with_empty_text_after_strip() -> None:
    docs = [
        _doc(id="src:a", url="https://a", text="   "),
        _doc(id="src:b", url="https://b"),
    ]
    frame = normalize(docs)
    assert frame["id"].to_list() == ["src:b"]


def test_normalize_dedupes_on_url_keeping_first() -> None:
    docs = [
        _doc(id="src:a", url="https://dup", title="First", text="first"),
        _doc(id="src:b", url="https://dup", title="Second", text="second"),
    ]
    frame = normalize(docs)
    assert len(frame) == 1
    assert frame["id"].to_list() == ["src:a"]
    assert frame["text"].to_list() == ["first"]


def test_normalize_truncates_text_to_max_doc_chars() -> None:
    long_text = "x" * (MAX_DOC_CHARS + 500)
    docs = [_doc(id="src:a", url="https://a", text=long_text)]
    frame = normalize(docs)
    assert len(frame["text"][0]) == MAX_DOC_CHARS


def test_normalize_sorts_by_provider_then_title() -> None:
    docs = [
        _doc(id="src:a", url="https://a", provider="wikipedia", title="Zebra"),
        _doc(id="src:b", url="https://b", provider="arxiv", title="Zeta"),
        _doc(id="src:c", url="https://c", provider="arxiv", title="Alpha"),
    ]
    frame = normalize(docs)
    assert frame["id"].to_list() == ["src:c", "src:b", "src:a"]


def test_to_documents_round_trips_normalize_output() -> None:
    docs = [_doc(id="src:a", url="https://a"), _doc(id="src:b", url="https://b")]
    frame = normalize(docs)
    round_tripped = to_documents(frame)
    assert {d.id for d in round_tripped} == {"src:a", "src:b"}
    assert all(isinstance(d, SourceDocument) for d in round_tripped)


def test_to_sources_builds_source_rows_with_matching_fields() -> None:
    docs = [
        _doc(
            id="src:a",
            url="https://a",
            title="A Title",
            provider="arxiv",
            published="2024-01-01",
            text="Some text.",
        )
    ]
    frame = normalize(docs)
    sources = to_sources(frame)
    assert len(sources) == 1
    source = sources[0]
    assert source.id == "src:a"
    assert source.title == "A Title"
    assert source.url == "https://a"
    assert source.provider == "arxiv"
    assert source.published == "2024-01-01"
    assert source.retrieved_at == "2026-09-14T00:00:00Z"
    assert source.content == "Some text."
    assert source.content_hash == content_sha256("Some text.")
