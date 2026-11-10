//! Embedded console assets. Everything is compiled into the binary; the
//! console never loads a CDN, font or script from the network.

pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_CSS: &str = include_str!("assets/app.css");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const FAVICON_SVG: &str = include_str!("assets/favicon.svg");
