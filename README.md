# buzz-vim

A small, keyboard-first terminal client for Buzz. Vim-style modes, tabs from your Desktop channel sections,
threads, 👍, @mentions, images inline (iTerm2, Kitty, WezTerm, Ghostty; half-blocks elsewhere), and uploads
that always ask first.

## Install

```sh
cargo install --locked --force --git https://github.com/lucianearth/buzz-vim
buzz-vim login   # once: relay (default wss://buzz.gitcoin.co) and your nsec
buzz-vim
```

`--force` reinstalls even when the version number has not changed. It needs a Rust toolchain
([rustup](https://rustup.rs)); `rust-toolchain.toml` picks the version.

Your nsec is in Buzz Desktop under Settings → Profile → Private key. `login` saves it to
`buzz-vim/config.json` in your config dir (`~/Library/Application Support` on macOS), readable only by you.
`BUZZ_PRIVATE_KEY` and `BUZZ_RELAY_URL` override the saved login.

## Keys

Normal mode shows the common keys in the footer. Press `?` for all of them.

- `i` write, `Esc` back to normal, `⏎` send, `⇧⏎` (or `⌥⏎`, `^j`) new line
- `j` `k` move, `1`–`9` jump to a tab, `H` `L` or `⇥` cycle tabs (tab 0 included)
- `0` is tab 0, **Needs you**, pinned first (it can't be moved, closed or archived). *Needs an answer*: channels
  the watcher marks stalled, waiting on lucian or waiting on release (the row opens its ping), parked checklist
  items with their question, and people's @mentions and DMs to the operator (`owner`) that he hasn't answered.
  *FYI*: bots' @mentions of him the watcher didn't flag. One row each (channel, who, one line, age), oldest
  first. `⏎` opens the row at its message, `r` answers it in a box that posts into that row's thread. A row
  goes once he posts in its thread after it (in a DM: anywhere in it), or the watch state or item moves on.
  It's all from the relay (the last week of mentions), nothing local. Bots are profiles with a NIP-OA `auth`
  tag or a channel `bot` role, agents we see telemetry from, and the watcher
- `!` goes to the next row that needs an answer, oldest first and round again, like `:cnext`. Every channel's
  status line shows how many there are as `⚑ N`
- `n` the bell: the latest message in this tab, read or not. In a thread it opens on your first unread reply
- `<` `>` move the current tab left or right within its section; `T` opens the tab order list (`J` `K` move
  the picked tab, `a` then `y` archives it for everyone). The order is synced: dragging tabs on the phone moves them here too, and the other way round
- `⏎` or `l` open a thread, `r` reply in it, `+` 👍 the selected message; `h`, `←` or `Esc` goes back
- `a` the agents screen: who is working, where, and their last tool calls. A tab spins while an agent
  works in that channel, and a thread shows `taskbot thinking` while an agent answers in it
- `c` the channel's checklist (the operator loop's records, kinds 30470-30472, read-only here: agents write
  them with `buzz checklist`). Items in order with open / done / parked, the release flag and evidence, and
  the picked item's details, evidence links and parked question beside them. Above a channel's messages a
  status line shows its mode (ideating / running), the watcher's state (working, stalled, waiting on lucian,
  waiting on release, with a link to its ping), open / done / parked counts and agent crashes today
- `Q` (or `^q`, which also works while writing) asks a quick question: a box that posts a new top-level message
  in #quick, starting with `[Q]` and @mentioning taskbot. Press `^q` again in the box to add the channel you're
  in (its name, id, link and latest 30 messages' ids) so taskbot can read it; again takes it off. `Esc` keeps
  the text for next time. The answer comes in that message's thread in #quick. If there is no #quick channel
  it says so; buzz-vim never makes one
- `s` pick which sections become tabs (`m` there moves the current channel into the section under the cursor, the same
  move Desktop makes), `R` reload them from the relay
- `^v` paste an image or file from the clipboard; dropping a file does the same. Either way a box shows what it
  found and where it goes (`<relay>/upload`), and nothing uploads until you press `y`.
- Links: click one, or press `f` and type the letter on its chip. A `buzz://` message link opens here: its
  channel's tab, or an extra tab at the end when the channel isn't in your sections (it goes away when you
  leave it). Links marked 🐝 (pr, issue, repo) open in Buzz Desktop, and web links in your browser.
  `⌫` or `^o` goes back to where you were, as many steps as you followed.
- `o` open the selected image full screen (`←` `→` page through the images, `Esc` closes), `q` quit

## iPhone

The iPhone app (`ios/`, on the same core through `ffi/`) has the same tab 0: a ⚑ needs you chip first in the
tab row with the Needs-an-answer count as its badge, the same rows (tap one to open it, Reply to answer it in
its thread), and ⚑ N on every channel's status line. It posts a local notification for each new watcher
row its sync sees (stalled, waiting on lucian, waiting on release), once per row, and only for rows that
began after the phone started watching. People's @mentions and DMs, parked items and FYI wait in tab 0
without one. That works while the app runs.

Q is the button at the top right, beside the online line. It opens the same Q as the terminal's (the same
core function builds the message): a new top-level message in #quick, starting with `[Q]` and @mentioning
taskbot. Tap the Q chip in the sheet to add the channel you're in (its name, id, link, open thread and
latest 30 messages' ids); tap it again to take that off. Closing the sheet keeps the text. Send lands you in
the new thread in #quick, where the answer comes.

Closed, it gets real pushes through upstream Buzz's push (`buzz-push-v1`): the phone enrolls its APNs token
with the push gateway (`Push.gateway` in `ios/App/Push.swift`, App Attest), delegates to the relay's push
key, and publishes a lease (kind 30350, encrypted to the relay) for one thing only: the watcher's messages
that @mention you. @mentions and DMs from anyone else never push. The push itself says nothing;
the notification extension fetches the message and shows it like a tab 0 row. The lease follows the
watcher's keys, renews while the app runs, and goes on logout. While it is live the app posts no local
notification for a watcher row with its ping out, so nothing comes twice; a watcher row without a ping
still gets one. It needs a relay that advertises `buzz-push-v1` in its NIP-11.

## Operator loop settings

In `buzz-vim/config.json` (or the env var):

- `owner` (`BUZZ_VIM_OWNER`): who may edit any checklist record besides its creator. Default lucian.
- `watcher` (`BUZZ_VIM_WATCHER`, comma separated): the watcher's pubkeys; its state counts only from these
  (and the owner). Default the watcher seat (`d84ddba9…`).
- `quick_channel` (`BUZZ_VIM_QUICK_CHANNEL`), default `quick`, and `quick_agent` (`BUZZ_VIM_QUICK_AGENT`),
  default `taskbot`: where Q posts and who it @mentions.

## Development

`./scripts/green.sh` is the gate: `cargo fmt --check`, clippy with `-D warnings`, tests.
`tools/drive.py` drives the real binary in a fake terminal (needs `pyte`); set `BUZZ_VIM_IMAGES=halfblocks`
there, because the fake terminal can't answer image-protocol queries.

## License

MIT. See [LICENSE](LICENSE).
