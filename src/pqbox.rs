//! pq in the terminal: `P` asks for a secret (a REQUEST in this channel or
//! thread), `p` on a message answers its REQUEST (sealed reply in its
//! thread) or opens its MESSAGE. Keys live in pq's own inbox, so the `pq`
//! CLI and buzz-vim can open each other's requests.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use buzz_vim_core::draft::Draft;
use buzz_vim_core::pq::{self, Piece};

use crate::app::{Action, App};
use crate::keys::Mode;

/// Where a sealed reply goes: the request's channel and thread, and who
/// asked (tagged, so they hear about it).
#[derive(Clone)]
pub struct Reply {
    pub req: pq::Request,
    pub channel: String,
    pub root: String,
    pub asker: String,
}

pub enum PqBox {
    /// `P`: what the secret is for.
    Ask(Draft),
    /// `p` on a REQUEST: the secret, typed or pasted (shown as `*`), or a
    /// file's path (`file`, shown as typed).
    Answer { to: Reply, input: Draft, file: bool },
    /// Last look before it is sealed and posted.
    Confirm {
        to: Reply,
        data: Vec<u8>,
        name: String,
    },
    /// `p` on a MESSAGE: what it held. Its key `id` burns when this
    /// closes, so it can be copied or saved first.
    Opened {
        name: String,
        data: Vec<u8>,
        id: String,
    },
}

impl App {
    /// `P`: ask for a secret here.
    pub fn pq_ask(&mut self) {
        if self.draft_target().is_none() {
            return self.say("open a channel first");
        }
        self.pq = Some(PqBox::Ask(Draft::default()));
        self.mode = Mode::Pq;
    }

    /// `p`: answer the cursor message's REQUEST, or open its MESSAGE.
    pub fn pq_act(&mut self) {
        let Some(id) = self.cursor_message() else {
            return;
        };
        let Some((ch, m)) = self.store.find_message(&id) else {
            return;
        };
        let piece = pq::parse_blocks(&m.content)
            .into_iter()
            .find(|p| !matches!(p, Piece::Text(_)));
        let (channel, root, asker) = (
            ch.id.clone(),
            m.root.clone().unwrap_or(m.id.clone()),
            m.author.clone(),
        );
        match piece {
            Some(Piece::Request(req)) => {
                let to = Reply {
                    req,
                    channel,
                    root,
                    asker,
                };
                self.pq = Some(PqBox::Answer {
                    to,
                    input: Draft::default(),
                    file: false,
                });
                self.mode = Mode::Pq;
            }
            Some(Piece::Message(msg)) => self.pq_open(&msg),
            _ => self.say("p: no pq request or sealed message here"),
        }
    }

    fn pq_open(&mut self, msg: &pq::Message) {
        let Some(store) = &self.pq_store else {
            return self.say("no pq key store (set HOME or PQ_HOME)");
        };
        let rec = match store.load(&msg.header.id) {
            Ok(Some(rec)) => rec,
            Ok(None) => return self.say("no key here (another device, or already opened)"),
            Err(e) => return self.say(format!("couldn't read the key: {e}")),
        };
        let data = match pq::open(msg, &rec) {
            Ok(d) => d,
            Err(e) => return self.say(format!("couldn't open it: {e}")),
        };
        self.pq = Some(PqBox::Opened {
            name: msg.header.shown_name(),
            data,
            id: rec.id,
        });
        self.mode = Mode::Pq;
    }

    pub fn pq_paste(&mut self, text: &str) {
        match &mut self.pq {
            Some(PqBox::Ask(d)) | Some(PqBox::Answer { input: d, .. }) => d.insert(text),
            _ => {}
        }
    }

    /// Close the box. An opened secret's key burns now, as pq burns once
    /// it has delivered: one look, but a whole one.
    pub(crate) fn pq_close(&mut self) {
        if let Some(PqBox::Opened { id, .. }) = self.pq.take()
            && let Some(store) = &self.pq_store
            && let Err(e) = store.burn(&id)
        {
            self.say(format!("couldn't burn the key: {e}"));
        }
        self.mode = Mode::Normal;
    }

    pub fn key_pq(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let Some(b) = &mut self.pq else {
            return self.pq_close();
        };
        match b {
            PqBox::Ask(d) | PqBox::Answer { input: d, .. } => match k.code {
                KeyCode::Esc => self.pq_close(),
                KeyCode::Enter => self.pq_enter(),
                KeyCode::Tab => {
                    if let PqBox::Answer { file, input, .. } = b {
                        *file = !*file;
                        input.clear();
                    }
                }
                KeyCode::Char('u') if ctrl => d.clear(),
                KeyCode::Char('w') if ctrl => d.delete_word(),
                KeyCode::Char(c) if !ctrl => d.insert(c.encode_utf8(&mut [0; 4])),
                KeyCode::Backspace => d.backspace(),
                KeyCode::Left => d.left(),
                KeyCode::Right => d.right(),
                _ => {}
            },
            PqBox::Confirm { .. } => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.pq_enter(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.pq_close();
                    self.say("not sent");
                }
                _ => {}
            },
            PqBox::Opened { name, data, .. } => match k.code {
                KeyCode::Char('y') => match std::str::from_utf8(data) {
                    Ok(t) => {
                        let t = t.to_string();
                        self.actions.push(Action::Copy(t));
                    }
                    Err(_) => self.say("it isn't text; s saves it"),
                },
                KeyCode::Char('s') => {
                    let r = save(name, data);
                    match r {
                        Ok(p) => self.say(format!("saved {}", p.display())),
                        Err(e) => self.say(format!("couldn't save: {e}")),
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.pq_close(),
                _ => {}
            },
        }
    }

    fn pq_enter(&mut self) {
        match self.pq.take() {
            Some(PqBox::Ask(d)) => {
                let Some((channel, root)) = self.draft_target() else {
                    return self.pq_close();
                };
                let Some(store) = &self.pq_store else {
                    self.pq_close();
                    return self.say("no pq key store (set HOME or PQ_HOME)");
                };
                let rec = pq::request(d.text.trim());
                if let Err(e) = store.save(&rec) {
                    self.pq_close();
                    return self.say(format!("couldn't keep the key: {e}"));
                }
                let content = rec.request().block();
                self.actions.push(Action::SendPq {
                    channel,
                    root,
                    content,
                    to: vec![],
                });
                self.pq_close();
                self.say(format!("asked; the key is in pq's inbox as {}", rec.id));
            }
            Some(PqBox::Answer { to, input, file }) => {
                let read = if file {
                    let path = expand_home(input.text.trim());
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    read_small_file(&path).map(|d| (d, name.unwrap_or_else(|| "file".into())))
                } else {
                    Ok((input.text.clone().into_bytes(), "message.txt".to_string()))
                };
                match read {
                    Ok((data, _)) if data.is_empty() => {
                        self.pq = Some(PqBox::Answer { to, input, file });
                        self.say("nothing to send");
                    }
                    Ok((data, name)) => self.pq = Some(PqBox::Confirm { to, data, name }),
                    Err(e) => {
                        self.pq = Some(PqBox::Answer { to, input, file });
                        self.say(format!("couldn't read it: {e}"));
                    }
                }
            }
            Some(PqBox::Confirm { to, data, name }) => {
                match pq::seal(&to.req, &data, &name) {
                    Ok(content) => self.actions.push(Action::SendPq {
                        channel: to.channel,
                        root: Some(to.root),
                        content,
                        to: vec![to.asker],
                    }),
                    Err(e) => self.say(format!("couldn't seal it: {e}")),
                }
                self.pq_close();
            }
            other => {
                self.pq = other;
            }
        }
    }
}

fn expand_home(p: &str) -> std::path::PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => std::path::Path::new(&h).join(rest),
        _ => p.into(),
    }
}

/// The most a sealed message carries, as on iOS.
const MAX_SEND: u64 = 1 << 20;

/// A regular file of at most [`MAX_SEND`] bytes. Checked before it is
/// opened: a FIFO would block the UI and `/dev/zero` never ends.
fn read_small_file(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, Read};
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(Error::other("not a regular file"));
    }
    let too_big = || Error::other("too big to send in a message (over 1 MB)");
    if meta.len() > MAX_SEND {
        return Err(too_big());
    }
    // It may have grown since: read one byte past the cap to tell.
    let mut data = vec![];
    std::fs::File::open(path)?
        .take(MAX_SEND + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_SEND {
        return Err(too_big());
    }
    Ok(data)
}

/// Where any download goes ([`crate::sync::download_path`]). `name` is
/// already `Header::shown_name`: no path, no dotfile.
fn save(name: &str, data: &[u8]) -> anyhow::Result<std::path::PathBuf> {
    let path = crate::sync::download_path(name)?;
    std::fs::write(&path, data)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn app(keys: &Keys, home: &std::path::Path) -> App {
        let mut app = App::new(keys.public_key().to_hex(), None);
        app.tabs = vec!["c1".into()];
        app.pq_store = Some(pq::Store {
            home: home.to_path_buf(),
        });
        app
    }

    fn post(apps: &mut [&mut App], by: &Keys, content: &str) -> String {
        let ev = EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", "c1"]).unwrap()])
            .sign_with_keys(by)
            .unwrap();
        for a in apps {
            a.store.apply(&ev);
        }
        ev.id.to_hex()
    }

    fn keys(app: &mut App, codes: &[KeyCode]) {
        for c in codes {
            app.on_term(crossterm::event::Event::Key(KeyEvent::from(*c)));
        }
    }

    fn typed(app: &mut App, s: &str) {
        for c in s.chars() {
            keys(app, &[KeyCode::Char(c)]);
        }
    }

    fn sent(app: &mut App) -> (Option<String>, String, Vec<String>) {
        match app.actions.pop() {
            Some(Action::SendPq {
                channel,
                root,
                content,
                to,
            }) if channel == "c1" => (root, content, to),
            _ => panic!("no pq send"),
        }
    }

    fn status(app: &App) -> &str {
        app.status.as_ref().map_or("", |(s, _)| s.as_str())
    }

    #[test]
    fn ask_answer_and_open_round_trip() {
        let tmp = std::env::temp_dir().join(format!("pqbox-{}", std::process::id()));
        let (me, them) = (Keys::generate(), Keys::generate());
        let (mut a, mut b) = (app(&me, &tmp.join("me")), app(&them, &tmp.join("them")));

        keys(&mut a, &[KeyCode::Char('P')]);
        typed(&mut a, "db password");
        keys(&mut a, &[KeyCode::Enter]);
        assert_eq!(a.mode, Mode::Normal);
        let (root, request, to) = sent(&mut a);
        assert_eq!((root, to), (None, vec![]));
        let req = pq::Request::from_block(&request).unwrap();
        assert_eq!(req.label, "db password");
        let _ = post(&mut [&mut a, &mut b], &me, &request);

        // They answer: typed, never shown, confirmed, sealed to the asker.
        keys(&mut b, &[KeyCode::Char('p')]);
        typed(&mut b, "hunter2");
        keys(&mut b, &[KeyCode::Enter]);
        assert!(
            matches!(&b.pq, Some(PqBox::Confirm { data, name, .. }) if data == b"hunter2" && name == "message.txt")
        );
        keys(&mut b, &[KeyCode::Enter]);
        let (root, sealed, to) = sent(&mut b);
        assert!(root.is_some());
        assert_eq!(to, vec![me.public_key().to_hex()]);
        let id = post(&mut [&mut a, &mut b], &them, &sealed);
        a.selected.insert("c1".into(), id);

        // The key outlives the open and a copy; closing burns it, and a
        // second look has nothing to open with.
        keys(&mut a, &[KeyCode::Char('p')]);
        assert!(matches!(&a.pq, Some(PqBox::Opened { data, .. }) if data == b"hunter2"));
        keys(&mut a, &[KeyCode::Char('y')]);
        assert!(matches!(a.actions.last(), Some(Action::Copy(t)) if t == "hunter2"));
        let key = |a: &App| a.pq_store.as_ref().unwrap().load(&req.id).unwrap();
        assert!(key(&a).is_some());
        keys(&mut a, &[KeyCode::Esc]);
        assert_eq!(key(&a), None);
        keys(&mut a, &[KeyCode::Char('p')]);
        assert_eq!(
            status(&a),
            "no key here (another device, or already opened)"
        );
        assert_eq!(a.mode, Mode::Normal);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_pasted_secret_keeps_its_exact_bytes() {
        let tmp = std::env::temp_dir().join(format!("pqbox-paste-{}", std::process::id()));
        let (me, them) = (Keys::generate(), Keys::generate());
        let mut b = app(&them, &tmp);
        post(&mut [&mut b], &me, &pq::request("x").request().block());
        keys(&mut b, &[KeyCode::Char('p')]);
        b.on_term(crossterm::event::Event::Paste("a\r\nb\rc\n".into()));
        keys(&mut b, &[KeyCode::Enter]);
        assert!(matches!(&b.pq, Some(PqBox::Confirm { data, .. }) if data == b"a\r\nb\rc\n"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_file_answer_sends_the_file_under_its_name() {
        let tmp = std::env::temp_dir().join(format!("pqbox-file-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("id_ed25519");
        std::fs::write(&file, b"\x00key").unwrap();
        let (me, them) = (Keys::generate(), Keys::generate());
        let mut b = app(&them, &tmp);
        post(
            &mut [&mut b],
            &me,
            &pq::request("ssh key").request().block(),
        );
        keys(&mut b, &[KeyCode::Char('p'), KeyCode::Tab]);
        typed(&mut b, file.to_str().unwrap());
        keys(&mut b, &[KeyCode::Enter]);
        assert!(
            matches!(&b.pq, Some(PqBox::Confirm { data, name, .. }) if data == b"\x00key" && name == "id_ed25519")
        );
        keys(&mut b, &[KeyCode::Esc]);
        assert_eq!((b.mode, status(&b)), (Mode::Normal, "not sent"));
        keys(&mut b, &[KeyCode::Char('p'), KeyCode::Enter]);
        assert_eq!(status(&b), "nothing to send");

        // Not a regular file, or over 1 MB: refused before reading, and
        // the box stays open to try another.
        let big = tmp.join("big");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_SEND + 1)
            .unwrap();
        let fifo = tmp.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        keys(&mut b, &[KeyCode::Tab]);
        for (path, why) in [
            (&tmp, "not a regular file"),
            (&fifo, "not a regular file"),
            (&big, "too big to send in a message (over 1 MB)"),
        ] {
            b.on_term(crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
            )));
            typed(&mut b, path.to_str().unwrap());
            keys(&mut b, &[KeyCode::Enter]);
            assert_eq!(status(&b), format!("couldn't read it: {why}"));
            assert!(matches!(b.pq, Some(PqBox::Answer { file: true, .. })));
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
