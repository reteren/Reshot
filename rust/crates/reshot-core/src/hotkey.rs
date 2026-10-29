//! A parsed global hotkey: modifiers plus one virtual-key code. Pure and Win32-free; the
//! platform layer feeds `modifiers` and `virtual_key` straight into `RegisterHotKey`.
//! Accepts strings like "PrtScn", "Ctrl+Shift+A", "Alt+F4", "Win+D".

use std::fmt;

/// `RegisterHotKey` modifier flags (fsModifiers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HotkeyModifiers(pub u32);

impl HotkeyModifiers {
    pub const NONE: Self = Self(0x0);
    pub const ALT: Self = Self(0x1);
    pub const CONTROL: Self = Self(0x2);
    pub const SHIFT: Self = Self(0x4);
    pub const WIN: Self = Self(0x8);
    pub const NO_REPEAT: Self = Self(0x4000);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for HotkeyModifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for HotkeyModifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyDefinition {
    pub modifiers: HotkeyModifiers,
    /// Virtual-key code (VK_*) of the main key.
    pub virtual_key: u32,
    /// Canonical name of the main key ("PrtScn", "A", "F4").
    pub key_name: String,
}

impl HotkeyDefinition {
    /// Tokens are '+'-separated; the one non-modifier is the main key. Whitespace and case
    /// are ignored. `MOD_NOREPEAT` is always added: one press, one event.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Err("Hotkey string is empty.".into());
        }
        let tokens: Vec<&str> = text.split('+').map(str::trim).filter(|t| !t.is_empty()).collect();
        if tokens.is_empty() {
            return Err("Hotkey string has no keys.".into());
        }
        let mut modifiers = HotkeyModifiers::NONE;
        let mut main: Option<&str> = None;
        for token in tokens {
            if let Some(m) = map_modifier(token) {
                modifiers |= m;
            } else if main.is_none() {
                main = Some(token);
            } else {
                return Err(format!("More than one non-modifier key in '{text}'."));
            }
        }
        let main = main.ok_or_else(|| format!("Hotkey '{text}' has modifiers but no main key."))?;
        let (virtual_key, key_name) =
            map_key(main).ok_or_else(|| format!("Unknown key '{main}' in '{text}'."))?;
        Ok(Self { modifiers: modifiers | HotkeyModifiers::NO_REPEAT, virtual_key, key_name })
    }
}

impl fmt::Display for HotkeyDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (HotkeyModifiers::CONTROL, "Ctrl+"),
            (HotkeyModifiers::ALT, "Alt+"),
            (HotkeyModifiers::SHIFT, "Shift+"),
            (HotkeyModifiers::WIN, "Win+"),
        ] {
            if self.modifiers.contains(flag) {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key_name)
    }
}

fn map_modifier(token: &str) -> Option<HotkeyModifiers> {
    match token.to_lowercase().as_str() {
        "ctrl" | "control" => Some(HotkeyModifiers::CONTROL),
        "alt" => Some(HotkeyModifiers::ALT),
        "shift" => Some(HotkeyModifiers::SHIFT),
        "win" | "windows" | "meta" | "super" => Some(HotkeyModifiers::WIN),
        _ => None,
    }
}

fn map_key(token: &str) -> Option<(u32, String)> {
    let key = token.to_lowercase();
    let b = key.as_bytes();
    if b.len() == 1 && b[0].is_ascii_lowercase() {
        return Some((b[0].to_ascii_uppercase() as u32, key.to_uppercase()));
    }
    if b.len() == 1 && b[0].is_ascii_digit() {
        return Some((b[0] as u32, key));
    }
    if (2..=3).contains(&b.len())
        && b[0] == b'f'
        && let Ok(n) = key[1..].parse::<u32>()
        && (1..=24).contains(&n)
    {
        return Some((0x70 + n - 1, format!("F{n}")));
    }
    let (vk, name) = match key.as_str() {
        "prtscn" | "prnt" | "printscreen" | "print" | "snapshot" => (0x2C, "PrtScn"),
        "insert" | "ins" => (0x2D, "Insert"),
        "delete" | "del" => (0x2E, "Delete"),
        "home" => (0x24, "Home"),
        "end" => (0x23, "End"),
        "pageup" | "pgup" => (0x21, "PageUp"),
        "pagedown" | "pgdn" => (0x22, "PageDown"),
        "space" | "spacebar" => (0x20, "Space"),
        "tab" => (0x09, "Tab"),
        "enter" | "return" => (0x0D, "Enter"),
        "esc" | "escape" => (0x1B, "Esc"),
        "backspace" => (0x08, "Backspace"),
        "pause" => (0x13, "Pause"),
        "scrolllock" => (0x91, "ScrollLock"),
        "up" => (0x26, "Up"),
        "down" => (0x28, "Down"),
        "left" => (0x25, "Left"),
        "right" => (0x27, "Right"),
        _ => return None,
    };
    Some((vk, name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prtscn_with_no_modifiers() {
        let d = HotkeyDefinition::parse("PrtScn").unwrap();
        assert_eq!(d.virtual_key, 0x2C);
        assert_eq!(d.key_name, "PrtScn");
        assert!(!d.modifiers.contains(HotkeyModifiers::CONTROL));
        assert!(!d.modifiers.contains(HotkeyModifiers::ALT));
        assert!(d.modifiers.contains(HotkeyModifiers::NO_REPEAT));
    }

    #[test]
    fn prtscn_aliases_and_casing_normalize() {
        for input in ["printscreen", "PRINT", "Prnt", "  snapshot "] {
            let d = HotkeyDefinition::parse(input).unwrap();
            assert_eq!(d.virtual_key, 0x2C);
            assert_eq!(d.key_name, "PrtScn");
        }
    }

    #[test]
    fn parses_modifier_combo() {
        let d = HotkeyDefinition::parse("Ctrl+Shift+A").unwrap();
        assert!(d.modifiers.contains(HotkeyModifiers::CONTROL));
        assert!(d.modifiers.contains(HotkeyModifiers::SHIFT));
        assert!(!d.modifiers.contains(HotkeyModifiers::ALT));
        assert_eq!(d.virtual_key, 'A' as u32);
        assert_eq!(d.key_name, "A");
    }

    #[test]
    fn parses_function_keys() {
        let d = HotkeyDefinition::parse("Alt+F4").unwrap();
        assert!(d.modifiers.contains(HotkeyModifiers::ALT));
        assert_eq!(d.virtual_key, 0x73);
        assert_eq!(d.key_name, "F4");
    }

    #[test]
    fn parses_digits() {
        let d = HotkeyDefinition::parse("Win+1").unwrap();
        assert!(d.modifiers.contains(HotkeyModifiers::WIN));
        assert_eq!(d.virtual_key, '1' as u32);
    }

    #[test]
    fn display_roundtrips_to_canonical_form() {
        assert_eq!(HotkeyDefinition::parse("shift+ctrl+prtscn").unwrap().to_string(), "Ctrl+Shift+PrtScn");
    }

    #[test]
    fn rejects_invalid_input() {
        for input in ["", "   ", "Ctrl", "Ctrl+A+B", "Ctrl+Nope"] {
            assert!(!HotkeyDefinition::parse(input).unwrap_err().is_empty(), "{input:?}");
        }
    }
}
