//! Interface language. Every user-facing string is written in English in the source and goes
//! through [`tr`] (or [`trf`] when it has `{named}` placeholders); the English text itself is the
//! key into `locales/<code>.json`. A string missing from a locale falls back to English, so a
//! half-finished translation degrades instead of breaking, and a test keeps every locale
//! complete against the keys the source actually uses.
//!
//! The language is process-wide: chosen once at startup and again whenever the settings are
//! saved. Windows that are open re-read their strings on the next render.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One interface language.
pub struct Language {
    /// BCP 47 code, as stored in `settings.json`.
    pub code: &'static str,
    /// The language's name in itself, for the picker.
    pub native: &'static str,
    /// Windows OCR recognizer tags for text in this language, best first. Empty when Windows
    /// ships no OCR pack for it; a close relative stands in where one exists.
    pub ocr: &'static [&'static str],
    source: &'static str,
}

macro_rules! languages {
    ($($code:literal, $native:literal, [$($ocr:literal),*]);* $(;)?) => {
        /// Every interface language, English first and the rest in the picker's order.
        pub const LANGUAGES: &[Language] = &[
            $(Language {
                code: $code,
                native: $native,
                ocr: &[$($ocr),*],
                source: include_str!(concat!("../locales/", $code, ".json")),
            }),*
        ];
    };
}

languages! {
    "en", "English", ["en"];
    "ru", "Русский", ["ru"];
    "uk", "Українська", ["ru"];
    "de", "Deutsch", ["de"];
    "fr", "Français", ["fr"];
    "es", "Español", ["es"];
    "it", "Italiano", ["it"];
    "pt", "Português", ["pt"];
    "nl", "Nederlands", ["nl"];
    "pl", "Polski", ["pl"];
    "cs", "Čeština", ["cs"];
    "sk", "Slovenčina", ["sk"];
    "hu", "Magyar", ["hu"];
    "ro", "Română", ["ro"];
    "bg", "Български", ["bg"];
    "sr", "Српски", ["sr-Cyrl", "sr-Latn"];
    "hr", "Hrvatski", ["hr", "bs", "sr-Latn"];
    "sl", "Slovenščina", ["sl"];
    "el", "Ελληνικά", ["el"];
    "sv", "Svenska", ["sv"];
    "nb", "Norsk bokmål", ["nb", "no"];
    "da", "Dansk", ["da"];
    "fi", "Suomi", ["fi"];
    "et", "Eesti", [];
    "lv", "Latviešu", [];
    "lt", "Lietuvių", [];
    "tr", "Türkçe", ["tr"];
    "zh-Hans", "简体中文", ["zh-Hans", "zh-CN", "zh-SG"];
    "zh-Hant", "繁體中文", ["zh-Hant", "zh-TW", "zh-HK", "zh-MO"];
    "ja", "日本語", ["ja"];
    "ko", "한국어", ["ko"];
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SYSTEM: OnceLock<Vec<String>> = OnceLock::new();
static TABLES: [OnceLock<HashMap<String, String>>; LANGUAGES.len()] =
    [const { OnceLock::new() }; LANGUAGES.len()];

/// Records the user's Windows display languages, most preferred first (`GetUserPreferredUILanguages`).
/// Called once at startup, before the first [`apply`].
pub fn set_system_languages(tags: Vec<String>) {
    let _ = SYSTEM.set(tags);
}

/// The Windows display languages recorded at startup.
pub fn system_languages() -> &'static [String] {
    SYSTEM.get().map_or(&[], Vec::as_slice)
}

/// Switches the interface to the language a `language` setting names. Empty, or a code this
/// build does not know, follows the system.
pub fn apply(setting: &str) {
    CURRENT.store(resolve(setting, system_languages()), Ordering::Relaxed);
}

/// The language a setting resolves to, as an index into [`LANGUAGES`].
pub fn resolve(setting: &str, system: &[String]) -> usize {
    if let Some(i) = LANGUAGES.iter().position(|l| l.code.eq_ignore_ascii_case(setting)) {
        return i;
    }
    system.iter().find_map(|tag| match_tag(tag)).unwrap_or(0)
}

/// The interface language that serves a Windows language tag such as `pt-BR` or `zh-TW`.
pub fn match_tag(tag: &str) -> Option<usize> {
    let lower = tag.to_ascii_lowercase();
    let mut parts = lower.split(['-', '_']);
    let primary = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    let code = match primary {
        "zh" => {
            let traditional = rest.iter().any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"));
            if traditional { "zh-Hant" } else { "zh-Hans" }
        }
        "no" | "nn" => "nb",
        other => other,
    };
    LANGUAGES.iter().position(|l| l.code.eq_ignore_ascii_case(code))
}

/// The language the interface is in.
pub fn current() -> &'static Language {
    &LANGUAGES[CURRENT.load(Ordering::Relaxed)]
}

fn table(i: usize) -> &'static HashMap<String, String> {
    TABLES[i].get_or_init(|| match serde_json::from_str(LANGUAGES[i].source) {
        Ok(map) => map,
        Err(error) => {
            log::warn!("Locale {} is unreadable: {error}", LANGUAGES[i].code);
            HashMap::new()
        }
    })
}

/// `english` in the interface language.
pub fn tr(english: &'static str) -> &'static str {
    let i = CURRENT.load(Ordering::Relaxed);
    if i == 0 {
        return english;
    }
    table(i).get(english).map_or(english, String::as_str)
}

/// [`tr`], then every `{name}` replaced by its value.
pub fn trf(english: &'static str, args: &[(&str, &str)]) -> String {
    let mut out = tr(english).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Windows OCR tags in the order recognition should try them: the interface language, then
/// English, then the user's other display languages. Recognition appends whatever else is
/// installed after these.
pub fn ocr_priority() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |tag: &str| {
        if !out.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            out.push(tag.to_string());
        }
    };
    current().ocr.iter().for_each(|t| push(t));
    push("en");
    for tag in system_languages() {
        if let Some(i) = match_tag(tag) {
            LANGUAGES[i].ocr.iter().for_each(|t| push(t));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn system_tags_map_to_interface_languages() {
        let code = |tag: &str| match_tag(tag).map(|i| LANGUAGES[i].code);
        assert_eq!(code("ru-RU"), Some("ru"));
        assert_eq!(code("pt-BR"), Some("pt"));
        assert_eq!(code("zh-CN"), Some("zh-Hans"));
        assert_eq!(code("zh-TW"), Some("zh-Hant"));
        assert_eq!(code("zh-Hant-HK"), Some("zh-Hant"));
        assert_eq!(code("nn-NO"), Some("nb"));
        assert_eq!(code("sr-Latn-RS"), Some("sr"));
        assert_eq!(code("ar-SA"), None);
    }

    #[test]
    fn an_empty_or_unknown_setting_follows_the_system_then_english() {
        let system = vec!["ar-SA".to_string(), "de-AT".to_string()];
        assert_eq!(LANGUAGES[resolve("", &system)].code, "de");
        assert_eq!(LANGUAGES[resolve("xx", &system)].code, "de");
        assert_eq!(LANGUAGES[resolve("ja", &system)].code, "ja");
        assert_eq!(LANGUAGES[resolve("", &[])].code, "en");
    }

    #[test]
    fn there_are_about_thirty_languages_with_unique_codes() {
        let codes: BTreeSet<_> = LANGUAGES.iter().map(|l| l.code).collect();
        assert_eq!(codes.len(), LANGUAGES.len());
        assert!(LANGUAGES.len() >= 30);
        assert_eq!(LANGUAGES[0].code, "en");
    }

    /// Every string literal passed to `tr` or `trf` in the workspace sources.
    fn source_keys() -> BTreeSet<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut keys = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    collect_keys(&source, &mut keys);
                }
            }
        }
        keys
    }

    fn collect_keys(source: &str, keys: &mut BTreeSet<String>) {
        // Built at run time so this scanner's own source holds no call pattern to find.
        for call in ["tr", "trf"].map(|name| format!("{name}(")) {
            let call = call.as_str();
            let mut rest = source;
            while let Some(at) = rest.find(call) {
                let before = rest[..at].chars().next_back();
                rest = &rest[at + call.len()..];
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // The literal may start on the next line.
                let Some(literal) = rest.trim_start().strip_prefix('"') else {
                    continue;
                };
                rest = literal;
                let mut key = String::new();
                let mut chars = rest.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => key.push('\n'),
                            Some(other) => key.push(other),
                            None => break,
                        },
                        _ => key.push(c),
                    }
                }
                if !key.is_empty() {
                    keys.insert(key);
                }
            }
        }
    }

    fn placeholders(s: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(open) = rest.find('{') {
            match rest[open..].find('}') {
                Some(close) => {
                    out.insert(rest[open..open + close + 1].to_string());
                    rest = &rest[open + close + 1..];
                }
                None => break,
            }
        }
        out
    }

    #[test]
    fn every_locale_translates_exactly_the_keys_the_source_uses() {
        let keys = source_keys();
        assert!(keys.len() > 100, "found only {} keys", keys.len());
        let mut problems = Vec::new();
        for (i, language) in LANGUAGES.iter().enumerate().skip(1) {
            let map: HashMap<String, String> = serde_json::from_str(language.source)
                .unwrap_or_else(|e| panic!("{}.json: {e}", language.code));
            for key in &keys {
                match map.get(key) {
                    None => problems.push(format!("{}: missing {key:?}", language.code)),
                    Some(value) if value.trim().is_empty() => {
                        problems.push(format!("{}: empty {key:?}", language.code))
                    }
                    Some(value) if placeholders(value) != placeholders(key) => {
                        problems.push(format!("{}: placeholders differ in {key:?}", language.code))
                    }
                    Some(value) if key.starts_with('&') && !value.contains('&') => {
                        problems.push(format!("{}: menu mnemonic lost in {key:?}", language.code))
                    }
                    Some(_) => {}
                }
            }
            for key in map.keys() {
                if !keys.contains(key) {
                    problems.push(format!("{}: unused {key:?}", language.code));
                }
            }
            assert!(!table(i).is_empty());
        }
        assert!(problems.is_empty(), "{} locale problems:\n{}", problems.len(), problems.join("\n"));
    }

    #[test]
    fn named_placeholders_are_filled() {
        assert_eq!(trf("Recording started. Press {hotkey} to stop.", &[("hotkey", "F9")]), "Recording started. Press F9 to stop.");
    }
}
