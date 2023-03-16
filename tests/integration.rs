mod fixtures;
mod utils;

use std::collections::HashSet;

use anyhow::Result;
use assert_fs::TempDir;
use fixtures::{consul, tmpdir, Consul};
use rstest::rstest;
use wireguard_keys::Privkey;
use wiresmith::wireguard::WgPeer;

use crate::{utils::spawn_wiresmith, utils::wait_for_files};

/// An initial configuration with a single peer is created in case no existing peers are found.
#[rstest]
#[tokio::test]
async fn initial_configuration(consul: Consul, tmpdir: TempDir) -> Result<()> {
    let mut wiresmith = spawn_wiresmith("10.0.0.1/24", "192.168.0.1", consul.http_port, &tmpdir);

    let network_file = tmpdir.join("wg0.network");
    let netdev_file = tmpdir.join("wg0.netdev");

    wait_for_files(vec![network_file.as_path(), netdev_file.as_path()]);

    // Check the networkd files.
    let network_entry = freedesktop_entry_parser::parse_entry(network_file)?;
    assert_eq!(network_entry.section("Match").attr("Name").unwrap(), "wg0");
    assert_eq!(
        network_entry.section("Network").attr("Address").unwrap(),
        "10.0.0.1/24"
    );

    let netdev_entry = freedesktop_entry_parser::parse_entry(netdev_file)?;
    assert_eq!(netdev_entry.section("NetDev").attr("Name").unwrap(), "wg0");
    assert_eq!(
        netdev_entry.section("NetDev").attr("Kind").unwrap(),
        "wireguard"
    );
    assert_eq!(
        netdev_entry.section("NetDev").attr("Description").unwrap(),
        "WireGuard client"
    );
    assert_eq!(
        netdev_entry.section("NetDev").attr("MTUBytes").unwrap(),
        "1280"
    );

    // The private key is generated automatically but we should verify it's valid.
    let private_key = Privkey::from_base64(
        netdev_entry
            .section("WireGuard")
            .attr("PrivateKey")
            .unwrap(),
    )?;

    // Check the config put into Consul.
    let peers = consul.client.get_peers().await?;
    let mut expected_peers = HashSet::new();
    expected_peers.insert(WgPeer {
        public_key: private_key.pubkey(),
        endpoint: "192.168.0.1:51820".parse().unwrap(),
        address: "10.0.0.1/32".parse().unwrap(),
    });
    assert_eq!(peers, expected_peers);

    wiresmith.kill()?;

    Ok(())
}
