use std::{fmt, net::IpAddr};

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use wireguard_keys::Pubkey;

#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct WgPeer {
    pub public_key: Pubkey,
    pub endpoint: String,

    /// The WireGuard internal IP of the peer.
    ///
    /// It should be provided with the most specific netmask as it's meant to for only that peer.
    /// So for IPv4, use /32 and for IPv6, use /128.
    pub address: IpNet,
}

impl WgPeer {
    pub fn new(public_key: Pubkey, endpoint: &str, address: IpAddr) -> Self {
        Self {
            public_key,
            endpoint: endpoint.to_string(),
            address: address.into(),
        }
    }
}

impl fmt::Debug for WgPeer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WgPeer")
            .field("public_key", &self.public_key.to_base64_urlsafe())
            .field("endpoint", &self.endpoint)
            .field("address", &self.address)
            .finish()
    }
}
