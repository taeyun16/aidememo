"""Decision-pattern detector tests.

The detector backs the opt-in capture adapter; precision matters more than
recall because false positives clutter the wiki.
"""

from __future__ import annotations

from hermes_aidememo.decisions import detect


def test_explicit_decision_marker():
    text = "Decision: use HNSW as the default semantic index"
    out = detect(text)
    assert len(out) == 1
    assert "HNSW" in out[0].content
    assert out[0].fact_type == "decision"


def test_we_decided_phrasing():
    text = "Earlier we decided to ship the rebuild cache as a Tier 8 follow-up."
    out = detect(text)
    assert any(d.fact_type == "decision" for d in out)
    assert any("rebuild cache" in d.content.lower() for d in out)


def test_korean_decision_phrasing():
    text = "결정: HNSW를 기본 인덱스로 채택"
    out = detect(text)
    assert len(out) == 1
    assert out[0].fact_type == "decision"


def test_korean_inline_decision():
    text = "이번엔 model2vec을 default로 채택하기로 결정했어"
    out = detect(text)
    assert any(d.fact_type == "decision" for d in out)


def test_convention_marker():
    text = "Convention: lints run on every commit"
    out = detect(text)
    assert any(d.fact_type == "convention" for d in out)


def test_low_confidence_phrasings_below_floor_are_dropped():
    # "Always do X" sits at 0.7 weight — at default floor 0.8, dropped.
    text = "Always run cargo fmt before committing"
    assert detect(text) == []
    # Lowering the floor surfaces it.
    relaxed = detect(text, confidence_floor=0.6)
    assert any("cargo fmt" in d.content for d in relaxed)


def test_no_false_positives_on_exposition():
    text = (
        "I'm going to think about how we should approach this; "
        "the plan involves looking at the index implementation "
        "and possibly tweaking ef_construction."
    )
    assert detect(text) == []


def test_dedupe_repeated_decisions():
    text = (
        "Decision: use HNSW as the default semantic index\n"
        "Decision: use HNSW as the default semantic index"
    )
    assert len(detect(text)) == 1


def test_dedupe_collapses_user_assistant_echo_with_formatting():
    """Real-world case from the e2e run: user types a decision in
    plain text, the assistant echoes it back with markdown
    backticks and a trailing period. Only one fact should land."""
    text = (
        "결정: 한국어 패턴도 auto_record off 모드에서 즉시 aidememo에 기록한다\n"
        "결론: 한국어 패턴도 `auto_record off` 모드에서 즉시 aidememo에 기록한다."
    )
    out = detect(text)
    assert len(out) == 1, [d.content for d in out]


def test_dedupe_handles_collapsed_whitespace_difference():
    text = (
        "Decision: ship the rebuild cache as a Tier 8 follow-up\n"
        "Decision: ship  the   rebuild   cache as a Tier 8 follow-up"
    )
    assert len(detect(text)) == 1


def test_minimum_length_filter():
    # Even an explicit marker won't fire if the payload is too short.
    text = "Decision: yes"
    assert detect(text) == []


def test_maximum_length_filter():
    payload = "x" * 250
    text = f"Decision: {payload}"
    assert detect(text) == []
