//! Log in as `BUZZ_PRIVATE_KEY` on `BUZZ_RELAY_URL` and print what the
//! phone would show after a few seconds: tabs, one timeline, agents.

use std::time::Duration;

use buzz_vim_ffi::{Client, Listener, new_read_id};

struct Quiet;
impl Listener for Quiet {
    fn changed(&self) {}
}

fn main() {
    let relay = std::env::var("BUZZ_RELAY_URL").unwrap_or("wss://buzz.gitcoin.co".into());
    let key = std::env::var("BUZZ_PRIVATE_KEY").expect("BUZZ_PRIVATE_KEY");
    let c = Client::new(
        relay,
        key,
        vec![],
        new_read_id(),
        new_read_id(),
        std::env::var("BUZZ_AUTH_TAG").ok(),
        Box::new(Quiet),
    )
    .expect("login");
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if c.ready() && !c.tabs().is_empty() {
            break;
        }
    }
    println!("ready={} conn={:?}", c.ready(), c.conn() as u8);
    let tabs = c.tabs();
    println!(
        "{} tabs: {:?}",
        tabs.len(),
        tabs.iter().map(|t| &t.label).take(8).collect::<Vec<_>>()
    );
    if let Some(t) = tabs.first() {
        c.open_channel(Some(t.id.clone()));
        std::thread::sleep(Duration::from_secs(3));
        let tl = c.timeline(t.id.clone());
        println!(
            "{}: {} messages, loaded={}",
            t.label,
            tl.messages.len(),
            tl.loaded
        );
        if let Some(m) = tl.messages.last() {
            println!(
                "last: {} {:?}",
                m.author_name,
                m.segments.first().map(|_| "…")
            );
        }
    }
    println!(
        "agents: {:?}",
        c.agents().iter().map(|a| &a.name).collect::<Vec<_>>()
    );
    println!("notices: {:?}", c.take_notices());
}
