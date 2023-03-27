use std::{
    fmt,
    net::IpAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Result};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
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

fn parse_latest_handshakes(s: &str) -> Result<Vec<(Pubkey, SystemTime)>> {
    let mut peers = vec![];
    for line in s.lines() {
        let split_line = line.split_ascii_whitespace().collect::<Vec<_>>();
        peers.push((
            split_line[0].parse()?,
            UNIX_EPOCH + Duration::from_secs(split_line[1].parse()?),
        ));
    }
    Ok(peers)
}

/// Get a list of latest handshakes by running `wg show <interface> latest-handshakes`
pub async fn latest_handshakes(interface: &str) -> Result<Vec<(Pubkey, SystemTime)>> {
    let output = Command::new("wg")
        .arg("show")
        .arg(interface)
        .arg("latest-handshakes")
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "Couldn't get output of wg show: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output_reader = std::str::from_utf8(&output.stdout)?;

    parse_latest_handshakes(output_reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn latest_handshakes_test() {
        let handshakes_output = "\
DIyAfJzu5HRqpx/Dd8q1Q1Llqns+D7sL3JBEaYkzhCY=    1679909056
Enpz7ohwrsFIYfEt1S1zInXv4c6DfEh9khHP3ZeCKAU=    1679909054
7ROFbBm9HEtnnuctjp1uuglg/x+dwo75nnVgCPfUFyo=    0
VD59H0sSPpG/nAr1/CgQPGBGhNQ2YLwOurWvxcogSFU=    1679909060
7M679vrRuJuZcQ8hSAtbswhEcQZINCLkPyUx3S1LTC8=    0";

        let result = parse_latest_handshakes(handshakes_output).unwrap();
        let expected = vec![
            (
                Pubkey::from_base64("DIyAfJzu5HRqpx/Dd8q1Q1Llqns+D7sL3JBEaYkzhCY=").unwrap(),
                UNIX_EPOCH + Duration::from_secs(1679909056),
            ),
            (
                Pubkey::from_base64("Enpz7ohwrsFIYfEt1S1zInXv4c6DfEh9khHP3ZeCKAU=").unwrap(),
                UNIX_EPOCH + Duration::from_secs(1679909054),
            ),
            (
                Pubkey::from_base64("7ROFbBm9HEtnnuctjp1uuglg/x+dwo75nnVgCPfUFyo=").unwrap(),
                UNIX_EPOCH + Duration::from_secs(0),
            ),
            (
                Pubkey::from_base64("VD59H0sSPpG/nAr1/CgQPGBGhNQ2YLwOurWvxcogSFU=").unwrap(),
                UNIX_EPOCH + Duration::from_secs(1679909060),
            ),
            (
                Pubkey::from_base64("7M679vrRuJuZcQ8hSAtbswhEcQZINCLkPyUx3S1LTC8=").unwrap(),
                UNIX_EPOCH + Duration::from_secs(0),
            ),
        ];
        assert_eq!(result, expected);
    }
}
