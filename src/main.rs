mod args;

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use clap::Parser;
use tokio::time::sleep;
use tracing::{debug, info, trace};

use wiresmith::{
    consul::ConsulClient,
    networkd::NetworkdConfiguration,
    wireguard::{latest_handshakes, WgPeer},
};

#[tokio::main]
async fn main() -> Result<()> {
    let args = args::CliArgs::parse();

    if args.verbose {
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

    let endpoint = format!("{endpoint_address}:{}", args.wg_port);

    info!("Getting existing peers from Consul");
    let peers = consul_client.get_peers().await?;
    if peers.is_empty() {
        info!("No existing peers found in Consul");
    } else {
        info!("Found {} existing peer(s) in Consul", peers.len());
        debug!("Existing peers: {:#?}", peers);
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
        networkd_config.write_config(&args.networkd_dir).await?;
    }

    info!("Restarting systemd-networkd");
    NetworkdConfiguration::restart().await?;

    // Enter main loop which periodically checks for updates to the list of WireGuard peers.
    // To make sure we give new peers a chance to send handshakes, we need to debounce
    // timeout removals.
    let mut last_timeout_removal = Instant::now();
    let timeout_debounce = Duration::from_secs(60);

    loop {
        trace!("Checking Consul for peer updates");
        let peers = consul_client
            .get_peers()
            .await
            .expect("Can't fetch existing peers from Consul");
        let mut networkd_config =
            NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface)
                .await
                .expect("Couldn't load existing NetworkdConfiguration from disk");

        if args.peer_timeout > Duration::ZERO && last_timeout_removal.elapsed() > timeout_debounce {
            let handshakes = latest_handshakes(&args.wg_interface)
                .await
                .expect("Couldn't get list of handshakes from WireGuard");
            for (pubkey, latest_handshake) in handshakes {
                // This can fail due to clock drift and potentially it could be a negative
                // value. In that case, we just do nothing in that particular iteration.
                if let Ok(elapsed) = latest_handshake.elapsed() {
                    if elapsed >= args.peer_timeout {
                        info!(
                            "Peer {} has exceeded timeout of {:?}, deleting",
                            pubkey.to_base64_urlsafe(),
                            args.peer_timeout
                        );
                        consul_client
                            .delete_config(pubkey)
                            .await
                            .expect("Couldn't delete peer from Consul");
                        last_timeout_removal = Instant::now();
                    }
                }
            }
        }

        // Exclude own peer config.
        let peers_without_own_config = peers
            .iter()
            .cloned()
            .filter(|x| x.public_key != networkd_config.public_key)
            .collect::<HashSet<WgPeer>>();

        // If there is a mismatch, write a new networkd configuration.
        let diff = peers_without_own_config
            .difference(&networkd_config.peers)
            .collect::<Vec<_>>();
        if !diff.is_empty() {
            info!("Found {} new peer(s) in Consul", diff.len());
            debug!("New peers: {:#?}", diff);

            networkd_config.peers = peers_without_own_config;
            networkd_config
                .write_config(&args.networkd_dir)
                .await
                .expect("Couldn't write new NetworkdConfiguration");

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
            info!("Existing WireGuard config doesn't yet exist in Consul");

            // Send the config to Consul.
            let wg_peer = WgPeer::new(
                networkd_config.public_key,
                &endpoint,
                networkd_config.wg_address.addr(),
            );
            info!("Existing peer config:\n{:#?}", wg_peer);

            consul_client
                .put_config(wg_peer)
                .await
                .expect("Failed to put peer config into Consul");
            info!("Wrote own WireGuard peer config to Consul");
        }

        sleep(args.update_period).await;
    }
}
