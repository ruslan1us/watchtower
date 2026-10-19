//! Command keys under a Cyrillic keyboard layout (doc/0011).
//!
//! Without the kitty keyboard protocol a terminal reports the character a
//! key types, not the physical key, so with a Russian or Ukrainian layout
//! active the `n` key arrives as `т`. Tower maps such characters back to the
//! Latin character on the same physical key of a US QWERTY keyboard, using
//! the standard ЙЦУКЕН layouts (RU and UA, as in xkb `ru` and `ua`).
//!
//! The map is only applied where a key is a command:
//!
//! - [`command_key`] in command contexts (the list, confirmations, pickers,
//!   overlays, the quit question): plain letters and the RU `.` / `,` of the
//!   `/` key are translated.
//! - [`shortcut_key`] everywhere else, including text fields: only a key with
//!   `Ctrl` or `Alt` held is translated, so typed Cyrillic text stays as is.
//!
//! Both are idempotent and leave Latin input unchanged.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The Latin character on the same physical key as the Cyrillic `c` in the
/// standard RU or UA ЙЦУКЕН layout; shifted letters give the shifted
/// character (`К` → `R`, `Ж` → `:`). `None` for anything else.
pub fn latin_for(c: char) -> Option<char> {
    let latin = match c {
        // Top row.
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ъ' | 'ї' => ']',
        'Й' => 'Q',
        'Ц' => 'W',
        'У' => 'E',
        'К' => 'R',
        'Е' => 'T',
        'Н' => 'Y',
        'Г' => 'U',
        'Ш' => 'I',
        'Щ' => 'O',
        'З' => 'P',
        'Х' => '{',
        'Ъ' | 'Ї' => '}',
        // Home row.
        'ф' => 'a',
        'ы' | 'і' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'э' | 'є' => '\'',
        'Ф' => 'A',
        'Ы' | 'І' => 'S',
        'В' => 'D',
        'А' => 'F',
        'П' => 'G',
        'Р' => 'H',
        'О' => 'J',
        'Л' => 'K',
        'Д' => 'L',
        'Ж' => ':',
        'Э' | 'Є' => '"',
        // Bottom row.
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',
        'Я' => 'Z',
        'Ч' => 'X',
        'С' => 'C',
        'М' => 'V',
        'И' => 'B',
        'Т' => 'N',
        'Ь' => 'M',
        'Б' => '<',
        'Ю' => '>',
        // RU `ё` is left of `1`; UA `ґ` is on the backslash key.
        'ё' => '`',
        'Ё' => '~',
        'ґ' => '\\',
        'Ґ' => '|',
        _ => return None,
    };
    Some(latin)
}

/// The RU layout types `.` and `,` (with Shift) on the key that is `/` and
/// `?` in QWERTY. Only for contexts where `.` and `,` are not bound.
fn latin_punctuation(c: char) -> Option<char> {
    match c {
        '.' => Some('/'),
        ',' => Some('?'),
        _ => None,
    }
}

/// Normalises a key in a command context: a Cyrillic letter becomes the
/// Latin character on the same key, with or without modifiers, and the RU
/// `.` / `,` become `/` / `?`. Each key is translated at most once, so `ю`
/// (the `.` key) becomes `.`, not `/`.
pub fn command_key(key: KeyEvent) -> KeyEvent {
    match key.code {
        KeyCode::Char(c) => match latin_for(c).or_else(|| latin_punctuation(c)) {
            Some(latin) => KeyEvent {
                code: KeyCode::Char(latin),
                ..key
            },
            None => key,
        },
        _ => key,
    }
}

/// Normalises a key in a context with text input: only with `Ctrl` or
/// `Alt` held does a Cyrillic letter become its Latin key (`Ctrl+ы` →
/// `Ctrl+s`); plain typed characters are left as they are.
pub fn shortcut_key(key: KeyEvent) -> KeyEvent {
    if !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return key;
    }
    match key.code {
        KeyCode::Char(c) => match latin_for(c) {
            Some(latin) => KeyEvent {
                code: KeyCode::Char(latin),
                ..key
            },
            None => key,
        },
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_letter_maps_to_its_physical_key() {
        let lower = "йцукенгшщзхъфывапролджэячсмитьбю";
        let qwerty = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
        for (c, l) in lower.chars().zip(qwerty.chars()) {
            assert_eq!(latin_for(c), Some(l), "{c}");
        }
        let upper = "ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ";
        let shifted = "QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>";
        for (c, l) in upper.chars().zip(shifted.chars()) {
            assert_eq!(latin_for(c), Some(l), "{c}");
        }
    }

    #[test]
    fn ukrainian_letters() {
        assert_eq!(latin_for('і'), Some('s'));
        assert_eq!(latin_for('І'), Some('S'));
        assert_eq!(latin_for('ї'), Some(']'));
        assert_eq!(latin_for('Ї'), Some('}'));
        assert_eq!(latin_for('є'), Some('\''));
        assert_eq!(latin_for('Є'), Some('"'));
        assert_eq!(latin_for('ґ'), Some('\\'));
        assert_eq!(latin_for('Ґ'), Some('|'));
        assert_eq!(latin_for('ё'), Some('`'));
    }

    #[test]
    fn latin_and_other_characters_are_not_mapped() {
        for c in ['a', 'n', 'Q', '/', '?', '.', ',', ' ', '1', 'ß', 'λ'] {
            assert_eq!(latin_for(c), None, "{c}");
        }
    }

    fn ch(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    #[test]
    fn command_keys_translate_letters_and_punctuation_once() {
        let none = KeyModifiers::NONE;
        assert_eq!(command_key(ch('т', none)), ch('n', none));
        assert_eq!(
            command_key(ch('К', KeyModifiers::SHIFT)),
            ch('R', KeyModifiers::SHIFT)
        );
        assert_eq!(command_key(ch('.', none)), ch('/', none));
        assert_eq!(command_key(ch(',', none)), ch('?', none));
        assert_eq!(command_key(ch('ю', none)), ch('.', none));
        assert_eq!(command_key(ch('n', none)), ch('n', none));
        let enter = KeyEvent::new(KeyCode::Enter, none);
        assert_eq!(command_key(enter), enter);
    }

    #[test]
    fn shortcut_keys_translate_only_with_ctrl_or_alt() {
        let none = KeyModifiers::NONE;
        assert_eq!(shortcut_key(ch('т', none)), ch('т', none));
        assert_eq!(shortcut_key(ch('.', none)), ch('.', none));
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(shortcut_key(ch('ы', ctrl)), ch('s', ctrl));
        assert_eq!(shortcut_key(ch('.', ctrl)), ch('.', ctrl));
        let alt = KeyModifiers::ALT;
        assert_eq!(shortcut_key(ch('ф', alt)), ch('a', alt));
        assert_eq!(shortcut_key(ch('s', ctrl)), ch('s', ctrl));
    }
}
