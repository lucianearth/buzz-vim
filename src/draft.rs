//! The compose box: a small multi-line editor plus the bits a message needs
//! besides text (mentions picked from autocomplete, attached uploads).

use crate::relay::BlobDescriptor;

#[derive(Default)]
pub struct Draft {
    pub text: String,
    /// Byte offset of the cursor, always on a char boundary.
    pub cursor: usize,
    /// (label as typed, pubkey). Kept only while the label is still in the text.
    pub mentions: Vec<(String, String)>,
    pub attachments: Vec<BlobDescriptor>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }

    /// Put an unsent draft back in front of whatever is being typed.
    pub fn restore(&mut self, mut old: Draft) {
        if self.is_empty() {
            *self = old;
            return;
        }
        let shift = old.text.len() + 1;
        old.text.push('\n');
        self.text.insert_str(0, &old.text);
        self.cursor += shift;
        self.mentions.append(&mut old.mentions);
        old.attachments.append(&mut self.attachments);
        self.attachments = old.attachments;
    }

    pub fn clear(&mut self) {
        *self = Draft::default();
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.replace_range(i..self.cursor, "");
            self.cursor = i;
        }
    }

    pub fn delete(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text
                .replace_range(self.cursor..self.cursor + c.len_utf8(), "");
        }
    }

    pub fn left(&mut self) {
        if let Some((i, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = i;
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Up/down across lines, keeping the column where possible.
    pub fn vertical(&mut self, down: bool) -> bool {
        let line_start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let col = self.text[line_start..self.cursor].chars().count();
        let target_start = if down {
            match self.text[self.cursor..].find('\n') {
                Some(i) => self.cursor + i + 1,
                None => return false,
            }
        } else {
            if line_start == 0 {
                return false;
            }
            self.text[..line_start - 1].rfind('\n').map_or(0, |i| i + 1)
        };
        let line_end = self.text[target_start..]
            .find('\n')
            .map_or(self.text.len(), |i| target_start + i);
        let mut pos = target_start;
        for (n, (i, c)) in self.text[target_start..line_end].char_indices().enumerate() {
            if n == col {
                break;
            }
            pos = target_start + i + c.len_utf8();
        }
        self.cursor = pos;
        true
    }

    /// The `@partial` being typed right before the cursor, if any.
    pub fn mention_query(&self) -> Option<(usize, &str)> {
        let before = &self.text[..self.cursor];
        let at = before.rfind('@')?;
        let q = &before[at + 1..];
        let starts_word = at == 0 || before[..at].ends_with(char::is_whitespace);
        (starts_word && !q.contains('\n') && q.chars().count() <= 32 && !q.ends_with(' '))
            .then_some((at, q))
    }

    /// Replace the `@partial` with `@Full Name ` and remember who it is.
    pub fn complete_mention(&mut self, at: usize, name: &str, pubkey: &str) {
        let label = format!("@{name}");
        self.text
            .replace_range(at..self.cursor, &format!("{label} "));
        self.cursor = at + label.len() + 1;
        self.mentions.push((label, pubkey.to_string()));
    }

    /// Pubkeys to p-tag: picked mentions whose label survived editing.
    pub fn mention_pubkeys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (label, pk) in &self.mentions {
            if self.text.contains(label.as_str()) && !out.contains(pk) {
                out.push(pk.clone());
            }
        }
        out
    }

    /// True when the cursor sits inside an unclosed ``` block, so Enter
    /// should break the line instead of sending half a snippet.
    pub fn in_code_fence(&self) -> bool {
        self.text[..self.cursor]
            .split('\n')
            .filter(|l| l.trim_start().starts_with("```"))
            .count()
            % 2
            == 1
    }

    /// (line, display column) of the cursor, for drawing. Tabs count as
    /// `TAB` spaces, matching how the compose box draws them.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().map_or(0, |l| {
            unicode_width::UnicodeWidthStr::width(expand_tabs(l).as_str())
        });
        (line, col)
    }
}

pub const TAB: usize = 4;

pub fn expand_tabs(s: &str) -> String {
    s.replace('\t', &" ".repeat(TAB))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_mentions() {
        let mut d = Draft::default();
        d.insert("hi @jud");
        let (at, q) = d.mention_query().unwrap();
        assert_eq!(q, "jud");
        d.complete_mention(at, "judgebot", "pk1");
        assert_eq!(d.text, "hi @judgebot ");
        d.insert("ok");
        assert_eq!(d.mention_pubkeys(), vec!["pk1".to_string()]);
        d.delete_word();
        d.delete_word();
        assert_eq!(d.text, "hi ");
        assert!(d.mention_pubkeys().is_empty());
        d.insert("é\nab");
        d.left();
        assert!(d.vertical(false));
        assert_eq!(d.cursor_line_col(), (0, 1));
        d.backspace();
        assert_eq!(d.text, "i é\nab");
        assert!(d.mention_query().is_none());
        let mut e = Draft::default();
        e.insert("mail me@host");
        assert!(e.mention_query().is_none());
        let mut c = Draft::default();
        c.insert("look:\n```rust\n\tfn x() {}");
        assert!(c.in_code_fence());
        assert_eq!(c.cursor_line_col(), (2, 4 + 9));
        c.insert("\n```");
        assert!(!c.in_code_fence());
    }
}
