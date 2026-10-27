//! Pure-Rust Apple device services used by Impresario.
//!
//! The account/auth flow lives in `impresario-apple-auth`; this crate owns the
//! device side: RemotePairing pair/verify, the RSD tunnel, AFC/installation
//! proxy, personalized Developer Disk Image mounting, XCUITest startup and
//! `apple-codesign` signing. No host tool (`xcrun`, `devicectl`,
//! `ideviceinstaller`, `ssh`) is invoked.

pub mod ddi;
pub mod error;
pub mod install;
pub mod pairing;
pub mod products;
pub mod provision;
pub mod runner;
pub mod session;
pub mod sign;
pub mod signing_set;
pub mod tunnel;

pub use ddi::DdiAssets;
pub use error::{Error, Result};
pub use pairing::{ApplePaths, PairingSummary, RpPairingFile};
pub use products::ProductSet;
pub use provision::{
    install, provision, CertificateSummary, ExistingIdentity, IdentityMaterial, ProvisionError,
    ProvisionPlatform, ProvisionReport, ProvisionRequest, ProvisionedBundle,
};
pub use runner::{RunnerOptions, RunnerSession};
pub use session::{prepare, sign_products, PreparedDevice, SessionOptions, SignedProducts};
pub use signing_set::{ProfileInfo, SigningSet};
pub use tunnel::AppleTunnel;
