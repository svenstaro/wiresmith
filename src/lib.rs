pub mod consul;
pub mod networkd;
pub mod wireguard;

use std::{net::SocketAddr, path::Path};

use anyhow::Result;
use wireguard::WgPeer;

use crate::{consul::ConsulClient, networkd::NetworkdConfiguration};

/// Generate a new node config and write it to the local network configuration and Consul.
#[tracing::instrument(skip(consul_client))]
pub async fn make_new_config(
    endpoint: SocketAddr,
    networkd_config: &NetworkdConfiguration,
    networkd_dir: &Path,
    consul_client: &ConsulClient,
) -> Result<()> {
    networkd_config.write_config(networkd_dir)?;

    // Send the config to Consul.
    let wg_peer = WgPeer::new(
        networkd_config.public_key,
        endpoint,
        networkd_config.wg_address.addr(),
    );
    consul_client.put_config(wg_peer).await
}
