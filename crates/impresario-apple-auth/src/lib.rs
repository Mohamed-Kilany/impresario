//! Apple ID authentication, anisette and free-provisioning developer
//! requests.
//!
//! This crate is a vendored subset of [`isideload` 0.5.0](https://github.com/nab138/isideload)
//! (MIT). The sideloading/installation module and its LGPL
//! `apple-codesign-quick` dependency are deliberately removed. Impresario's
//! signing (`apple-codesign`) and installation (RemotePairing/RSD/AFC) live
//! outside this crate so the account flow has no dependency on them.
//!
//! Only the host-side Apple ID flow is kept: GrandSlam login and 2FA,
//! remote anisette state, developer teams/devices/App IDs/profiles and the
//! developer session. No password is written anywhere by this crate.

use rootcause::{
    hooks::{Hooks, context_formatter::ContextFormatterHook},
    prelude::*,
};

pub mod account;
pub mod anisette;
pub mod auth;
pub mod dev;
pub mod util;

/// Apple's public root certificate, used by the GrandSlam HTTP client.
pub const APPLE_ROOT: &[u8] = include_bytes!("assets/apple_root.cer");

/// Apple Worldwide Developer Relations intermediate certificate (G3), used to
/// build a complete development-signing chain in a generated PKCS#12.
pub const APPLE_WWDR_G3: &[u8] = include_bytes!("assets/AppleWWDRCAG3.cer");

#[derive(Debug, thiserror::Error)]
pub enum SideloadError {
    #[error("Auth error {0}: {1}")]
    AuthWithMessage(i64, String),

    #[error("Plist parse error: {0}")]
    PlistParseError(String),

    #[error("Developer error {0}: {1}")]
    DeveloperError(i64, String),

    #[error("Invalid bundle: {0}")]
    InvalidBundle(String),
}

// The default reqwest error formatter gives no useful chain.
struct ReqwestErrorFormatter;

impl ContextFormatterHook<reqwest::Error> for ReqwestErrorFormatter {
    fn display(
        &self,
        report: rootcause::ReportRef<'_, reqwest::Error, markers::Uncloneable, markers::Local>,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        writeln!(f, "{}", report.format_current_context_unhooked())?;
        let mut source = report.current_context_error_source();
        while let Some(s) = source {
            writeln!(f, "Caused by: {:?}", s)?;
            source = s.source();
        }
        Ok(())
    }

    fn debug(
        &self,
        report: rootcause::ReportRef<'_, reqwest::Error, markers::Uncloneable, markers::Local>,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        writeln!(f, "{}", report.format_current_context_unhooked())?;
        let mut source = report.current_context_error_source();
        while let Some(s) = source {
            writeln!(f, "Caused by: {:?}", s)?;
            source = s.source();
        }
        Ok(())
    }
}

/// Install the error hooks once. Safe to call repeatedly.
pub fn init() {
    let _ = Hooks::new()
        .context_formatter::<reqwest::Error, _>(ReqwestErrorFormatter)
        .install();
}

/// Install the rustls crypto provider used by the reqwest backend on every
/// platform (no SChannel/OpenSSL fallback).
pub fn init_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
