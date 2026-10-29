//! Icons compiled into the binary. gpui asks for them by path through [`Assets`].

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! embedded {
    ($($path:literal),* $(,)?) => {
        const FILES: &[(&str, &[u8])] = &[$(($path, include_bytes!(concat!("../assets/", $path)))),*];
    };
}

embedded!["icons/tick.svg"];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| (*p).into()).collect())
    }
}
