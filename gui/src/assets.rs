//! Embedded SVG icon assets, served to gpui's `svg()` element via `AssetSource`.
//! Icons are Lucide (MIT), monochrome — gpui rasterizes each to an alpha mask and
//! tints it with the element's `text_color`. Wired in `main.rs` via `with_assets`.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[ $( ($name, include_bytes!(concat!("../assets/icons/", $name, ".svg")).as_slice()) ),* ]
    };
}

/// (name, bytes) for every bundled icon. `svg().path("icons/<name>.svg")` resolves here.
const ICONS: &[(&str, &[u8])] = icons![
    "folder", "clipboard", "stop", "trash", "settings", "chevron-left", "mic",
    "play", "pause", "skip-back", "skip-forward", "rewind", "fast-forward",
    "image", "volume", "volume-x", "refresh", "scissors", "list-tree",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // Paths look like "icons/folder.svg"; match on the bare stem.
        let stem = path
            .strip_prefix("icons/")
            .unwrap_or(path)
            .strip_suffix(".svg")
            .unwrap_or(path);
        Ok(ICONS
            .iter()
            .find(|(n, _)| *n == stem)
            .map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(n, _)| SharedString::from(format!("icons/{n}.svg")))
            .collect())
    }
}
