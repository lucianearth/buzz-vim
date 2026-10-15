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
- `j` `k` move, `0`–`9` jump to a tab, `H` `L` or `⇥` cycle tabs
- `⏎` or `l` open a thread, `r` reply in it, `+` 👍 the selected message; `h`, `←` or `Esc` goes back
- `a` the agents screen: who is working, where, and their last tool calls. The header shows who is working
  from anywhere, and a thread shows `taskbot thinking` while an agent answers in it
- `s` pick which sections become tabs, `R` reload them from the relay
- `^v` paste an image or file from the clipboard; dropping a file does the same. Either way a box shows what it
  found and where it goes (`<relay>/upload`), and nothing uploads until you press `y`.
- `o` open the selected image full screen (`←` `→` page through the images, `Esc` closes), `q` quit

## Development

`./scripts/green.sh` is the gate: `cargo fmt --check`, clippy with `-D warnings`, tests.
`tools/drive.py` drives the real binary in a fake terminal (needs `pyte`); set `BUZZ_VIM_IMAGES=halfblocks`
there, because the fake terminal can't answer image-protocol queries.

## License

MIT. See [LICENSE](LICENSE).
