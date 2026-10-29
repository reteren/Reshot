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
