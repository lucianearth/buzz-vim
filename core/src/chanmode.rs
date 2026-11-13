//! A channel's mode (🧠 ideating, 🚀 running, or off) and whether its tab
//! needs the operator (🚩, or 📣 for a person's mention).
//!
//! The mode: a running list's header says running; else the owner's saved
//! pick for the channel (a plain kind 30078, `d` = `buzz-mode:<channel>`,
//! content `ideating` or `off`, `h` = the channel; the aei watcher reads the
//! same record; the relay keeps 30078 global, so read it by `#d`, never
//! `#h`); else a list makes it ideating; else ideating once the
//! owner has @mentioned a bot there, otherwise off. DMs are always off.
//!
//! Needs you: running, the watcher says so (tab 0's watch rows) or the
//! wrap-up just finished; ideating, a bot posted after the owner and no
//! agent is working there. A person's mention or DM is 📣; 🚩 outranks it.
//! Both clear once the owner posts (or types) there, except a watcher row,
//! which clears when he answers its ping (needs::rows), and a contested
//! one, which clears when he picks a driver. Off channels and DMs keep 📣
//! only.

use std::collections::HashMap;

use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};

use crate::checklist::{Checklists, Header, Mode};
use crate::model::{Store, tag};
use crate::needs::{Reason, Row};

pub const KIND: u16 = 30078;
pub const D_PREFIX: &str = "buzz-mode:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChanMode {
    Ideating,
    Running,
    Off,
}

impl ChanMode {
    pub const ALL: [ChanMode; 3] = [ChanMode::Ideating, ChanMode::Running, ChanMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            ChanMode::Ideating => "ideating",
            ChanMode::Running => "running",
            ChanMode::Off => "off",
        }
    }

    /// The tab's icon; off has none.
    pub fn icon(self) -> Option<&'static str> {
        match self {
            ChanMode::Ideating => Some("🧠"),
            ChanMode::Running => Some("🚀"),
            ChanMode::Off => None,
        }
    }
}

/// The owner's saved picks, newest per channel.
#[derive(Debug, Default)]
pub struct Overrides {
    by: HashMap<String, (u64, ChanMode)>,
}

/// A mode record's channel and pick, before trust.
fn parse(ev: &Event) -> Option<(&str, ChanMode)> {
    if ev.kind.as_u16() != KIND {
        return None;
    }
    let channel = tag(ev, "d")?.strip_prefix(D_PREFIX)?;
    let mode = match ev.content.trim() {
        "ideating" => ChanMode::Ideating,
        "off" => ChanMode::Off,
        _ => return None,
    };
    Some((channel, mode))
}

pub fn is_record(ev: &Event) -> bool {
    ev.kind.as_u16() == KIND && tag(ev, "d").is_some_and(|d| d.starts_with(D_PREFIX))
}

impl Overrides {
    /// Fold one record, if the owner signed it. True when it is the newest.
    pub fn apply(&mut self, ev: &Event, owner: &str) -> bool {
        let Some((channel, mode)) = parse(ev).filter(|_| ev.pubkey.to_hex() == owner) else {
            return false;
        };
        let at = ev.created_at.as_secs();
        match self.by.get(channel) {
            Some((old, _)) if *old >= at => false,
            _ => {
                self.by.insert(channel.to_string(), (at, mode));
                true
            }
        }
    }

    pub fn get(&self, channel: &str) -> Option<ChanMode> {
        self.by.get(channel).map(|(_, m)| *m)
    }

    /// [`record`] dated `now`, or a second after the previous pick when
    /// that is not older: two picks in one second must not tie, since the
    /// relay keeps the lowest id on a same-second replace.
    pub fn record(&self, channel: &str, mode: ChanMode, now: u64) -> EventBuilder {
        let after = self.by.get(channel).map_or(0, |(at, _)| at + 1);
        record(channel, mode).custom_created_at(Timestamp::from(now.max(after)))
    }
}

/// The owner's pick for a channel with no running list.
pub fn record(channel: &str, mode: ChanMode) -> EventBuilder {
    let tags = [["d", &format!("{D_PREFIX}{channel}")], ["h", channel]]
        .map(|t| Tag::parse(t).expect("tag"));
    EventBuilder::new(Kind::Custom(KIND), mode.label()).tags(tags)
}

/// Has the owner @mentioned a bot in this channel (in what we hold)? A
/// watcher is not one: answering its ping does not make a channel ideating.
pub fn mentioned_bot(
    store: &Store,
    owner: &str,
    channel: &str,
    watchers: &[String],
    is_bot: impl Fn(&str) -> bool,
) -> bool {
    let bot = |p: &str| is_bot(p) && !watchers.iter().any(|w| w == p);
    store.channels.get(channel).is_some_and(|c| {
        c.messages
            .values()
            .any(|m| m.author == owner && m.mentions.iter().any(|p| bot(p)))
    })
}

/// The channel's mode (see the module doc).
pub fn effective(
    header: Option<&Header>,
    saved: Option<ChanMode>,
    dm: bool,
    mentioned_bot: bool,
) -> ChanMode {
    match (header.map(|h| h.mode), saved) {
        _ if dm => ChanMode::Off,
        (Some(Mode::Running), _) => ChanMode::Running,
        (_, Some(m)) => m,
        (Some(_), None) => ChanMode::Ideating,
        (None, None) if mentioned_bot => ChanMode::Ideating,
        (None, None) => ChanMode::Off,
    }
}

/// What picking a mode publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The owner's saved pick (`record`).
    Save(ChanMode),
    /// The owner signs the running header back to ideating.
    Flip,
    /// A message from the owner @mentioning `to`: the go to the list's
    /// creator ([`GO_TEXT`]), or with no list, the ask for one
    /// ([`ASK_LIST_TEXT`]).
    Send { text: &'static str, to: String },
}

/// The steps for picking `pick` in `channel`, from what we hold: [`plan`]
/// with the list's header, the saved pick and the newest bot. DMs have no
/// mode, and a contested channel waits for the owner to pick a driver.
pub fn steps(
    store: &Store,
    lists: &Checklists,
    saved: &Overrides,
    channel: &str,
    pick: ChanMode,
    is_bot: impl Fn(&str) -> bool,
) -> Result<Vec<Step>, &'static str> {
    if store.channels.get(channel).is_some_and(|c| c.is_dm) {
        return Err("DMs have no mode");
    }
    if !lists.driver(channel).contested.is_empty() {
        return Err("two lists claim this channel; pick a driver first (checklist mode --driver)");
    }
    let header = lists.header(channel);
    let bot = last_bot(store, channel, &lists.trust.watchers, is_bot);
    plan(pick, header.as_ref(), saved.get(channel), bot.as_deref())
}

/// The steps for picking `pick` in a channel. `bot`: who to ask for a list
/// when there is none. Err says why nothing can happen.
pub fn plan(
    pick: ChanMode,
    header: Option<&Header>,
    saved: Option<ChanMode>,
    bot: Option<&str>,
) -> Result<Vec<Step>, &'static str> {
    let running = header.is_some_and(|h| h.mode == Mode::Running);
    Ok(match pick {
        ChanMode::Running => match header {
            None => {
                let mut s = vec![Step::Send {
                    text: ASK_LIST_TEXT,
                    to: bot
                        .ok_or("no list here, and no bot to ask for one")?
                        .to_string(),
                }];
                // A saved off would hide the list the bot makes.
                if saved.is_some() {
                    s.push(Step::Save(ChanMode::Ideating));
                }
                s
            }
            Some(_) if running => vec![],
            Some(h) => {
                let mut s = vec![Step::Send {
                    text: GO_TEXT,
                    to: h.creator.clone(),
                }];
                // The saved pick would outlive the run.
                if saved.is_some() {
                    s.push(Step::Save(ChanMode::Ideating));
                }
                s
            }
        },
        m => {
            let mut s = if running { vec![Step::Flip] } else { vec![] };
            s.push(Step::Save(m));
            s
        }
    })
}

/// The go message's text and the ask for a list.
pub const GO_TEXT: &str = "go";
pub const ASK_LIST_TEXT: &str = "make a checklist for this channel";

/// The newest bot in a channel, not a watcher: who a list request goes to.
pub fn last_bot(
    store: &Store,
    channel: &str,
    watchers: &[String],
    is_bot: impl Fn(&str) -> bool,
) -> Option<String> {
    store
        .channels
        .get(channel)?
        .messages
        .values()
        .filter(|m| !m.deleted && is_bot(&m.author) && !watchers.contains(&m.author))
        .max_by_key(|m| (m.created_at, &m.id))
        .map(|m| m.author.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flag {
    /// 📣 a person's mention or DM.
    Mention,
    /// 🚩 needs you.
    NeedsYou,
}

impl Flag {
    pub fn icon(self) -> &'static str {
        match self {
            Flag::Mention => "📣",
            Flag::NeedsYou => "🚩",
        }
    }
}

/// One tab's mode and flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tab {
    pub mode: ChanMode,
    /// The flag and since when.
    pub flag: Option<(Flag, u64)>,
}

impl Tab {
    /// Flashes (mode icon <-> flag); off channels and DMs hold a steady 📣.
    pub fn flashes(&self) -> bool {
        self.mode != ChanMode::Off && self.flag.is_some()
    }

    /// The icon to draw on frame `on` (flash phase).
    pub fn icon(&self, on: bool) -> Option<&'static str> {
        match self.flag {
            Some((f, _)) if self.mode == ChanMode::Off || on => Some(f.icon()),
            _ => self.mode.icon(),
        }
    }
}

/// Inputs for one channel's [`tab`].
pub struct World<'a> {
    pub store: &'a Store,
    pub lists: &'a Checklists,
    pub saved: &'a Overrides,
    /// The needs rows (`needs::rows`).
    pub rows: &'a [Row],
    pub is_bot: &'a dyn Fn(&str) -> bool,
    /// An agent is working in the channel (the tab's spinner).
    pub working: bool,
    /// When the owner last typed there (0: not known).
    pub typed: u64,
    /// Now (secs): a bot's typing pulse this recent is working too.
    pub now: u64,
}

/// How long a bot's typing pulse counts as working: the watcher's idle
/// window (aei stall-judge IDLE_SECONDS).
pub const TYPING_IDLE: u64 = 210;

/// A bot, anyone's, sent a typing pulse in `channel` within [`TYPING_IDLE`].
pub fn bot_typing(store: &Store, channel: &str, is_bot: impl Fn(&str) -> bool, now: u64) -> bool {
    store.channels.get(channel).is_some_and(|c| {
        c.typing
            .iter()
            .any(|(pk, at)| *at + TYPING_IDLE > now && is_bot(pk))
    })
}

/// The wrap-up just finished: the list is back to ideating with its
/// standing last item done. When it was ticked.
pub fn wrap_up_done(lists: &Checklists, channel: &str) -> Option<u64> {
    if lists.header(channel)?.mode != Mode::Ideating {
        return None;
    }
    lists
        .items(channel)
        .into_iter()
        .find(|i| i.standing.as_deref() == Some("last"))
        .filter(|i| i.status == crate::checklist::Status::Done)?
        .done_at
}

/// One channel's tab.
pub fn tab(w: &World, channel: &str) -> Tab {
    let op = w.lists.trust.owner.as_str();
    let ch = w.store.channels.get(channel);
    let dm = ch.is_some_and(|c| c.is_dm);
    let header = w.lists.header(channel);
    let mode = effective(
        header.as_ref(),
        w.saved.get(channel),
        dm,
        mentioned_bot(w.store, op, channel, &w.lists.trust.watchers, w.is_bot),
    );
    // He's been back: posted or typed there.
    let back = ch
        .into_iter()
        .flat_map(|c| c.messages.values())
        .filter(|m| m.author == op && !m.deleted)
        .map(|m| m.created_at)
        .max()
        .unwrap_or(0)
        .max(w.typed);
    let mut flags: Vec<(Flag, u64)> = w
        .rows
        .iter()
        .filter(|r| r.channel == channel)
        .filter_map(|r| match r.reason {
            // 📣 clears once he has been back to the channel.
            Reason::Mention | Reason::Dm => (r.at > back).then_some((Flag::Mention, r.at)),
            // A watch row's `at` is the stall's `since`, which can predate
            // his last post; needs::rows already cleared it if he answered
            // the ping, and a contested row stays until he picks a driver.
            Reason::Watch(_) | Reason::Contested(_) => Some((Flag::NeedsYou, r.at)),
        })
        .collect();
    if let Some(at) = wrap_up_done(w.lists, channel).filter(|at| *at > back) {
        flags.push((Flag::NeedsYou, at));
    }
    let working = w.working || bot_typing(w.store, channel, w.is_bot, w.now);
    if mode == ChanMode::Ideating && !working {
        let bot_last = ch
            .into_iter()
            .flat_map(|c| c.messages.values())
            .filter(|m| !m.deleted && !m.system && m.created_at > back && (w.is_bot)(&m.author))
            .map(|m| m.created_at)
            .min();
        // Only once he has spoken here: a bot alone in a channel isn't a turn.
        if let Some(at) = bot_last.filter(|_| back > 0) {
            flags.push((Flag::NeedsYou, at));
        }
    }
    if mode == ChanMode::Off {
        flags.retain(|(f, _)| *f == Flag::Mention);
        // A contested channel has no header, so it can read Off; its 🚩
        // stays until he picks a driver.
        flags.extend(
            w.rows
                .iter()
                .filter(|r| r.channel == channel && matches!(r.reason, Reason::Contested(_)))
                .map(|r| (Flag::NeedsYou, r.at)),
        );
    }
    // The strongest flag, since its oldest cause.
    let flag = flags
        .iter()
        .copied()
        .min_by_key(|&(f, at)| (std::cmp::Reverse(f), at));
    Tab { mode, flag }
}

/// `!`: the next flagged channel among `tabs` after `current`, oldest
/// flag first, round again at the end.
pub fn next_flagged<'a>(flagged: &'a [(String, u64)], current: Option<&str>) -> Option<&'a str> {
    let mut order: Vec<&(String, u64)> = flagged.iter().collect();
    order.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    let at = current.and_then(|c| order.iter().position(|(id, _)| id == c));
    let next = match at {
        Some(i) => order.get(i + 1).or(order.first()),
        None => order.first(),
    };
    next.map(|(id, _)| id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::Trust;
    use crate::model::Profile;
    use nostr::{Keys, Timestamp};
    use serde_json::json;

    const CH: &str = "2130646b-d6c8-4ec3-93e3-36f77bee1a37";
    const DM: &str = "bcd11c3d-f78b-4934-8e48-c08b6baed1c5";
    const NOW: u64 = 1_800_000_000;

    struct T {
        store: Store,
        lists: Checklists,
        saved: Overrides,
        op: Keys,
        human: Keys,
        bot: Keys,
        watcher: Keys,
        working: bool,
    }

    fn pk(k: &Keys) -> String {
        k.public_key().to_hex()
    }

    fn sign(who: &Keys, b: EventBuilder, at: u64) -> Event {
        b.custom_created_at(Timestamp::from(at))
            .sign_with_keys(who)
            .unwrap()
    }

    impl T {
        fn new() -> Self {
            let (op, human, bot, watcher) = (
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
                Keys::generate(),
            );
            let mut store = Store::new(pk(&op));
            store.profiles.insert(
                pk(&bot),
                Profile {
                    is_agent: true,
                    ..Default::default()
                },
            );
            let dm = store.channel_mut(DM);
            dm.is_dm = true;
            dm.members = vec![pk(&op), pk(&human)];
            store.channel_mut(CH);
            let lists = Checklists::new(Trust {
                owner: pk(&op),
                watchers: vec![pk(&watcher)],
            });
            T {
                store,
                lists,
                saved: Overrides::default(),
                op,
                human,
                bot,
                watcher,
                working: false,
            }
        }

        fn post(&mut self, who: &Keys, ch: &str, to: Option<&Keys>, at: u64) -> String {
            let mut tags = vec![Tag::parse(["h", ch]).unwrap()];
            if let Some(k) = to {
                tags.push(Tag::parse(["p", &pk(k)]).unwrap());
            }
            let ev = sign(who, EventBuilder::new(Kind::Custom(9), "hi").tags(tags), at);
            self.store.apply(&ev);
            ev.id.to_hex()
        }

        fn record(&mut self, who: &Keys, kind: u16, d: String, at: u64, v: serde_json::Value) {
            let ty = match kind {
                30470 => "item",
                30471 => "header",
                _ => "watch",
            };
            let tags = [
                ["d", d.as_str()],
                ["h", CH],
                ["t", "buzz-checklist"],
                ["channel", CH],
                ["checklist-type", ty],
            ]
            .map(|t| Tag::parse(t).unwrap());
            let ev = sign(
                who,
                EventBuilder::new(Kind::Custom(kind), v.to_string()).tags(tags),
                at,
            );
            assert!(self.lists.apply(&ev));
        }

        fn list(&mut self, mode: &str, at: u64) {
            let b = self.bot.clone();
            let d = format!("buzz-checklist/header/{CH}/{}", pk(&b));
            self.record(
                &b,
                30471,
                d,
                at,
                json!({"v": 1, "title": "t", "mode": mode}),
            );
        }

        fn wrap_up(&mut self, status: &str, at: u64) {
            let b = self.bot.clone();
            let d = format!("buzz-checklist/item/{CH}/{}/0000000f", pk(&b));
            self.record(
                &b,
                30470,
                d,
                at,
                json!({"text": "wrap-up", "status": status, "standing": "last", "done_at": at}),
            );
        }

        fn tab(&self, ch: &str) -> Tab {
            let bots = [pk(&self.bot), pk(&self.watcher)];
            let is_bot = |p: &str| bots.iter().any(|b| b == p);
            let rows = crate::needs::rows(&self.store, &self.lists, is_bot, NOW);
            tab(
                &World {
                    store: &self.store,
                    lists: &self.lists,
                    saved: &self.saved,
                    rows: &rows,
                    is_bot: &is_bot,
                    working: self.working,
                    typed: 0,
                    now: NOW,
                },
                ch,
            )
        }
    }

    #[test]
    fn mode_is_list_then_saved_pick_then_bot_mention() {
        use ChanMode::*;
        let mut t = T::new();
        assert_eq!(t.tab(CH).mode, Off);
        let bot = t.bot.clone();
        let op = t.op.clone();
        t.post(&op, CH, Some(&bot), NOW - 100);
        assert_eq!(t.tab(CH).mode, Ideating);
        let off = sign(&op, record(CH, Off), NOW - 90);
        assert_eq!(parse(&off), Some((CH, Off)));
        // Only the owner's records count, the newest wins.
        assert!(
            !t.saved
                .apply(&sign(&bot, record(CH, Ideating), NOW), &pk(&op))
        );
        assert!(t.saved.apply(&off, &pk(&op)));
        assert!(
            !t.saved
                .apply(&sign(&op, record(CH, Ideating), NOW - 95), &pk(&op))
        );
        assert_eq!(t.tab(CH).mode, Off);
        t.list("running", NOW - 80);
        assert_eq!(t.tab(CH).mode, Running);
        t.list("ideating", NOW - 70);
        assert_eq!(t.tab(CH).mode, Off);
        assert_eq!(t.tab(DM).mode, Off);
    }

    #[test]
    fn ideating_flags_when_a_bot_spoke_last_and_nobody_works() {
        let mut t = T::new();
        let (op, bot) = (t.op.clone(), t.bot.clone());
        t.post(&op, CH, Some(&bot), NOW - 100);
        assert_eq!(t.tab(CH).flag, None);
        t.post(&bot, CH, None, NOW - 90);
        t.post(&bot, CH, None, NOW - 80);
        assert_eq!(t.tab(CH).flag, Some((Flag::NeedsYou, NOW - 90)));
        assert!(t.tab(CH).flashes());
        assert_eq!(t.tab(CH).icon(true), Some("🚩"));
        assert_eq!(t.tab(CH).icon(false), Some("🧠"));
        t.working = true;
        assert_eq!(t.tab(CH).flag, None);
        t.working = false;
        t.post(&op, CH, None, NOW - 70);
        assert_eq!(t.tab(CH).flag, None);
    }

    #[test]
    fn another_owners_bot_typing_is_working() {
        // Review 6877cd69: a bot we don't own mid-turn sends only kind
        // 20002; within the idle window it holds the 🚩 back.
        let mut t = T::new();
        let (op, bot) = (t.op.clone(), t.bot.clone());
        t.post(&op, CH, Some(&bot), NOW - 300);
        t.post(&bot, CH, None, NOW - 290);
        assert_eq!(t.tab(CH).flag, Some((Flag::NeedsYou, NOW - 290)));
        let pulse = |at| sign(&bot, crate::model::typing(CH, None), at);
        t.store.apply(&pulse(NOW - TYPING_IDLE - 1));
        assert!(t.tab(CH).flag.is_some(), "too old");
        t.store.apply(&pulse(NOW - 10));
        assert_eq!(t.tab(CH).flag, None);
        // A person typing is not a bot working.
        let h = t.human.clone();
        t.store.channel_mut(CH).typing.clear();
        t.store
            .apply(&sign(&h, crate::model::typing(CH, None), NOW - 5));
        assert!(t.tab(CH).flag.is_some());
    }

    #[test]
    fn mentions_flash_a_megaphone_and_hold_it_steady_when_off() {
        let mut t = T::new();
        let (op, bot, human) = (t.op.clone(), t.bot.clone(), t.human.clone());
        t.post(&human, CH, Some(&op), NOW - 50);
        let off = t.tab(CH);
        assert_eq!(
            (off.mode, off.flag),
            (ChanMode::Off, Some((Flag::Mention, NOW - 50)))
        );
        assert!(!off.flashes());
        assert_eq!((off.icon(true), off.icon(false)), (Some("📣"), Some("📣")));
        t.post(&human, DM, None, NOW - 40);
        assert_eq!(t.tab(DM).flag, Some((Flag::Mention, NOW - 40)));
        // 🚩 outranks 📣.
        t.post(&op, CH, Some(&bot), NOW - 60);
        t.post(&bot, CH, None, NOW - 30);
        assert_eq!(t.tab(CH).flag, Some((Flag::NeedsYou, NOW - 30)));
        t.post(&op, CH, None, NOW - 20);
        assert_eq!(t.tab(CH).flag, None);
    }

    #[test]
    fn running_flags_on_the_watcher_and_on_a_finished_wrap_up() {
        let mut t = T::new();
        let (op, w) = (t.op.clone(), t.watcher.clone());
        t.post(&op, CH, None, NOW - 200);
        t.list("running", NOW - 150);
        t.wrap_up("open", NOW - 150);
        assert_eq!(
            t.tab(CH),
            Tab {
                mode: ChanMode::Running,
                flag: None
            }
        );
        t.record(
            &w,
            30472,
            format!("buzz-checklist/watch/{CH}"),
            NOW - 100,
            json!({"state": "stalled", "since": NOW - 100}),
        );
        assert_eq!(t.tab(CH).flag, Some((Flag::NeedsYou, NOW - 100)));
        t.record(
            &w,
            30472,
            format!("buzz-checklist/watch/{CH}"),
            NOW - 90,
            json!({"state": "working", "since": NOW - 90}),
        );
        t.wrap_up("done", NOW - 60);
        assert_eq!(wrap_up_done(&t.lists, CH), None, "still running");
        t.list("ideating", NOW - 59);
        assert_eq!(wrap_up_done(&t.lists, CH), Some(NOW - 60));
        let tab = t.tab(CH);
        assert_eq!(
            tab,
            Tab {
                mode: ChanMode::Ideating,
                flag: Some((Flag::NeedsYou, NOW - 60))
            }
        );
        t.post(&op, CH, None, NOW - 10);
        assert_eq!(t.tab(CH).flag, None);
    }

    #[test]
    fn a_stall_older_than_his_last_post_still_flags_until_he_answers_the_ping() {
        // Review 977beced: the watch row's `at` is the stall's `since`; his
        // post elsewhere after it, before the ping, must not hide the 🚩.
        let mut t = T::new();
        let (op, w) = (t.op.clone(), t.watcher.clone());
        t.list("running", NOW - 150);
        t.post(&op, CH, None, NOW - 80);
        let ping = t.post(&w, CH, Some(&op), NOW - 50);
        t.record(
            &w,
            30472,
            format!("buzz-checklist/watch/{CH}"),
            NOW - 50,
            json!({"state": "stalled", "since": NOW - 100, "ping": ping}),
        );
        assert_eq!(t.tab(CH).flag, Some((Flag::NeedsYou, NOW - 100)));
    }

    #[test]
    fn watchers_are_not_the_bots_mode_and_list_asks_look_for() {
        // Review 94dc1a6f: is_bot counts watchers; these two must not.
        let mut t = T::new();
        let (op, bot, w) = (t.op.clone(), t.bot.clone(), t.watcher.clone());
        t.post(&op, CH, Some(&w), NOW - 100);
        assert_eq!(t.tab(CH).mode, ChanMode::Off, "answering the watcher");
        t.post(&bot, CH, None, NOW - 90);
        t.post(&w, CH, None, NOW - 80);
        let bots = [pk(&bot), pk(&w)];
        let is_bot = |p: &str| bots.iter().any(|b| b == p);
        let watchers = &t.lists.trust.watchers.clone();
        assert_eq!(last_bot(&t.store, CH, watchers, is_bot), Some(pk(&bot)));
        assert!(!mentioned_bot(&t.store, &pk(&op), CH, watchers, is_bot));
        t.post(&op, CH, Some(&bot), NOW - 70);
        assert!(mentioned_bot(&t.store, &pk(&op), CH, watchers, is_bot));
    }

    #[test]
    fn dms_and_contested_channels_refuse_a_pick() {
        let mut t = T::new();
        let is_bot = |_: &str| false;
        let pick =
            |t: &T, ch: &str| steps(&t.store, &t.lists, &t.saved, ch, ChanMode::Ideating, is_bot);
        assert_eq!(pick(&t, DM), Err("DMs have no mode"));
        assert_eq!(pick(&t, CH), Ok(vec![Step::Save(ChanMode::Ideating)]));
        t.list("running", NOW - 10);
        let h = t.human.clone();
        let d = format!("buzz-checklist/header/{CH}/{}", pk(&h));
        t.record(
            &h,
            30471,
            d,
            NOW - 5,
            json!({"v": 1, "title": "u", "mode": "ideating"}),
        );
        assert!(!t.lists.driver(CH).contested.is_empty());
        assert!(pick(&t, CH).unwrap_err().contains("pick a driver"));
    }

    #[test]
    fn a_contested_tab_keeps_its_flag_with_no_mode() {
        // Review 755d1328: two lists, no saved pick, no bot mention: the
        // mode is Off, yet the contested 🚩 must stay for `!` and ⚑ N.
        let mut t = T::new();
        t.list("running", NOW - 10);
        let h = t.human.clone();
        let d = format!("buzz-checklist/header/{CH}/{}", pk(&h));
        t.record(
            &h,
            30471,
            d,
            NOW - 5,
            json!({"v": 1, "title": "u", "mode": "ideating"}),
        );
        let tab = t.tab(CH);
        assert_eq!(tab.mode, ChanMode::Off);
        assert_eq!(tab.flag.map(|(f, _)| f), Some(Flag::NeedsYou));
        assert_eq!((tab.icon(true), tab.icon(false)), (Some("🚩"), Some("🚩")));
    }

    #[test]
    fn a_pick_is_dated_after_the_previous_one() {
        let mut t = T::new();
        let op = t.op.clone();
        let at = |b: EventBuilder| b.sign_with_keys(&op).unwrap().created_at.as_secs();
        assert_eq!(at(t.saved.record(CH, ChanMode::Off, NOW)), NOW);
        let first = t
            .saved
            .record(CH, ChanMode::Off, NOW)
            .sign_with_keys(&op)
            .unwrap();
        assert!(t.saved.apply(&first, &pk(&op)));
        // Same second: a second later, so the newer pick wins.
        let next = t
            .saved
            .record(CH, ChanMode::Ideating, NOW)
            .sign_with_keys(&op)
            .unwrap();
        assert_eq!(next.created_at.as_secs(), NOW + 1);
        assert!(t.saved.apply(&next, &pk(&op)));
        assert_eq!(t.saved.get(CH), Some(ChanMode::Ideating));
        assert_eq!(at(t.saved.record(CH, ChanMode::Off, NOW + 50)), NOW + 50);
    }

    #[test]
    fn the_owner_flips_the_drivers_header_to_ideating() {
        let mut t = T::new();
        t.list("running", NOW - 10);
        let flip = t.lists.flip_to_ideating(CH, NOW - 20).unwrap();
        let ev = flip.sign_with_keys(&t.op).unwrap();
        assert_eq!(
            ev.created_at.as_secs(),
            NOW - 9,
            "newer than the version it replaces"
        );
        assert!(t.lists.apply(&ev));
        let h = t.lists.header(CH).unwrap();
        assert_eq!(
            (h.mode, h.creator, h.title),
            (Mode::Ideating, pk(&t.bot), "t".to_string())
        );
    }

    #[test]
    fn picking_a_mode_plans_what_to_publish() {
        use ChanMode::*;
        let mut t = T::new();
        let bot = pk(&t.bot);
        assert_eq!(
            plan(Running, None, None, None),
            Err("no list here, and no bot to ask for one")
        );
        assert_eq!(
            plan(Running, None, None, Some(&bot)),
            Ok(vec![Step::Send {
                text: ASK_LIST_TEXT,
                to: bot.clone()
            }])
        );
        assert_eq!(plan(Off, None, None, None), Ok(vec![Step::Save(Off)]));
        // Review 53509a4d: a saved off would hide the list the bot makes.
        assert_eq!(
            plan(Running, None, Some(Off), Some(&bot)),
            Ok(vec![
                Step::Send {
                    text: ASK_LIST_TEXT,
                    to: bot.clone()
                },
                Step::Save(Ideating)
            ])
        );
        t.list("ideating", NOW);
        let h = t.lists.header(CH);
        assert_eq!(
            plan(Running, h.as_ref(), None, None),
            Ok(vec![Step::Send {
                text: GO_TEXT,
                to: bot.clone()
            }])
        );
        assert_eq!(
            plan(Running, h.as_ref(), Some(Off), None),
            Ok(vec![
                Step::Send {
                    text: GO_TEXT,
                    to: bot.clone()
                },
                Step::Save(Ideating)
            ])
        );
        t.list("running", NOW + 1);
        let h = t.lists.header(CH);
        assert_eq!(plan(Running, h.as_ref(), None, None), Ok(vec![]));
        assert_eq!(
            plan(Ideating, h.as_ref(), None, None),
            Ok(vec![Step::Flip, Step::Save(Ideating)])
        );
        assert_eq!(
            plan(Off, h.as_ref(), None, None),
            Ok(vec![Step::Flip, Step::Save(Off)])
        );
    }

    #[test]
    fn bang_goes_oldest_first_and_round() {
        let f = vec![
            ("b".to_string(), 20),
            ("a".to_string(), 10),
            ("c".to_string(), 30),
        ];
        assert_eq!(next_flagged(&f, None), Some("a"));
        assert_eq!(next_flagged(&f, Some("a")), Some("b"));
        assert_eq!(next_flagged(&f, Some("c")), Some("a"));
        assert_eq!(next_flagged(&f, Some("x")), Some("a"));
        assert_eq!(next_flagged(&[], Some("x")), None);
    }
}
