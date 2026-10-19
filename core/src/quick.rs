//! The Q pop-up: a quick question posted as a new top-level message in
//! #quick, @mentioning the agent that answers there (taskbot by default).
//! Asked again before sending, it carries the channel you were in.

use crate::model::{Channel, Store};

/// The channel's name (default, overridable).
pub const CHANNEL: &str = "quick";
/// #quick on buzz.gitcoin.co, used when the name doesn't match anything.
pub const CHANNEL_ID: &str = "d6968d65-a475-4700-b996-00bb36aa2018";
pub const AGENT: &str = "taskbot";
pub const AGENT_PUBKEY: &str = "f020f0378f36ade38acadcce6c024b4c1bb77f05d607213f2b7674a1a4817da3";
/// The first line of every Q, so the agent knows it's a quick question.
pub const MARKER: &str = "[Q]";
/// How many of the channel's latest messages the context names.
pub const RECENT: usize = 30;

/// #quick by name (not archived, not a DM), else by its known id.
pub fn find_channel<'a>(store: &'a Store, name: &str) -> Option<&'a Channel> {
    let name = name.trim_start_matches('#');
    store
        .channels
        .values()
        .find(|c| !c.is_dm && !c.archived && c.name.eq_ignore_ascii_case(name))
        .or_else(|| {
            store
                .channels
                .get(CHANNEL_ID)
                .filter(|c| !c.archived && name == CHANNEL)
        })
}

/// The agent's pubkey: a profile with that name, members of #quick first;
/// else the default key when the name is the default's.
pub fn find_agent(store: &Store, quick: Option<&Channel>, name: &str) -> Option<String> {
    let name = name.trim_start_matches('@');
    let named = |pk: &String| {
        store
            .profiles
            .get(pk)
            .is_some_and(|p| p.name.eq_ignore_ascii_case(name))
    };
    quick
        .and_then(|c| c.members.iter().find(|pk| named(pk)).cloned())
        .or_else(|| store.profiles.keys().find(|pk| named(pk)).cloned())
        .or_else(|| (name == AGENT).then(|| AGENT_PUBKEY.to_string()))
}

/// What the current channel adds: its name, id and link, the open thread
/// if any, and its latest top-level messages' ids so the agent can fetch
/// them.
pub fn context_block(ch: &Channel, thread: Option<&str>) -> String {
    let mut out = format!(
        "Context: #{} (channel {}) buzz://channel/{}",
        ch.name, ch.id, ch.id
    );
    if let Some(root) = thread {
        out.push_str(&format!(
            "\nOpen thread: buzz://message?channel={}&id={root}",
            ch.id
        ));
    }
    let recent: Vec<&str> = ch
        .timeline
        .iter()
        .filter(|id| ch.messages.get(*id).is_none_or(|m| !m.deleted))
        .rev()
        .take(RECENT)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if !recent.is_empty() {
        out.push_str(&format!(
            "\nLast {} messages (oldest first): {}",
            recent.len(),
            recent.join(" ")
        ));
    }
    out
}

/// The message as sent: the marker line, the context when asked for, then
/// the @mention and what was typed.
pub fn compose(agent: &str, text: &str, context: Option<&str>) -> String {
    let mut out = format!("{MARKER}\n");
    if let Some(c) = context {
        out.push_str(c);
        out.push_str("\n\n");
    }
    out.push_str(&format!(
        "@{} {}",
        agent.trim_start_matches('@'),
        text.trim()
    ));
    out
}

/// Why Q can't open now (no #quick, or channels still loading), or None
/// when it can. buzz-vim never makes the channel.
pub fn unavailable(store: &Store, name: &str, channels_loaded: bool) -> Option<String> {
    if find_channel(store, name).is_some() {
        return None;
    }
    Some(if channels_loaded {
        format!(
            "no #{} channel: Q posts there, and buzz-vim doesn't make channels",
            name.trim_start_matches('#')
        )
    } else {
        "channels are still loading; try Q again in a moment".to_string()
    })
}

/// Whether the channel you're in can go along with a Q: not when there is
/// none, and not #quick itself.
pub fn can_add(store: &Store, name: &str, here: Option<&str>) -> Result<(), String> {
    let here = here
        .and_then(|h| store.channels.get(h))
        .ok_or("no channel open to add")?;
    if find_channel(store, name).is_some_and(|q| q.id == here.id) {
        return Err("you're in the Q channel already; nothing to add".into());
    }
    Ok(())
}

/// A Q ready to sign: post `content` as a new top-level message in
/// `channel`, p-tagging and mention-tagging `agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub channel: String,
    pub agent: String,
    pub content: String,
}

/// The Q as it will be sent, or why it can't be. `here` is the channel to
/// add (with its open thread, if any); None asks the question alone.
pub fn ask(
    store: &Store,
    name: &str,
    agent: &str,
    text: &str,
    here: Option<(&str, Option<&str>)>,
) -> Result<Ask, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("type a question first".into());
    }
    let q = find_channel(store, name)
        .ok_or_else(|| format!("no #{} channel", name.trim_start_matches('#')))?;
    let agent_pk = find_agent(store, Some(q), agent)
        .ok_or_else(|| format!("no agent named {agent} found to ask"))?;
    let context = match here {
        Some((h, root)) => {
            let ch = store.channels.get(h).ok_or("no channel open to add")?;
            Some(context_block(ch, root))
        }
        None => None,
    };
    Ok(Ask {
        channel: q.id.clone(),
        agent: agent_pk,
        content: compose(agent, text, context.as_deref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_builds_both_variants_or_says_why_not() {
        let mut s = Store::new("me".into());
        assert_eq!(
            unavailable(&s, "quick", false).as_deref(),
            Some("channels are still loading; try Q again in a moment")
        );
        assert!(
            unavailable(&s, "#quick", true).is_some_and(|m| m.starts_with("no #quick channel"))
        );
        assert_eq!(
            ask(&s, "quick", AGENT, "hi", None),
            Err("no #quick channel".into())
        );
        let q = s.channel_mut("q1");
        q.name = "quick".into();
        q.members = vec!["me".into(), "tb".into()];
        s.profiles.entry("tb".into()).or_default().name = "taskbot".into();
        let c = s.channel_mut("c1");
        c.name = "buzz-vim".into();
        c.timeline = vec!["m1".into(), "m2".into()];
        assert_eq!(unavailable(&s, "quick", false), None);
        assert_eq!(
            ask(&s, "quick", AGENT, "  ", None),
            Err("type a question first".into())
        );
        // First Q: the question alone.
        let a = ask(&s, "quick", AGENT, " how do we ship? ", None).unwrap();
        assert_eq!((a.channel.as_str(), a.agent.as_str()), ("q1", "tb"));
        assert_eq!(a.content, "[Q]\n@taskbot how do we ship?");
        // Q again: + the channel and its latest messages (and open thread).
        assert_eq!(can_add(&s, "quick", Some("c1")), Ok(()));
        let a = ask(&s, "quick", AGENT, "why?", Some(("c1", Some("m1")))).unwrap();
        assert!(
            a.content
                .starts_with("[Q]\nContext: #buzz-vim (channel c1)"),
            "{}",
            a.content
        );
        assert!(
            a.content
                .contains("Open thread: buzz://message?channel=c1&id=m1")
        );
        assert!(a.content.contains("Last 2 messages (oldest first): m1 m2"));
        assert!(a.content.ends_with("\n\n@taskbot why?"));
        // Nothing to add from #quick itself, or from nowhere.
        assert!(can_add(&s, "quick", Some("q1")).is_err_and(|e| e.contains("already")));
        assert!(can_add(&s, "quick", None).is_err_and(|e| e == "no channel open to add"));
        assert_eq!(
            ask(&s, "quick", AGENT, "x", Some(("gone", None))),
            Err("no channel open to add".into())
        );
        assert!(ask(&s, "quick", "nobody", "x", None).is_err_and(|e| e.contains("nobody")));
    }

    #[test]
    fn finds_quick_and_its_agent() {
        let mut s = Store::new("me".into());
        let q = s.channel_mut("q1");
        q.name = "Quick".into();
        q.members = vec!["me".into(), "tb".into()];
        s.channel_mut("dm").name = "quick".into();
        s.channel_mut("dm").is_dm = true;
        s.profiles.entry("tb".into()).or_default().name = "taskbot".into();
        s.profiles.entry("other".into()).or_default().name = "helper".into();
        let ch = find_channel(&s, "#quick").map(|c| c.id.clone());
        assert_eq!(ch.as_deref(), Some("q1"));
        let q = s.channels.get("q1");
        assert_eq!(find_agent(&s, q, "taskbot").as_deref(), Some("tb"));
        assert_eq!(find_agent(&s, q, "@helper").as_deref(), Some("other"));
        assert_eq!(find_agent(&s, q, "nobody"), None);
        s.profiles.clear();
        assert_eq!(find_agent(&s, q, "taskbot").as_deref(), Some(AGENT_PUBKEY));
        // Archived, or not there: none.
        s.channel_mut("q1").archived = true;
        assert!(find_channel(&s, "quick").is_none());
        // The known id still counts when the name hasn't loaded.
        s.channel_mut(CHANNEL_ID);
        assert!(find_channel(&s, "quick").is_some());
        assert!(find_channel(&s, "elsewhere").is_none());
    }

    #[test]
    fn context_names_the_channel_and_its_latest_messages() {
        let mut s = Store::new("me".into());
        let c = s.channel_mut("c1");
        c.name = "buzz-vim".into();
        c.timeline = (0..40).map(|i| format!("m{i:02}")).collect();
        let block = context_block(&s.channels["c1"], Some("r1"));
        assert!(block.starts_with("Context: #buzz-vim (channel c1) buzz://channel/c1"));
        assert!(block.contains("Open thread: buzz://message?channel=c1&id=r1"));
        assert!(block.contains("Last 30 messages (oldest first): m10 m11"));
        assert!(block.ends_with("m39"));
        let msg = compose("taskbot", " why? ", Some(&block));
        assert!(msg.starts_with("[Q]\nContext: #buzz-vim"));
        assert!(msg.ends_with("\n\n@taskbot why?"));
        assert_eq!(compose("@taskbot", "hi", None), "[Q]\n@taskbot hi");
    }
}
