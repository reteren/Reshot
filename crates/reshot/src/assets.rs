//! Icons compiled into the binary. Everything under `assets/` is embedded, so adding an icon
//! is just dropping the file in; gpui asks for it by path (`icons/brush.svg`).

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/*"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(<Assets as RustEmbed>::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(<Assets as RustEmbed>::iter().filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}

/// Roboto 3.015, the exact face the settings dialog was designed in: the webview bundled it
/// through @fontsource, whose per-script subsets are merged back into one file per weight.
/// It is named "Reshot Roboto" so an older Roboto installed on the machine (2.x, with other
/// kerning) never stands in for it.
const ROBOTO: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/ReshotRoboto-Thin.ttf"),
    include_bytes!("../assets/fonts/ReshotRoboto-Light.ttf"),
    include_bytes!("../assets/fonts/ReshotRoboto-Regular.ttf"),
    include_bytes!("../assets/fonts/ReshotRoboto-Bold.ttf"),
];

pub fn register_fonts(cx: &gpui::App) {
    let fonts = ROBOTO.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        log::error!("Fonts: could not register Roboto: {e:#}");
    }
}
