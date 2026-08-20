use super::models::{AiSession, Chunk, Message, Role};

/// Chunks larger than this are split into overlapping windows.
pub const CHUNK_MAX_CHARS: usize = 6000;
/// Exchanges shorter than this are merged into the pending chunk, so a chunk is
/// a topic rather than a single "ok".
pub const CHUNK_MIN_CHARS: usize = 50;
/// How much text a split window repeats from the previous one, so a phrase
/// straddling a boundary still matches.
const CHUNK_OVERLAP_CHARS: usize = 1200;

/// Turn a conversation into searchable chunks, grouping each user prompt with
/// the assistant reply it produced.
pub fn chunk_session(session: &AiSession, messages: &[Message]) -> Vec<Chunk> {
    let pairs = pair_messages(messages);
    let mut chunks = Vec::new();
    let mut chunk_index = 0;

    let mut pending = String::new();
    let mut pending_ts = session.started_at;

    for (user_msg, assistant_msg) in &pairs {
        let mut pair_text = String::new();

        if let Some(u) = user_msg {
            pair_text.push_str("USER: ");
            pair_text.push_str(&u.text);
            pair_text.push('\n');
            if let Some(ts) = u.timestamp {
                pending_ts = ts;
            }
        }

        if let Some(a) = assistant_msg {
            pair_text.push_str("ASSISTANT: ");
            pair_text.push_str(&a.text);
            if !a.tool_names.is_empty() {
                pair_text.push_str(&format!("\n[tools: {}]", a.tool_names.join(", ")));
            }
            pair_text.push('\n');
            if let Some(ts) = a.timestamp {
                pending_ts = ts;
            }
        }

        if pair_text.len() < CHUNK_MIN_CHARS && !pending.is_empty() {
            pending.push_str(&pair_text);
            continue;
        }

        if !pending.is_empty() && pending.len() + pair_text.len() > CHUNK_MAX_CHARS {
            chunks.push(make_chunk(session, &pending, chunk_index, pending_ts));
            chunk_index += 1;
            pending.clear();
        }

        pending.push_str(&pair_text);

        if pending.len() > CHUNK_MAX_CHARS {
            for window in split_with_overlap(&pending, CHUNK_MAX_CHARS, CHUNK_OVERLAP_CHARS) {
                chunks.push(make_chunk(session, &window, chunk_index, pending_ts));
                chunk_index += 1;
            }
            pending.clear();
        }
    }

    if !pending.trim().is_empty() {
        chunks.push(make_chunk(session, &pending, chunk_index, pending_ts));
    }

    chunks
}

fn make_chunk(session: &AiSession, text: &str, index: usize, timestamp: i64) -> Chunk {
    Chunk {
        chunk_id: format!("{}:{}", session.uid, index),
        session_uid: session.uid.clone(),
        source: session.source,
        project: session.project.clone(),
        title: session.title.clone(),
        timestamp,
        text: text.to_string(),
    }
}

/// Pair each user message with the assistant reply that follows it. Extra
/// consecutive assistant messages become their own pairs.
fn pair_messages(messages: &[Message]) -> Vec<(Option<&Message>, Option<&Message>)> {
    let mut pairs = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        if messages[i].role == Role::User {
            let user = Some(&messages[i]);
            i += 1;
            let assistant = if i < messages.len() && messages[i].role == Role::Assistant {
                let a = Some(&messages[i]);
                i += 1;
                a
            } else {
                None
            };
            pairs.push((user, assistant));

            while i < messages.len() && messages[i].role == Role::Assistant {
                pairs.push((None, Some(&messages[i])));
                i += 1;
            }
        } else {
            pairs.push((None, Some(&messages[i])));
            i += 1;
        }
    }

    pairs
}

fn split_with_overlap(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let step = max_chars.saturating_sub(overlap).max(1);
    let mut windows = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        windows.push(chars[start..end].iter().collect());
        if end >= chars.len() {
            break;
        }
        start += step;
    }

    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::{session_uid, Source};

    fn session() -> AiSession {
        AiSession {
            uid: session_uid(Source::Claude, "test-session"),
            source: Source::Claude,
            session_id: "test-session".into(),
            project: "/test/project".into(),
            title: Some("Test".into()),
            started_at: 1_700_000_000_000,
            last_activity: 1_700_000_000_000,
            model: None,
            message_count: 0,
            file_path: "/tmp/test.jsonl".into(),
            file_mtime: 0,
            file_size: 0,
        }
    }

    fn msg(role: Role, text: &str) -> Message {
        Message {
            role,
            text: text.into(),
            timestamp: None,
            tool_names: vec![],
        }
    }

    #[test]
    fn pairs_a_simple_exchange() {
        let msgs = vec![msg(Role::User, "hello"), msg(Role::Assistant, "hi there")];
        let pairs = pair_messages(&msgs);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0.unwrap().text, "hello");
        assert_eq!(pairs[0].1.unwrap().text, "hi there");
    }

    #[test]
    fn keeps_extra_assistant_messages_as_their_own_pairs() {
        let msgs = vec![
            msg(Role::User, "q"),
            msg(Role::Assistant, "a1"),
            msg(Role::Assistant, "a2"),
        ];
        let pairs = pair_messages(&msgs);
        assert_eq!(pairs.len(), 2);
        assert!(pairs[1].0.is_none());
        assert_eq!(pairs[1].1.unwrap().text, "a2");
    }

    #[test]
    fn chunks_a_short_exchange_into_one_chunk() {
        let s = session();
        let msgs = vec![
            msg(Role::User, "What is Rust?"),
            msg(Role::Assistant, "A systems programming language."),
        ];
        let chunks = chunk_session(&s, &msgs);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("USER: What is Rust?"));
        assert!(chunks[0].text.contains("ASSISTANT: A systems"));
        assert_eq!(chunks[0].chunk_id, "claude:test-session:0");
    }

    #[test]
    fn empty_conversation_produces_no_chunks() {
        assert!(chunk_session(&session(), &[]).is_empty());
    }

    #[test]
    fn records_tool_names_in_chunk_text() {
        let msgs = vec![
            msg(Role::User, "read the file"),
            Message {
                role: Role::Assistant,
                text: "Here it is.".into(),
                timestamp: None,
                tool_names: vec!["Read".into(), "Grep".into()],
            },
        ];
        let chunks = chunk_session(&session(), &msgs);
        assert!(chunks[0].text.contains("[tools: Read, Grep]"));
    }

    #[test]
    fn splits_oversized_exchanges_with_sequential_ids() {
        let big = "x".repeat(CHUNK_MAX_CHARS + 1000);
        let msgs = vec![msg(Role::User, &big), msg(Role::Assistant, "ok")];
        let chunks = chunk_session(&session(), &msgs);
        assert!(chunks.len() > 1);
        for (i, chunk) in chunks.iter().enumerate() {
            assert!(chunk.chunk_id.ends_with(&format!(":{}", i)));
            assert!(chunk.text.chars().count() <= CHUNK_MAX_CHARS);
        }
    }

    #[test]
    fn split_windows_overlap() {
        let text = "a".repeat(100);
        let windows = split_with_overlap(&text, 40, 10);
        assert!(windows.len() >= 3);
        assert_eq!(windows[0].chars().count(), 40);
    }

    #[test]
    fn split_leaves_short_text_alone() {
        assert_eq!(split_with_overlap("short", 100, 50), vec!["short"]);
    }
}
