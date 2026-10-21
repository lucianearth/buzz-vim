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
  the watcher marks stalled, waiting on lucian or waiting on release (the row opens its ping), a contested
  channel whose lists run ("two lists claim this channel; pick a driver"), and people's @mentions and DMs to the operator (`owner`) that he hasn't answered.
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
  them with `buzz checklist`). Items in order with open / done, the release flag and evidence, and
  the picked item's details and evidence links beside them. There is no parking: a legacy parked item reads as open work. Above a channel's messages a
  status line shows its mode (ideating / running), the watcher's state (working, stalled, waiting on lucian,
  waiting on release, with a link to its ping), open / done counts and agent crashes today. A channel
  where two or more lists claim the checklist and the owner hasn't picked a driver shows `contested` with
  their creators, and no list until he does
- `C` the Control view: the private #control channel's notes (`buzz control publish`, one per topic, day
  and source; the newest wins, a missing day is a gap). The overview is an ops cockpit: open requests on
  top with their vote keys (a digit votes the picked request, or the only one), a tile per topic (status
  color: ok green, warn amber, page red, no note dim; the newest note's first line; a 14-day sparkline of
  the topic's key metric: hooks cost per useful call, converge held runs, fleet-audit problems, upgrade
  events, else notes per day), then the newest hooks note's gauges (value, baseline, delta: green better,
  amber flag, red page; a flag is never green) and its seats. Tiles reflow down to 80 columns. `j` `k`
  move, `⏎` a request's note or a topic's days, `⏎` a day's note: its summary, images and details. hooks/1
  notes get the gauges, trend charts (one point per day, the baseline dashed), a hooks-alive strip, seats with dead hooks highlighted, agent types with their ⚑ flag
  and ramp by repo; any other schema shows its JSON. A request shows its actions as `1`–`9` with votes and
  status: pressing one asks, then sends that reaction as your vote (only approvers', when it names any).
  `r` reloads. The channel is `control_channel` (`BUZZ_VIM_CONTROL_CHANNEL`), default #control
- `q` or `Q` (or `^q`, which also works while writing) asks a quick question: a box that posts a new top-level message
  in #quick, starting with `[Q]` and @mentioning taskbot. Press `^q` again in the box to add the channel you're
  in (its name, id, link and latest 30 messages' ids) so taskbot can read it; again takes it off. A DM or
  private channel never goes along. `Esc` keeps
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
  Code blocks get a chip too: picking one copies the code.
- `o` open the selected image full screen (`←` `→` page through the images, `Esc` closes), `^c` quit

## iPhone

The iPhone app (`ios/`, on the same core through `ffi/`) has the same tab 0: a ⚑ needs you chip first in the
tab row with the Needs-an-answer count as its badge, the same rows (tap one to open it, Reply to answer it in
its thread), and ⚑ N on every channel's status line. It posts a local notification for each new watcher
row its sync sees (stalled, waiting on lucian, waiting on release), once per row, and only for rows that
began after the phone started watching. People's @mentions and DMs and FYI wait in tab 0
without one. That works while the app runs.

Q is the button at the top right, beside the online line. It opens the same Q as the terminal's (the same
core function builds the message): a new top-level message in #quick, starting with `[Q]` and @mentioning
taskbot. Tap the Q chip in the sheet to add the channel you're in, unless it is a DM or private (its
name, id, link, open thread and latest 30 messages' ids); tap it again to take that off. Closing the sheet keeps the text. Send lands you in
the new thread in #quick, where the answer comes.

The control chip, last in the tab row after routing, is the terminal's Control view (`C`) in the same
ops-cockpit style, from the same core: the open requests on top in an amber card with a button per declared
action (a tap reacts with that action's emoji, which is the vote; a dim button says why you can't vote it),
a tile per topic (status color and glyph, the worst note's line, a 14-day sparkline of the topic's key
metric), the newest hooks note's gauges (value, baseline, delta in its tone, 14 days) and its seats. A tile
opens the topic's days, a day its note (the hooks note has its status by day, gauges, trend charts and
tables; any other schema shows its details as JSON). Pull down to read #control again.

Closed, it gets real pushes through upstream Buzz's push (`buzz-push-v1`): the phone enrolls its APNs token
with the push gateway (`Push.gateway` in `ios/App/Push.swift`, App Attest), delegates to the relay's push
key, and publishes a lease (kind 30350, encrypted to the relay) for one thing only: the watcher's messages
that @mention you. @mentions and DMs from anyone else never push. The push itself says nothing;
the notification extension fetches the message and shows it like a tab 0 row. The lease follows the
watcher's keys, renews while the app runs, and goes on logout (a logout offline finishes at the next
launch or foreground: the gateway forgets the relay key, and no new lease goes out before that). While it
is live the app posts no local notification for a watcher row with its ping out, so nothing comes twice;
a watcher row without a ping still gets one. It needs a relay that advertises `buzz-push-v1` in its NIP-11.

## Operator loop settings

In `buzz-vim/config.json` (or the env var):

- `owner` (`BUZZ_VIM_OWNER`): who may edit any checklist record besides its creator, and who picks the
  driving list when two claim a channel. Default lucian.
- `watcher` (`BUZZ_VIM_WATCHER`, comma separated; in config.json a list or a comma-separated string): the
  watcher's pubkeys; its state counts only from these. Default the watcher seat (`d84ddba9…`). A value of
  another type is ignored with a warning, and the default used.
- `control_channel` (`BUZZ_VIM_CONTROL_CHANNEL`): the channel id the Control view (`C`) reads, default
  #control (`ee7289a1-…`).
- `quick_channel` (`BUZZ_VIM_QUICK_CHANNEL`), default `quick`, and `quick_agent` (`BUZZ_VIM_QUICK_AGENT`),
  default `taskbot`: where Q posts and who it @mentions.

## Development

`./scripts/green.sh` is the gate: `cargo fmt --check`, clippy with `-D warnings`, tests.
`tools/drive.py` drives the real binary in a fake terminal (needs `pyte`); set `BUZZ_VIM_IMAGES=halfblocks`
there, because the fake terminal can't answer image-protocol queries.

## License

MIT. See [LICENSE](LICENSE).
