//! What counts as new, shared by both apps: the "(N new)" on a reply line,
//! where the jump-to-unread key goes, and the newest-activity highlight.

use std::collections::HashSet;

use crate::model::{Channel, Message};
use crate::readstate::ReadState;

/// How far back a reply counts as new when its thread has no read
/// position anywhere. Also how far back the unread lookup reaches.
pub const HORIZON: u64 = 7 * 86_400;

/// The read-state context for a thread (Desktop's key).
pub fn thread_key(root: &str) -> String {
    format!("thread:{root}")
}

/// Replies under `root` from others that you haven't seen, oldest first:
/// after the thread's read position, or within the horizon when it has none.
pub fn new_replies<'a>(
    ch: &'a Channel,
    root: &str,
    me: &str,
    read: &ReadState,
    now: u64,
) -> Vec<&'a Message> {
    let seen = read
        .read_at(&thread_key(root))
        .unwrap_or(now.saturating_sub(HORIZON));
    ch.replies
        .get(root)
        .into_iter()
        .flatten()
        .filter_map(|id| ch.messages.get(id))
        .filter(|r| {
            !r.deleted
                && r.author != me
                && r.created_at > seen
                // Desktop's mark-as-read on a single message.
                && read
                    .read_at(&format!("msg:{}", r.id))
                    .is_none_or(|t| r.created_at > t)
        })
        .collect()
}

/// Where the jump key goes in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A top-level message.
    Message(String),
    /// The first new reply in a thread.
    Reply { root: String, id: String },
}

/// The oldest unread thing in a channel: a top-level message from someone
/// else after `entry` (the read position when you came into the channel)
/// that the key hasn't visited yet, or a new reply (see [`new_replies`]).
pub fn next_unread(
    ch: &Channel,
    me: &str,
    read: &ReadState,
    entry: Option<u64>,
    visited: &HashSet<String>,
    now: u64,
) -> Option<Next> {
    let after = entry.unwrap_or(now.saturating_sub(HORIZON));
    let mut best: Option<(u64, Next)> = None;
    let mut offer = |at: u64, n: Next| {
        if best.as_ref().is_none_or(|(b, _)| at < *b) {
            best = Some((at, n));
        }
    };
    for id in &ch.timeline {
        let Some(m) = ch.messages.get(id) else {
            continue;
        };
        if !m.deleted
            && !m.system
            && m.author != me
            && m.created_at > after
            && !visited.contains(id)
        {
            offer(m.created_at, Next::Message(id.clone()));
        }
    }
    for root in ch.replies.keys() {
        if let Some(r) = new_replies(ch, root, me, read, now).first() {
            offer(
                r.created_at,
                Next::Reply {
                    root: root.clone(),
                    id: r.id.clone(),
                },
            );
        }
    }
    best.map(|(_, n)| n)
}

/// The top-level message with the newest activity: its own time or its
/// latest reply's, whichever is later.
pub fn latest(ch: &Channel) -> Option<String> {
    ch.timeline
        .iter()
        .filter_map(|id| ch.messages.get(id))
        .filter(|m| !m.deleted)
        .map(|m| {
            let reply = ch
                .replies
                .get(&m.id)
                .and_then(|r| r.iter().rev().find_map(|id| ch.messages.get(id)))
                .map_or(0, |r| r.created_at);
            let summary = ch.summary_replies.get(&m.id).map_or(0, |s| s.1);
            (m.created_at.max(reply).max(summary), m.id.clone())
        })
        .max()
        .map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Store;
    use crate::readstate::new_id;

    fn put(s: &mut Store, id: &str, author: &str, at: u64, root: Option<&str>) {
        let ch = s.channel_mut("c");
        ch.messages.insert(
            id.into(),
            Message {
                id: id.into(),
                author: author.into(),
                created_at: at,
                root: root.map(str::to_string),
                ..Default::default()
            },
        );
        match root {
            Some(r) => ch.replies.entry(r.into()).or_default().push(id.into()),
            None => ch.timeline.push(id.into()),
        }
    }

    #[test]
    fn jump_walks_oldest_first_and_threads_clear_when_read() {
        let now = 1_000_000;
        let mut s = Store::new("me".into());
        put(&mut s, "old", "a", now - 500, None);
        put(&mut s, "t1", "me", now - 400, None);
        put(&mut s, "t1r", "a", now - 300, Some("t1"));
        put(&mut s, "m2", "b", now - 200, None);
        put(&mut s, "mine", "me", now - 100, None);
        let mut read = ReadState::new(new_id(), "x".into());
        let ch = &s.channels["c"];
        let mut visited = HashSet::new();
        let entry = Some(now - 450);
        let next =
            |read: &ReadState, v: &HashSet<String>| next_unread(ch, "me", read, entry, v, now);
        assert_eq!(
            next(&read, &visited),
            Some(Next::Reply {
                root: "t1".into(),
                id: "t1r".into()
            })
        );
        assert_eq!(new_replies(ch, "t1", "me", &read, now).len(), 1);
        read.mark(&thread_key("t1"), now);
        assert!(new_replies(ch, "t1", "me", &read, now).is_empty());
        assert_eq!(next(&read, &visited), Some(Next::Message("m2".into())));
        visited.insert("m2".to_string());
        assert_eq!(next(&read, &visited), None);
        assert_eq!(latest(ch).as_deref(), Some("mine"));
    }

    #[test]
    fn replies_past_the_horizon_without_a_position_are_old() {
        let now = 10 * HORIZON;
        let mut s = Store::new("me".into());
        put(&mut s, "r", "me", now - 2 * HORIZON, None);
        put(&mut s, "x", "a", now - HORIZON - 1, Some("r"));
        put(&mut s, "y", "a", now - 10, Some("r"));
        let read = ReadState::new(new_id(), "x".into());
        let ids: Vec<&str> = new_replies(&s.channels["c"], "r", "me", &read, now)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["y"]);
        // The thread with the newest reply is the latest activity.
        assert_eq!(latest(&s.channels["c"]).as_deref(), Some("r"));
    }
}
