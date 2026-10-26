use crate::auth::apple_account::{TwoFactorCallbackParams, TwoFactorCallbackResponse};
use rootcause::prelude::*;

// Don't care if anything is Send in the very single-threaded wasm build.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

pub trait TwoFactorCallback: Fn(TwoFactorCallbackParams) -> Self::Future + Send + Sync {
    type Future: Future<Output = Result<TwoFactorCallbackResponse, Report>> + MaybeSend;
}

impl<C, Fut> TwoFactorCallback for C
where
    C: Fn(TwoFactorCallbackParams) -> Fut + Send + Sync,
    Fut: Future<Output = Result<TwoFactorCallbackResponse, Report>> + MaybeSend,
{
    type Future = Fut;
}
