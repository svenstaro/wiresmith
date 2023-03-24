mod fixtures;
mod utils;

use std::{collections::HashSet, time::Duration};

use anyhow::Result;
use assert_fs::TempDir;
use fixtures::{consul, tmpdir, Consul};
use rstest::rstest;
use tokio::time::sleep;
use wireguard_keys::Privkey;
use wiresmith::{networkd::NetworkdConfiguration, wireguard::WgPeer};

use crate::{utils::spawn_wiresmith, utils::wait_for_files};

/// An initial configuration with a single peer is created in case no existing peers are found.
/// The address of the peer is not explicitly provided. Instead, the first free address inside the
/// network is used.
#[rstest]
#[tokio::test]
async fn initial_configuration(consul: Consul, tmpdir: TempDir) -> Result<()> {
    let mut wiresmith = spawn_wiresmith("10.0.0.0/24", "192.168.0.1", consul.http_port, &tmpdir);

    let network_file = tmpdir.join("wg0.network");
    let netdev_file = tmpdir.join("wg0.netdev");

    wait_for_files(vec![network_file.as_path(), netdev_file.as_path()]).await;

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

/// A second peer is joining the network after the first one has created the initial configuration.
/// This should cause the first peer to generate a new network config with the new peer. The second
/// peer should generate a network config containing the first peer.
#[rstest]
#[tokio::test]
async fn join_network(
    consul: Consul,
    #[from(tmpdir)] tmpdir_a: TempDir,
    #[from(tmpdir)] tmpdir_b: TempDir,
) -> Result<()> {
    let mut wiresmith_a =
        spawn_wiresmith("10.0.0.0/24", "192.168.0.1", consul.http_port, &tmpdir_a);

    let network_file_a = tmpdir_a.join("wg0.network");
    let netdev_file_a = tmpdir_a.join("wg0.netdev");

    wait_for_files(vec![network_file_a.as_path(), netdev_file_a.as_path()]).await;

    // Start the second peer after the first one has generated its files so we don't run into race
    // conditions with address allocation.
    let mut wiresmith_b =
        spawn_wiresmith("10.0.0.0/24", "192.168.0.2", consul.http_port, &tmpdir_b);

    let network_file_b = tmpdir_b.join("wg0.network");
    let netdev_file_b = tmpdir_b.join("wg0.netdev");

    wait_for_files(vec![network_file_b.as_path(), netdev_file_b.as_path()]).await;

    // Wait until the first client has had a chance to pick up the changes and generate a new
    // config.
    sleep(Duration::from_secs(20)).await;

    let networkd_config_a = NetworkdConfiguration::from_config(&tmpdir_a, "wg0")?;
    let networkd_config_b = NetworkdConfiguration::from_config(&tmpdir_b, "wg0")?;

    // Check the networkd files of the first peer.
    assert_eq!(networkd_config_a.wg_address, "10.0.0.1/24".parse()?);
    assert_eq!(networkd_config_b.wg_address, "10.0.0.2/24".parse()?);

    // // Check the config put into Consul.
    let peers = consul.client.get_peers().await?;
    let mut expected_peers = HashSet::new();
    expected_peers.insert(WgPeer {
        public_key: networkd_config_a.public_key,
        endpoint: "192.168.0.1:51820".parse().unwrap(),
        address: "10.0.0.1/32".parse().unwrap(),
    });
    expected_peers.insert(WgPeer {
        public_key: networkd_config_b.public_key,
        endpoint: "192.168.0.2:51820".parse().unwrap(),
        address: "10.0.0.2/32".parse().unwrap(),
    });
    assert_eq!(peers, expected_peers);
    assert_eq!(networkd_config_a.peers, expected_peers);
    assert_eq!(networkd_config_b.peers, expected_peers);

    wiresmith_a.kill()?;
    wiresmith_b.kill()?;

    Ok(())
}
