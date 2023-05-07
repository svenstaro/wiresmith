use std::{fmt, net::IpAddr};

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

fn parse_latest_transfer_rx(s: &str) -> Result<Vec<(Pubkey, usize)>> {
    let mut peers = vec![];
    for line in s.lines() {
        let split_line = line.split_ascii_whitespace().collect::<Vec<_>>();
        peers.push((split_line[0].parse()?, split_line[1].parse()?));
    }
    Ok(peers)
}

/// Get a list of latest received bytes of peers by running 'wg show <interface> transfer'
pub async fn latest_transfer_rx(interface: &str) -> Result<Vec<(Pubkey, usize)>> {
    let output = Command::new("wg")
        .arg("show")
        .arg(interface)
        .arg("transfer")
        .output()
        .await?;
    ensure!(
        output.status.success(),
        "Couldn't get output of wg show: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output_reader = std::str::from_utf8(&output.stdout)?;

    parse_latest_transfer_rx(output_reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn latest_transfer_test() {
        let transfer_output = "\
MkgQcW7mlCtqWIV3JrtIrBRgG9efxwSvnXOsU1R7x2c=	304	272
pKidG6sLcARl/OiB7j8s9yPeo/20fEHuxBi4aamAuVo=	308	272
MkgQcW7mlCtqWIV3JrtIrBRgG9efxwSvnXOsU1R7x2c=	0 272
pKidG6sLcARl/OiB7j8s9yPeo/20fEHuxBi4aamAuVo=	0 0";

        let result = parse_latest_transfer_rx(transfer_output).unwrap();
        let expected = vec![
            (
                Pubkey::from_base64("MkgQcW7mlCtqWIV3JrtIrBRgG9efxwSvnXOsU1R7x2c").unwrap(),
                304,
            ),
            (
                Pubkey::from_base64("pKidG6sLcARl/OiB7j8s9yPeo/20fEHuxBi4aamAuVo").unwrap(),
                308,
            ),
            (
                Pubkey::from_base64("MkgQcW7mlCtqWIV3JrtIrBRgG9efxwSvnXOsU1R7x2c").unwrap(),
                0,
            ),
            (
                Pubkey::from_base64("pKidG6sLcARl/OiB7j8s9yPeo/20fEHuxBi4aamAuVo").unwrap(),
                0,
            ),
        ];
        assert_eq!(result, expected);
    }
}
