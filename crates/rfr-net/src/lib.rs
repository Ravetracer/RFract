// SPDX-License-Identifier: LGPL-3.0-or-later

//! Network rendering: render clients announce themselves on the local network, a server (the
//! CLI or the app) sends them a job with the files it needs, hands out animation frames or
//! tiles of a still image and puts the results together.
//!
//! Ports: UDP [`DISCOVERY_PORT`] for the announcements, TCP [`CLIENT_PORT`] on the clients.

pub mod assets;
pub mod client;
pub mod discovery;
pub mod proto;
pub mod server;

/// UDP port the clients announce themselves on.
pub const DISCOVERY_PORT: u16 = 47820;
/// TCP port a client accepts the server on.
pub const CLIENT_PORT: u16 = 47821;
/// Still images go out in tiles of this many pixels per side (a 1080p image in 12 pieces).
pub const TILE: u32 = 512;
/// Changes whenever the messages change.
pub const PROTOCOL: u32 = 1;

/// This machine's name (shown in the client lists).
pub fn host_name() -> String {
    sysinfo::System::host_name().ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "this machine".into())
}

/// Version string clients and servers must share (same program version, same protocol).
pub fn version() -> String {
    format!("{} (protocol {PROTOCOL})", env!("CARGO_PKG_VERSION"))
}
