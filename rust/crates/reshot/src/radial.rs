//! Hold-to-open radial quick menu (SPEC §13). Ported in phase R8; until then a hold behaves
//! like a tap so the hotkey is never dead.

use gpui::App;

pub fn open(cx: &mut App, _vk: u32) {
    log::warn!("Radial menu is not ported yet (R8); treating the hold as a tap.");
    crate::app::capture(cx, "hotkey hold");
}
