# buzz-vim

A small, keyboard-first terminal client for Buzz. Vim-style modes, tabs from your Desktop channel sections,
threads, 👍, @mentions, images inline (iTerm2, Kitty, WezTerm, Ghostty; half-blocks elsewhere), and uploads
that always ask first.

## Install

```sh
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://buzz.gitcoin.co/git/f020f0378f36ade38acadcce6c024b4c1bb77f05d607213f2b7674a1a4817da3/buzz-vim.git
buzz-vim login   # once: relay (default wss://buzz.gitcoin.co) and your nsec
buzz-vim
```

`CARGO_NET_GIT_FETCH_WITH_CLI` makes cargo fetch with your own git, so the Buzz git auth (the git nostr
plugin) works. cargo's built-in git client can't authenticate to Buzz.

Your nsec is in Buzz Desktop under Settings → Profile → Private key. `login` saves it to
`buzz-vim/config.json` in your config dir (`~/Library/Application Support` on macOS), readable only by you.
`BUZZ_PRIVATE_KEY` and `BUZZ_RELAY_URL` override the saved login.

## Keys

Normal mode shows the common keys in the footer. Press `?` for all of them.

- `i` write, `Esc` back to normal, `⏎` send, `⌥⏎` or `^j` new line
- `j` `k` move, `1`–`9` jump to a tab, `H` `L` or `⇥` cycle tabs
- `⏎` open a thread, `r` reply in it, `+` 👍 the selected message
- `s` pick which sections become tabs, `R` reload them from the relay
- `^v` paste an image or file from the clipboard; dropping a file does the same. Either way a box shows what it
  found and where it goes (`<relay>/upload`), and nothing uploads until you press `y`.
- `t` show or hide the agents pane, `o` open the selected image, `q` quit

## Development

`./scripts/green.sh` is the gate: `cargo fmt --check`, clippy with `-D warnings`, tests.
`tools/drive.py` drives the real binary in a fake terminal (needs `pyte`); set `BUZZ_VIM_IMAGES=halfblocks`
there, because the fake terminal can't answer image-protocol queries.
