"""Second-pass hints from the topic of what was said."""
from voiceweave_worker.topics import LIMIT, parse, topic_terms, with_topic

TOPICS = [
    ["水星", "金星", "火星", "木星"],
    ["天球", "子午線", "南中", "黄道", "赤経", "赤緯", "春分点"],
    ["プラネタリウム", "投影機", "投映", "ドーム"],
]


def test_a_topic_named_in_the_text_gives_its_other_terms():
    assert topic_terms("設計と石油を使うと、天球上の星の位置を表せます。", TOPICS) == [
        "子午線", "南中", "黄道", "赤経", "赤緯", "春分点"]


def test_no_topic_no_terms_and_known_hints_are_left_out():
    assert topic_terms("明日の会議は四時からです", TOPICS) == []
    assert topic_terms("天球の話", TOPICS, known="赤経、赤緯") == ["子午線", "南中", "黄道", "春分点"]


def test_topics_with_more_matches_come_first_within_the_limit():
    terms = topic_terms("木星と金星、それから天球", TOPICS)
    assert terms[:2] == ["水星", "火星"]
    long_topic = [["天球"] + [f"用語{i:03d}" for i in range(100)]]
    picked = topic_terms("天球", long_topic)
    assert sum(len(term) + 1 for term in picked) <= LIMIT


def test_requests_are_checked_and_the_prompt_keeps_the_context_last():
    assert parse(None) == [] and parse("天球") == []
    assert parse([["天球", 3, " "], [], "x"]) == [["天球"]]
    assert with_topic("VoiceWeave", ["赤経"], "前の文") == "VoiceWeave、赤経、前の文"
    assert with_topic("", ["赤経", "赤緯"], "") == "赤経、赤緯"
