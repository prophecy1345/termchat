//! One line of editable text with a cursor: the message box, form fields and
//! popups. Zeroized on drop, since it also holds passwords.

use std::ops::Deref;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

#[derive(Default, Clone, PartialEq, Eq)]
pub struct Input {
    text: Zeroizing<String>,
    /// Byte offset, always on a char boundary.
    cursor: usize,
}

impl Deref for Input {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl From<&str> for Input {
    fn from(s: &str) -> Self {
        Self {
            text: Zeroizing::new(s.into()),
            cursor: s.len(),
        }
    }
}

impl Input {
    /// Chars before the cursor, to place it on screen.
    pub fn cursor_chars(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Empties the field and returns what was in it.
    pub fn take(&mut self) -> Zeroizing<String> {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at].char_indices().last().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    /// Start of the word before `at`: skip spaces, then non-spaces.
    fn word_left(&self, at: usize) -> usize {
        let before = self.text[..at].trim_end();
        before.rfind(' ').map_or(0, |i| i + 1)
    }

    fn word_right(&self, at: usize) -> usize {
        let after = &self.text[at..];
        let skip = after.len() - after.trim_start().len();
        after[skip..]
            .find(' ')
            .map_or(self.text.len(), |i| at + skip + i)
    }

    /// Edits on a key; `true` on Enter. Keys it does not know are ignored.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let word = ctrl || k.modifiers.contains(KeyModifiers::ALT);
        let at = self.cursor;
        match k.code {
            KeyCode::Enter => return true,
            KeyCode::Char('w') if ctrl => {
                let from = self.word_left(at);
                self.text.replace_range(from..at, "");
                self.cursor = from;
            }
            KeyCode::Char('u') if ctrl => {
                self.text.replace_range(..at, "");
                self.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace => {
                let from = self.prev(at);
                self.text.replace_range(from..at, "");
                self.cursor = from;
            }
            KeyCode::Delete => {
                let to = self.next(at);
                self.text.replace_range(at..to, "");
            }
            KeyCode::Left if word => self.cursor = self.word_left(at),
            KeyCode::Right if word => self.cursor = self.word_right(at),
            KeyCode::Left => self.cursor = self.prev(at),
            KeyCode::Right => self.cursor = self.next(at),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn press(i: &mut Input, code: KeyCode) {
        i.on_key(key(code, KeyModifiers::NONE));
    }

    #[test]
    fn edits_in_the_middle_and_by_words() {
        let mut i = Input::from("hello wörld");
        press(&mut i, KeyCode::Left);
        press(&mut i, KeyCode::Left);
        press(&mut i, KeyCode::Backspace);
        assert_eq!(&*i, "hello wöld");
        press(&mut i, KeyCode::Char('ö'));
        press(&mut i, KeyCode::Char('r'));
        assert_eq!(&*i, "hello wöörld");
        assert_eq!(i.cursor_chars(), 10);
        i.on_key(key(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(i.cursor_chars(), 6);
        i.on_key(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(&*i, "wöörld");
        press(&mut i, KeyCode::Delete);
        assert_eq!(&*i, "öörld");
        press(&mut i, KeyCode::End);
        i.on_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(&*i, "");
        assert!(i.on_key(key(KeyCode::Enter, KeyModifiers::NONE)));
    }
}
