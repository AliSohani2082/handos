//! Generated xkb keymaps for the wlr virtual keyboard: one key per distinct character, so any
//! Unicode text (Persian included) can be typed without depending on the user's layout.
//!
//! Characters go only on the evdev codes of ordinary character keys. Some applications give
//! certain codes a fixed meaning whatever keysym the keymap puts there: Chromium treats code 14
//! as Backspace and 29 as Control, and a `v` placed on code 1 (Escape) never pastes. Named keys
//! (Return, Control, Shift) sit on their real codes.

/// Real-modifier masks in an xkb keymap built from the "complete" compatibility map.
pub const SHIFT: u32 = 1;
pub const CONTROL: u32 = 4;

const KEY_ENTER: u32 = 28;
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_V: u32 = 47;
const KEY_SPACE: u32 = 57;

/// evdev codes of digit, letter and punctuation keys, in keyboard order.
const ORDINARY: [u32; 47] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, // 1 .. =
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, // q .. ]
    30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, // a .. `
    43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, // \ .. /
];

pub struct Keymap {
    /// The keymap text to upload (without the trailing NUL).
    pub text: String,
    /// (keysym name, evdev code) for every key.
    pub keys: Vec<(String, u32)>,
}

impl Keymap {
    fn from_keys(keys: Vec<(String, u32)>) -> Keymap {
        Keymap {
            text: render(&keys),
            keys,
        }
    }

    /// Keymaps for typing `text`: one per chunk, since each holds at most 48 distinct characters
    /// (47 ordinary keys plus space). Every keymap also has Return.
    pub fn for_text(text: &str) -> Vec<(Keymap, String)> {
        let mut out = Vec::new();
        let mut chunk = String::new();
        let mut keys: Vec<(String, u32)> = Vec::new();
        let mut used = 0;
        for c in text.chars() {
            let name = keysym_name(c);
            if !keys.iter().any(|(k, _)| *k == name) {
                let code = match name.as_str() {
                    "space" => KEY_SPACE,
                    "Return" => KEY_ENTER,
                    _ if used < ORDINARY.len() => {
                        used += 1;
                        ORDINARY[used - 1]
                    }
                    _ => {
                        // Pool exhausted: close this chunk and start a new keymap.
                        out.push((
                            Self::finish(std::mem::take(&mut keys)),
                            std::mem::take(&mut chunk),
                        ));
                        used = 1;
                        ORDINARY[0]
                    }
                };
                keys.push((name, code));
            }
            chunk.push(c);
        }
        out.push((Self::finish(keys), chunk));
        out
    }

    fn finish(mut keys: Vec<(String, u32)>) -> Keymap {
        if !keys.iter().any(|(k, _)| k == "Return") {
            keys.push(("Return".into(), KEY_ENTER));
        }
        Self::from_keys(keys)
    }

    /// A keymap for paste shortcuts: v, Return, Control and Shift, each on its real code.
    pub fn paste() -> Keymap {
        Self::from_keys(vec![
            ("v".into(), KEY_V),
            ("Return".into(), KEY_ENTER),
            ("Control_L".into(), KEY_LEFTCTRL),
            ("Shift_L".into(), KEY_LEFTSHIFT),
        ])
    }

    /// The evdev key code that produces `keysym` in this keymap.
    pub fn code(&self, keysym: &str) -> Option<u32> {
        self.keys.iter().find(|(k, _)| k == keysym).map(|&(_, c)| c)
    }

    /// Key codes to press, in order, to type `text`.
    pub fn codes_for(&self, text: &str) -> Vec<u32> {
        text.chars()
            .map(|c| self.code(&keysym_name(c)).expect("character not in keymap"))
            .collect()
    }
}

/// The xkb keysym name for a character. Letters and digits use their own names; everything
/// else uses the Unicode form `Uxxxx`, which xkbcommon accepts for any code point.
pub fn keysym_name(c: char) -> String {
    match c {
        ' ' => "space".into(),
        '\n' => "Return".into(),
        '\t' => "Tab".into(),
        c if c.is_ascii_alphanumeric() => c.to_string(),
        c => format!("U{:04X}", c as u32),
    }
}

/// xkb keycodes are evdev codes plus 8.
fn render(keys: &[(String, u32)]) -> String {
    let max = keys.iter().map(|&(_, c)| c).max().unwrap_or(1) + 8;
    let mut s = String::from("xkb_keymap {\nxkb_keycodes \"(unnamed)\" {\nminimum = 8;\n");
    s.push_str(&format!("maximum = {max};\n"));
    for (_, code) in keys {
        s.push_str(&format!("<K{code}> = {};\n", code + 8));
    }
    s.push_str("};\nxkb_types \"(unnamed)\" { include \"complete\" };\n");
    s.push_str("xkb_compatibility \"(unnamed)\" { include \"complete\" };\n");
    s.push_str("xkb_symbols \"(unnamed)\" {\n");
    for (name, code) in keys {
        s.push_str(&format!("key <K{code}> {{[ {name} ]}};\n"));
    }
    s.push_str("};\n};\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cover_latin_persian_and_joiners() {
        assert_eq!(keysym_name('a'), "a");
        assert_eq!(keysym_name('Z'), "Z");
        assert_eq!(keysym_name('7'), "7");
        assert_eq!(keysym_name(' '), "space");
        assert_eq!(keysym_name(','), "U002C");
        assert_eq!(keysym_name('س'), "U0633");
        assert_eq!(keysym_name('\u{200C}'), "U200C"); // zero-width non-joiner, used in Persian
        assert_eq!(keysym_name('،'), "U060C");
    }

    #[test]
    fn characters_never_land_on_keys_with_fixed_meanings() {
        let text = "The quick brown fox jumps over the lazy dog, 42 times! سلام دنیا، من می\u{200c}خواهم این متن را بنویسم.";
        for (km, _) in Keymap::for_text(text) {
            for (name, code) in &km.keys {
                let fixed = [1, 14, 15, 28, 29, 42, 54, 56, 58];
                assert!(
                    !fixed.contains(code) || (name == "Return" && *code == KEY_ENTER),
                    "{name} on code {code}"
                );
            }
        }
    }

    #[test]
    fn one_key_per_distinct_character_and_codes_round_trip() {
        let [(km, chunk)] = Keymap::for_text("سلام سلام").try_into().ok().unwrap();
        assert_eq!(chunk, "سلام سلام");
        assert_eq!(
            km.codes_for("سلام سلام"),
            vec![2, 3, 4, 5, KEY_SPACE, 2, 3, 4, 5]
        );
        assert_eq!(km.code("Return"), Some(KEY_ENTER));
    }

    #[test]
    fn long_alphabets_split_into_chunks_that_reassemble() {
        let text: String = ('\u{0621}'..='\u{064A}').chain('a'..='z').collect(); // 42 + 26 distinct
        let chunks = Keymap::for_text(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            chunks.iter().map(|(_, c)| c.as_str()).collect::<String>(),
            text
        );
        for (km, chunk) in &chunks {
            assert_eq!(km.codes_for(chunk).len(), chunk.chars().count());
        }
    }

    #[test]
    fn rendered_keymap_maps_keycode_names_to_symbols() {
        let [(km, _)] = Keymap::for_text("ab").try_into().ok().unwrap();
        assert!(km.text.contains("<K2> = 10;"));
        assert!(km.text.contains("key <K3> {[ b ]};"));
        assert!(km.text.contains("key <K28> {[ Return ]};"));
        assert!(km.text.contains("maximum = 36;"));
        assert_eq!(km.text.matches('{').count(), km.text.matches('}').count());
    }

    #[test]
    fn paste_keymap_uses_real_codes() {
        let km = Keymap::paste();
        assert_eq!(km.code("v"), Some(KEY_V));
        assert_eq!(km.code("Control_L"), Some(KEY_LEFTCTRL));
        assert_eq!(km.code("Return"), Some(KEY_ENTER));
    }
}
