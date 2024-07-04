use std::time::Duration;

pub mod consul;
pub mod networkd;
pub mod wireguard;

pub const CONSUL_TTL: Duration = Duration::from_secs(15);
