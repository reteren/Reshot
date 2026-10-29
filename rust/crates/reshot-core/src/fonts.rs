//! The installed system fonts, for the Text tool (SPEC §6.4). The single place that turns a
//! family name into a Skia typeface: the text object, the caret drawn while typing and the
//! picker all go through it, so a name is resolved and cached once and an unknown one falls
//! back to Segoe UI in exactly one place.
//!
//! Building the catalogue opens every installed font file to probe glyph coverage, which is why
//! it is lazy and why [`warmup`] exists: the overlay must never pay for it on the UI thread.

use parking_lot::Mutex;
use skia_safe::{FontMgr, FontStyle, Typeface};
use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

/// Family used when none is chosen, or the chosen one is not installed.
pub const DEFAULT_FAMILY: &str = "Segoe UI";

// The two ends of the Cyrillic block. A family carrying both effectively always carries the
// whole alphabet, and two probes keep the catalogue build cheap.
const CYRILLIC_CAPITAL_A: char = '\u{0410}';
const CYRILLIC_SMALL_YA: char = '\u{044F}';

/// One installed family as offered by the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontEntry {
    pub family: String,
    /// False when the family has no Cyrillic glyphs. Skia does not substitute another family
    /// for missing glyphs, so such a font renders Russian as tofu boxes; the picker dims these
    /// rows because its own preview (rendered by the UI toolkit, which does substitute) cannot
    /// be trusted to show the failure.
    pub supports_cyrillic: bool,
}

// Keyed by the lower-cased name: family lookups are case-insensitive, like the C# cache.
static TYPEFACES: LazyLock<Mutex<HashMap<String, Typeface>>> = LazyLock::new(Default::default);
static CATALOG: OnceLock<Vec<FontEntry>> = OnceLock::new();

/// Every installed family, the default first and the rest sorted case-insensitively.
/// Built once, on first access.
pub fn families() -> &'static [FontEntry] {
    CATALOG.get_or_init(build)
}

/// Builds the catalogue on a background thread so the first UI touch is instant.
pub fn warmup() {
    if CATALOG.get().is_none() {
        std::thread::Builder::new()
            .name("font-catalog".into())
            .spawn(|| {
                families();
            })
            .ok();
    }
}

/// The typeface for a family, cached one per family. A blank or not-installed name falls back
/// to the default family, so a stale font name in saved settings measures and draws exactly
/// like the default. Deliberately independent of [`families`]: this runs on every redraw, and
/// touching the catalogue here would block the first draw on probing every font file.
pub fn resolve(family: &str) -> Typeface {
    let name = family.trim();
    if name.is_empty() {
        return cached(DEFAULT_FAMILY);
    }
    let key = name.to_lowercase();
    if let Some(tf) = TYPEFACES.lock().get(&key) {
        return tf.clone();
    }
    // Skia answers an unknown family with a silently substituted face; comparing the family
    // that came back stops that face being cached as a second handle under the unknown name.
    let tf = match open(name) {
        Some(tf) if tf.family_name().eq_ignore_ascii_case(name) => tf,
        _ => cached(DEFAULT_FAMILY),
    };
    TYPEFACES.lock().entry(key).or_insert(tf).clone()
}

fn cached(family: &str) -> Typeface {
    let key = family.to_lowercase();
    if let Some(tf) = TYPEFACES.lock().get(&key) {
        return tf.clone();
    }
    let tf = open(family)
        .or_else(|| open(DEFAULT_FAMILY))
        .or_else(|| FontMgr::new().legacy_make_typeface(None, FontStyle::normal()))
        .expect("the system font manager has no fonts at all");
    TYPEFACES.lock().entry(key).or_insert(tf).clone()
}

fn open(family: &str) -> Option<Typeface> {
    FontMgr::new().match_family_style(family, FontStyle::normal())
}

fn build() -> Vec<FontEntry> {
    let mgr = FontMgr::new();
    let mut seen = std::collections::HashSet::new();
    let mut names: Vec<String> = mgr
        .family_names()
        .filter(|n| !n.trim().is_empty() && seen.insert(n.to_lowercase()))
        .collect();
    // Ordinal case-insensitive, not culture-aware: the same machine must produce the same order
    // for every user.
    names.sort_by_key(|n| n.to_lowercase());
    // The default leads the dropdown so it opens on the familiar choice.
    if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(DEFAULT_FAMILY))
        && i > 0
    {
        let preferred = names.remove(i);
        names.insert(0, preferred);
    }
    if names.is_empty() {
        names.push(DEFAULT_FAMILY.to_string());
    }
    names
        .into_iter()
        .map(|family| {
            let supports_cyrillic = has_cyrillic(&family);
            FontEntry { family, supports_cyrillic }
        })
        .collect()
}

fn has_cyrillic(family: &str) -> bool {
    // Goes through `cached`, not `resolve`, and shares its cache, so each font file is opened
    // once rather than once per probe.
    let tf = cached(family);
    tf.unichar_to_glyph(CYRILLIC_CAPITAL_A as i32) != 0 && tf.unichar_to_glyph(CYRILLIC_SMALL_YA as i32) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_family_leads_the_catalogue() {
        let f = families();
        assert!(!f.is_empty());
        assert!(f[0].family.eq_ignore_ascii_case(DEFAULT_FAMILY));
        assert!(f[0].supports_cyrillic, "Segoe UI carries Cyrillic");
    }

    #[test]
    fn catalogue_has_no_duplicates_and_is_sorted_after_the_default() {
        let f = families();
        let rest: Vec<String> = f[1..].iter().map(|e| e.family.to_lowercase()).collect();
        let mut sorted = rest.clone();
        sorted.sort();
        assert_eq!(rest, sorted);
        let mut uniq = rest.clone();
        uniq.dedup();
        assert_eq!(uniq.len(), rest.len());
    }

    #[test]
    fn unknown_and_blank_resolve_to_the_default() {
        let d = resolve(DEFAULT_FAMILY);
        assert_eq!(resolve("NoSuchFontFamily_ZZZ").family_name(), d.family_name());
        assert_eq!(resolve("   ").family_name(), d.family_name());
    }

    #[test]
    fn resolve_is_case_insensitive_and_cached() {
        let a = resolve("segoe ui");
        let b = resolve("SEGOE UI");
        assert_eq!(a.unique_id(), b.unique_id());
    }
}
