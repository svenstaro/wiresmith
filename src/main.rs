mod args;

use std::collections::HashSet;

use anyhow::{ensure, Context, Result};
use clap::Parser;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace};

use wiresmith::{consul::ConsulClient, networkd::NetworkdConfiguration, wireguard::WgPeer};

#[tokio::main]
async fn main() -> Result<()> {
    // Spawn a task to cancel us if we receive a SIGINT.
    let token = CancellationToken::new();
    tokio::spawn({
        let token = token.clone();
        async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for SIGINT");
            token.cancel();
        }
    });

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

    info!("Getting existing peers from Consul");
    let peers = consul_client.get_peers().await?;
    if peers.is_empty() {
        info!("No existing peers found in Consul");
    } else {
        info!("Found {} existing peer(s) in Consul", peers.len());
        debug!("Existing peers:\n{:#?}", peers);
    }

    // Check whether we can find and parse an existing config.
    let networkd_config = if let Ok(config) =
        NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface).await
    {
        info!("Successfully loading existing systemd-networkd config");
        config
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
        networkd_config
    };

    info!("Restarting systemd-networkd");
    NetworkdConfiguration::restart().await?;

    let consul_session = consul_client
        .create_session(networkd_config.public_key, args.consul_ttl, token.clone())
        .await?;

    let own_wg_peer = WgPeer::new(
        networkd_config.public_key,
        &format!("{endpoint_address}:{}", args.wg_port),
        networkd_config.wg_address.addr(),
    );

    info!("Submitting own WireGuard peer config:\n{:#?}", own_wg_peer);
    let config_checker = consul_session
        .put_config(own_wg_peer, token.clone())
        .await
        .context("Failed to put own peer config into Consul")?;
    info!("Wrote own WireGuard peer config to Consul");

    // Enter main loop which periodically checks for updates to the list of WireGuard peers.
    loop {
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

        // Wait until we've either been told to shut down or until we've slept for the update
        // period.
        //
        // TODO: Use long polling instead of periodic checks.
        tokio::select! {
            _ = token.cancelled() => {
                info!("Received SIGINT, shutting down");
                break;
            },
            _ = sleep(args.update_period) => continue,
        };
    }

    // Cancel the config checker first so we don't get spurious errors if the session is destroyed
    // first.
    config_checker
        .cancel()
        .await
        .context("Failed to join Consul config checker task")?;

    // Wait for the Consul session handler to destroy our session and exit. It was cancelled by the
    // same `CancellationToken` that cancelled us.
    consul_session
        .cancel()
        .await
        .context("Failed to join Consul session handler task")?;

    Ok(())
}
