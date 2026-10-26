# Vendored `idevice` 0.1.68

Source: <https://crates.io/crates/idevice/0.1.68>, MIT (see `LICENSE`).

## Why it is vendored

I-150 requires a physical tvOS XCUITest session over the direct outer-RSD
connection (`com.apple.dt.testmanagerd.remote` +
`com.apple.instruments.dtservicehub`) while the RemotePairing control
connection stays alive. The published `idevice` 0.1.68 only exposes
`XCUITestService::run`, which always builds a `CoreDeviceProxy` tunnel.

## Patch

`src/services/dvt/xctest/mod.rs` adds 238 lines:

- `XCUITestService::run_with_rsd(cfg, listener, timeout, outer_handle,
  outer_handshake, ios_major_version)`.
- The private `run` body is shared so both entry points perform the same DTX
  handshake, test-plan execution and teardown.
- The direct path opens `com.apple.dt.testmanagerd.remote` and
  `com.apple.instruments.dtservicehub` on an existing `AdapterHandle` and does
  not create a `CoreDeviceProxy`. It is the default; set
  `IMPRESARIO_XCTEST_DIRECT=0` only for an A/B diagnostic of the older
  from-outer stack.

No other file differs from the published crate. Keep the patch limited to this
module and re-check it against each upstream release.
