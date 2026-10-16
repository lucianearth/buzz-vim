//! What counts as new, shared by both apps: the "(N new)" on a reply line,
//! where the bell goes, and the newest-activity highlight.

use crate::model::{Channel, Message};
use crate::readstate::ReadState;

/// How far back a reply counts as new when its thread has no read
/// position anywhere.
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

/// Where the bell goes in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A top-level message.
    Message(String),
    /// A reply in a thread.
    Reply { root: String, id: String },
}

/// The bell: the newest activity in a channel, read or not. A top-level
/// post is itself; a thread opens on its first new reply (see
/// [`new_replies`]), or its newest once all of it is read. `last` is the
/// previous press here: until something newer arrives, the same place
/// (opening the thread marks it read, which would move a fresh answer).
pub fn bell(
    ch: &Channel,
    me: &str,
    read: &ReadState,
    now: u64,
    last: Option<&(u64, Next)>,
) -> Option<(u64, Next)> {
    let (at, root) = newest(ch)?;
    if let Some(l) = last.filter(|l| l.0 == at) {
        return Some(l.clone());
    }
    let m = ch.messages.get(&root)?;
    if at == m.created_at {
        return Some((at, Next::Message(root)));
    }
    let id = new_replies(ch, &root, me, read, now)
        .first()
        .map(|r| r.id.clone())
        .or_else(|| ch.replies.get(&root).and_then(|r| r.last().cloned()))
        .unwrap_or_else(|| root.clone());
    Some((at, Next::Reply { root, id }))
}

/// The top-level message with the newest activity: its own time or its
/// latest reply's, whichever is later.
pub fn latest(ch: &Channel) -> Option<String> {
    newest(ch).map(|(_, id)| id)
}

/// [`latest`] with that activity's time.
fn newest(ch: &Channel) -> Option<(u64, String)> {
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
    fn bell_goes_to_the_newest_activity_and_stays_there() {
        let now = 1_000_000;
        let mut s = Store::new("me".into());
        put(&mut s, "old", "a", now - 500, None);
        put(&mut s, "t1", "me", now - 400, None);
        put(&mut s, "t1a", "a", now - 300, Some("t1"));
        put(&mut s, "t1b", "b", now - 250, Some("t1"));
        put(&mut s, "m2", "b", now - 200, None);
        let mut read = ReadState::new(new_id(), "x".into());
        read.mark(&thread_key("t1"), now - 350);
        // A top-level post is newest: it is the place.
        let b = bell(&s.channels["c"], "me", &read, now, None);
        assert_eq!(b, Some((now - 200, Next::Message("m2".into()))));
        // Still there on a second press.
        assert_eq!(bell(&s.channels["c"], "me", &read, now, b.as_ref()), b);
        // A reply makes the thread newest: its first unread reply.
        put(&mut s, "t1c", "a", now - 100, Some("t1"));
        let b = bell(&s.channels["c"], "me", &read, now, b.as_ref());
        let first = Next::Reply {
            root: "t1".into(),
            id: "t1a".into(),
        };
        assert_eq!(b, Some((now - 100, first.clone())));
        // Opening the thread reads it, but the bell doesn't move.
        read.mark(&thread_key("t1"), now);
        assert_eq!(
            bell(&s.channels["c"], "me", &read, now, b.as_ref()),
            Some((now - 100, first))
        );
        // Fresh, with all of it read: the newest reply.
        assert_eq!(
            bell(&s.channels["c"], "me", &read, now, None),
            Some((
                now - 100,
                Next::Reply {
                    root: "t1".into(),
                    id: "t1c".into()
                }
            ))
        );
        assert_eq!(latest(&s.channels["c"]).as_deref(), Some("t1"));
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
