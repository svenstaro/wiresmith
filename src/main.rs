mod args;

use std::{collections::HashSet, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::{task, time::sleep};
use tracing::{debug, info, trace};

use wiresmith::{consul::ConsulClient, networkd::NetworkdConfiguration, wireguard::WgPeer};

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

    let consul_client = ConsulClient::new(
        args.consul_address,
        &args.consul_prefix,
        args.consul_token.as_ref().map(|x| x.as_str()),
    )?;

    info!("Getting existing peers from Consul");
    let peers = consul_client.get_peers().await?;

    let endpoint_address = if let Some(endpoint_address) = args.endpoint_address {
        endpoint_address
    } else if let Some(endpoint_interface) = args.endpoint_interface {
        // Find suitable IP on provided interface.
        endpoint_interface
            .ips
            .first()
            .context("No IPs on interface")?
            .ip()
    } else {
        unreachable!("Should have been handled by arg parsing");
    };

    let endpoint = format!("{endpoint_address}:{}", args.wg_port).parse()?;

    // There are multiple scenarios here for us:
    // 1. There are no existing peers on Consul. That means this node is the first one and we get
    //    to write the first config.
    // 2. There are existing peers on Consul. We then check for whether there's already a
    //    WireGuard configuration on the local node.
    //    a) If there is, we look at the private key, derive the public key and check whether the
    //       public key is already known to Consul. If it is not, add it along with our peer configuration.
    //    b) If there is no local WireGuard configuration
    // If there are no keys yet, add own config without regard for existing clients as there are
    // none.
    if peers.is_empty() {
        info!("No existing peers found in Consul");

        let networkd_config = NetworkdConfiguration::new(
            args.address,
            args.network,
            &args.wg_interface,
            HashSet::new(),
        )?;
        networkd_config.write_config(&args.networkd_dir)?;
        let wg_peer = WgPeer::new(
            networkd_config.public_key,
            endpoint,
            networkd_config.wg_address.addr(),
        );
        consul_client.put_config(wg_peer).await?;
    } else {
        info!("Found {} existing peer(s) in Consul", peers.len());
        debug!("Existing peers: {:#?}", peers);

        if let Ok(networkd_config) =
            NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface)
        {
            // Try to find own config in Consul.
            if peers
                .iter()
                .find(|x| x.public_key == networkd_config.public_key)
                .is_none()
            {
                info!("Existing WireGuard config doesn't yet exist in Consul");
                // Send the config to Consul.
                let wg_peer = WgPeer::new(
                    networkd_config.public_key,
                    endpoint,
                    networkd_config.wg_address.addr(),
                );
                consul_client.put_config(wg_peer).await?;
                info!("Wrote own WireGuard config to Consul");
            } else {
                info!("Existing WireGuard config already known to Consul")
            }
        } else {
            info!("No existing WireGuard configuration found on system");

            let networkd_config =
                NetworkdConfiguration::new(args.address, args.network, &args.wg_interface, peers)?;
            networkd_config.write_config(&args.networkd_dir)?;
            let wg_peer = WgPeer::new(
                networkd_config.public_key,
                endpoint,
                networkd_config.wg_address.addr(),
            );
            consul_client.put_config(wg_peer).await?;
        }
    }

    // Enter main loop which periodically checks for updates to the list of WireGuard peers.
    let main_loop = task::spawn(async move {
        loop {
            trace!("Checking Consul for peer updates");
            let peers = consul_client
                .get_peers()
                .await
                .expect("Can't fetch existing peers from Consul");
            let mut networkd_config =
                NetworkdConfiguration::from_config(&args.networkd_dir, &args.wg_interface)
                    .expect("Couldn't load existing NetworkdConfiguration from disk");

            // Exclude own peer config.
            let peers = peers
                .into_iter()
                .filter(|x| x.public_key != networkd_config.public_key)
                .collect::<HashSet<WgPeer>>();

            // If there is a mismatch, write a new networkd configuration.
            let diff = peers.difference(&networkd_config.peers).collect::<Vec<_>>();
            if !diff.is_empty() {
                info!("Found {} new peer(s) in Consul", diff.len());
                debug!("New peers: {:#?}", diff);

                networkd_config.peers = peers;
                networkd_config
                    .write_config(&args.networkd_dir)
                    .expect("Couldn't write new NetworkdConfiguration");
            }
            sleep(Duration::from_secs(args.update_period)).await;
        }
    });

    main_loop.await?;

    Ok(())
}
