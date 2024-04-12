mod args;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use clap::Parser;
use tokio::time::sleep;
use tracing::{debug, info, trace};

use wiresmith::{
    consul::ConsulClient,
    networkd::NetworkdConfiguration,
    wireguard::{latest_transfer_rx, WgPeer},
};

#[tokio::main]
async fn main() -> Result<()> {
    let args = args::CliArgs::parse();

    if args.verbose == 2 {
        tracing_subscriber::fmt()
            .with_env_filter("wiresmith=trace")
            .init();
    } else if args.verbose == 1 {
        tracing_subscriber::fmt()
            .with_env_filter("wiresmith=debug")
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter("wiresmith=info")
            .init();
    };

    if let Some(address) = args.address {
        ensure!(
            args.network.contains(&address),
            "Address {address} is not part of network {}",
            args.network
        );
    }

    let consul_client = ConsulClient::new(
        args.consul_address,
        &args.consul_prefix,
        args.consul_token.as_deref(),
        args.consul_datacenter,
    )?;

    let endpoint_address = if let Some(endpoint_address) = args.endpoint_address {
        endpoint_address
    } else if let Some(endpoint_interface) = args.endpoint_interface {
        // Find suitable IP on provided interface.
        endpoint_interface
            .ips
            .first()
            .context("No IPs on interface")?
            .ip()
            .to_string()
    } else {
        unreachable!("Should have been handled by arg parsing");
    };

    consul_client.acquire_lock().await?;
    info!("Getting existing peers from Consul");
    let peers = consul_client.get_peers().await?;
    if peers.is_empty() {
        info!("No existing peers found in Consul");
    } else {
        info!("Found {} existing peer(s) in Consul", peers.len());
        debug!("Existing peers:\n{:#?}", peers);
    }

    // Check whether we can find and parse an existing config.
    if NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface)
        .await
        .is_ok()
    {
        info!("Successfully loading existing systemd-networkd config");
    } else {
        info!("No existing WireGuard configuration found on system, creating a new one");

        // If we can't find or parse an existing config, we'll just generate a new one.
        let networkd_config = NetworkdConfiguration::new(
            args.address,
            args.network,
            args.wg_port,
            &args.wg_interface,
            peers,
        )?;
        networkd_config
            .write_config(&args.networkd_dir, args.keepalive)
            .await?;
        info!("Our new config is:\n{:#?}", networkd_config);
    }

    info!("Restarting systemd-networkd");
    NetworkdConfiguration::restart().await?;
    consul_client.drop_lock().await?;

    // Stores amount of received bytes together with a timestamp for every peer.
    let mut received_bytes: HashMap<wireguard_keys::Pubkey, (usize, Instant)> = HashMap::new();

    // Enter main loop which periodically checks for updates to the list of WireGuard peers.
    loop {
        if args.peer_timeout > Duration::ZERO {
            // Fetch latest received bytes from peers. If new bytes were received, update the
            // hashmap entry. Delete the peer if no new bytes were received for duration
            // `peer_timeout`.
            let transfer_rates = latest_transfer_rx(&args.wg_interface)
                .await
                .context("Couldn't get list of transfer rates from WireGuard")?;
            for (pubkey, bytes) in transfer_rates {
                let (old_bytes, timestamp) = received_bytes
                    .entry(pubkey)
                    .or_insert((bytes, Instant::now()));
                if timestamp.elapsed() > args.peer_timeout && bytes.eq(old_bytes) {
                    info!(
                        "Peer {} has exceeded timeout of {:?}, deleting",
                        pubkey.to_base64_urlsafe(),
                        args.peer_timeout
                    );
                    consul_client
                        .delete_config(pubkey)
                        .await
                        .context("Couldn't delete peer from Consul")?;
                } else if !bytes.eq(old_bytes) {
                    *old_bytes = bytes;
                    *timestamp = Instant::now();
                }
            }
        }

        consul_client.acquire_lock().await?;
        trace!("Checking Consul for peer updates");
        let peers = consul_client
            .get_peers()
            .await
            .context("Can't fetch existing peers from Consul")?;
        let mut networkd_config =
            NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface)
                .await
                .context("Couldn't load existing NetworkdConfiguration from disk")?;

        // Exclude own peer config.
        let peers_without_own_config = peers
            .iter()
            .filter(|&x| x.public_key != networkd_config.public_key)
            .cloned()
            .collect::<HashSet<WgPeer>>();

        // If there is a mismatch, write a new networkd configuration.
        let additional_peers = peers_without_own_config
            .difference(&networkd_config.peers)
            .collect::<Vec<_>>();
        let deleted_peers = networkd_config
            .peers
            .difference(&peers_without_own_config)
            .collect::<Vec<_>>();
        if !additional_peers.is_empty() {
            info!("Found {} new peer(s) in Consul", additional_peers.len());
            debug!("New peers: {:#?}", additional_peers);
        }
        if !deleted_peers.is_empty() {
            info!("Found {} deleted peer(s) in Consul", deleted_peers.len());
            debug!("Deleted peers: {:#?}", deleted_peers);
        }

        if !additional_peers.is_empty() || !deleted_peers.is_empty() {
            networkd_config.peers = peers_without_own_config;
            networkd_config
                .write_config(&args.networkd_dir, args.keepalive)
                .await
                .context("Couldn't write new NetworkdConfiguration")?;

            info!("Restarting systemd-networkd to apply new config");
            NetworkdConfiguration::restart()
                .await
                .context("Error restarting systemd-networkd")?;
        }

        // Add our own peer config to Consul. It could be lacking for two reasons:
        // 1. Either this is the first run and we were not added to Consul yet.
        // 2. We were removed due to a timeout (temporary network outage) and have to re-add
        //    ourselves.
        let own_peer_is_in_consul = peers
            .iter()
            .any(|x| x.public_key == networkd_config.public_key);

        if !own_peer_is_in_consul {
            info!("Existing WireGuard peer config doesn't yet exist in Consul");

            // Send the config to Consul.
            let wg_peer = WgPeer::new(
                networkd_config.public_key,
                &format!("{endpoint_address}:{}", args.wg_port),
                networkd_config.wg_address.addr(),
            );
            info!("Submitted own WireGuard peer config:\n{:#?}", wg_peer);

            consul_client
                .put_config(wg_peer)
                .await
                .context("Failed to put peer config into Consul")?;
            info!("Wrote own WireGuard peer config to Consul");
        }
        consul_client.drop_lock().await?;

        sleep(args.update_period).await;
    }
}
