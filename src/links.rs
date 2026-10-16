//! Links in message text: finding them, where they point, and opening the
//! ones we can't show ourselves.

/// Where a link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A channel, or a message in one, which we show ourselves.
    /// `buzz://message?channel=…&id=…[&thread=…]` or `buzz://channel/<id>[/<msg>]`.
    Message {
        channel: String,
        id: Option<String>,
        thread: Option<String>,
    },
    /// Any other `buzz://` link (pr, issue, repo, …): Buzz Desktop opens it.
    Desktop,
    /// http(s): the browser.
    Web,
}

pub fn target(url: &str) -> Target {
    let Some(rest) = url.strip_prefix("buzz://") else {
        return Target::Web;
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let param = |k: &str| {
        query
            .split('&')
            .filter_map(|p| p.split_once('='))
            .find(|(key, v)| *key == k && !v.is_empty())
            .map(|(_, v)| v.to_string())
    };
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    match segs.next() {
        Some("message") => match param("channel") {
            Some(channel) => Target::Message {
                channel,
                id: param("id"),
                thread: param("thread"),
            },
            None => Target::Desktop,
        },
        Some("channel") => match segs.next() {
            Some(c) => Target::Message {
                channel: c.to_string(),
                id: segs.next().map(str::to_string),
                thread: None,
            },
            None => Target::Desktop,
        },
        _ => Target::Desktop,
    }
}

/// The short label a bare `buzz://` link shows instead of its whole URL,
/// given a way to name channels. None keeps the URL.
pub fn label(url: &str, channel_name: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let rest = url.strip_prefix("buzz://")?;
    match target(url) {
        Target::Message {
            channel,
            id,
            thread,
        } => {
            let name = channel_name(&channel).unwrap_or_else(|| "channel".into());
            Some(match (id, thread) {
                (_, Some(_)) => format!("#{name} › thread"),
                (Some(_), None) => format!("#{name} › message"),
                (None, None) => format!("#{name}"),
            })
        }
        Target::Desktop => {
            let (host, query) = rest.split_once('?').unwrap_or((rest, ""));
            let get = |k: &str| {
                query
                    .split('&')
                    .filter_map(|p| p.split_once('='))
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| v)
            };
            let host = host.trim_end_matches('/');
            Some(match (get("d"), get("commit")) {
                (Some(d), Some(c)) => format!("{d} @ {}", c.chars().take(7).collect::<String>()),
                (Some(d), None) => format!("{d} {host}"),
                _ if host.is_empty() => return None,
                _ => host.to_string(),
            })
        }
        Target::Web => None,
    }
}

/// A link in a run of text: the bytes it covers, what to show, and the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub url: String,
}

const SCHEMES: &[&str] = &["https://", "http://", "buzz://"];

fn scheme_at(s: &str) -> bool {
    SCHEMES.iter().any(|p| s.starts_with(p))
}

/// The link starting at the front of `s`, if there is one: a markdown
/// `[text](url)` or a bare URL.
pub fn at(s: &str) -> Option<Found> {
    if let Some(inner) = s.strip_prefix('[') {
        let close = inner.find("](")?;
        let text = &inner[..close];
        if text.contains('[') {
            return None;
        }
        let after = &inner[close + 2..];
        let end = after.find(')')?;
        let url = &after[..end];
        if !scheme_at(url) || url.contains(char::is_whitespace) || text.contains('\n') {
            return None;
        }
        return Some(Found {
            start: 0,
            end: 1 + close + 2 + end + 1,
            text: text.to_string(),
            url: url.to_string(),
        });
    }
    if !scheme_at(s) {
        return None;
    }
    let mut end = s
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
        .unwrap_or(s.len());
    // Sentence punctuation after a URL isn't part of it; a ')' is only when
    // the URL opened one.
    while let Some(c) = s[..end].chars().last() {
        let unbalanced = c == ')' && s[..end].matches('(').count() < s[..end].matches(')').count();
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'') || unbalanced {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    let url = &s[..end];
    if SCHEMES.contains(&url) {
        return None;
    }
    Some(Found {
        start: 0,
        end,
        text: url.to_string(),
        url: url.to_string(),
    })
}

/// Hand a URL to the OS: the browser for the web, Buzz Desktop for `buzz://`.
pub fn open(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let (prog, args) = opener(std::env::consts::OS);
    let mut child = Command::new(prog)
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reap it so it doesn't linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The program and leading args that open a URL on `os`; the URL goes last,
/// as its own argument.
fn opener(os: &str) -> (&'static str, &'static [&'static str]) {
    match os {
        "macos" => ("open", &[]),
        // No shell: `cmd /C start` would run `&`, `|` and `%VAR%` in the URL.
        "windows" => ("rundll32", &["url.dll,FileProtocolHandler"]),
        _ => ("xdg-open", &[]),
    }
}

/// Hint labels for `n` links, home row first, all the same length so none
/// is a prefix of another.
pub fn hint_labels(n: usize) -> Vec<String> {
    const KEYS: &str = "asdfjklghqweruiopzxcvnm";
    let keys: Vec<char> = KEYS.chars().collect();
    if n <= keys.len() {
        return keys.iter().take(n).map(|c| c.to_string()).collect();
    }
    keys.iter()
        .flat_map(|a| keys.iter().map(move |b| format!("{a}{b}")))
        .take(n)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(
            target("buzz://message?channel=c1&id=m1&thread=r1"),
            Target::Message {
                channel: "c1".into(),
                id: Some("m1".into()),
                thread: Some("r1".into())
            }
        );
        assert_eq!(
            target("buzz://channel/c1"),
            Target::Message {
                channel: "c1".into(),
                id: None,
                thread: None
            }
        );
        assert_eq!(target("buzz://pr?id=abc"), Target::Desktop);
        assert_eq!(target("buzz://repo?owner=o&d=buzz-vim"), Target::Desktop);
        assert_eq!(target("https://example.com"), Target::Web);
    }

    #[test]
    fn finds_bare_and_markdown_links() {
        let f = at("https://a.com/x). more").unwrap();
        assert_eq!(f.url, "https://a.com/x");
        let f = at("https://en.wikipedia.org/wiki/Vim_(text_editor), ok").unwrap();
        assert_eq!(f.url, "https://en.wikipedia.org/wiki/Vim_(text_editor)");
        let f = at("[the docs](https://a.com/d) after").unwrap();
        assert_eq!(
            (f.text.as_str(), f.url.as_str(), f.end),
            ("the docs", "https://a.com/d", 27)
        );
        assert!(at("[x](not a link)").is_none());
        // A stray bracket isn't the start of the markdown link after it.
        assert!(at("[1] see [docs](https://x.com)").is_none());
        assert_eq!(at("[docs](https://x.com)").unwrap().text, "docs");
        assert!(at("https:// nothing").is_none());
        assert!(at("plain").is_none());
    }

    #[test]
    fn labels() {
        let name = |c: &str| (c == "c1").then(|| "general".to_string());
        assert_eq!(
            label("buzz://message?channel=c1&id=m", &name).as_deref(),
            Some("#general › message")
        );
        assert_eq!(
            label(
                "buzz://repo?owner=o&d=buzz-vim&tab=commits&commit=d5f0f354787",
                &name
            )
            .as_deref(),
            Some("buzz-vim @ d5f0f35")
        );
        assert_eq!(label("buzz://pr?id=x", &name).as_deref(), Some("pr"));
        // Untrusted text: no byte slicing through a character.
        assert_eq!(
            label("buzz://repo?d=x&commit=ééééé", &name).as_deref(),
            Some("x @ ééééé")
        );
        assert_eq!(label("https://a.com", &name), None);
        // Nothing to show: keep the URL so the link stays clickable.
        assert_eq!(label("buzz://?a=b", &name), None);
    }

    #[test]
    fn opening_never_goes_through_a_shell() {
        for os in ["windows", "macos", "linux"] {
            let (prog, args) = opener(os);
            assert!(!["cmd", "sh", "powershell"].contains(&prog), "{os}");
            assert!(args.iter().all(|a| !a.contains("start")), "{os}");
        }
    }

    #[test]
    fn hint_labels_never_prefix_each_other() {
        assert_eq!(hint_labels(3), vec!["a", "s", "d"]);
        let many = hint_labels(40);
        assert_eq!(many.len(), 40);
        assert!(many.iter().all(|l| l.len() == 2));
    }
}
