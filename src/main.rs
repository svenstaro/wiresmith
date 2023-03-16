mod args;

use std::collections::HashSet;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{debug, info};
use wireguard_keys::Privkey;

use wiresmith::{
    consul::ConsulClient, make_new_config, networkd::NetworkdConfiguration, wireguard::WgPeer,
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
        make_new_config(
            endpoint,
            &networkd_config,
            &args.networkd_dir,
            &consul_client,
        )
        .await?;
    } else {
        info!("Found {} existing peer(s) in Consul", peers.len());
        debug!("Existing peers: {:#?}", peers);

        // let (private_key, public_key) = get_existing_keys(&args.wg_interface)?;
        // Get existing WireGuard private key.
        if let Ok(networkd_netdev) = freedesktop_entry_parser::parse_entry(
            args.networkd_dir
                .join(&args.wg_interface)
                .with_extension("netdev"),
        ) {
            if let Some(private_key_base64) =
                networkd_netdev.section("WireGuard").attr("PrivateKey")
            {
                let private_key = Privkey::from_base64(private_key_base64)?;
                info!("Found existing WireGuard private key");

                // Derive the public key from the private key and then check whether our own configuration
                // is already in Consul.
                let public_key = private_key.pubkey();

                // Try to find own config in Consul.
                if peers.iter().find(|x| x.public_key == public_key).is_none() {
                    info!("Existing WireGuard config doesn't yet exist in Consul");
                    // Send the config to Consul.
                    let wg_peer = WgPeer {
                        public_key: private_key.pubkey(),
                        endpoint,
                        address: "10.0.0.0/24".parse()?, // TODO
                    };
                    consul_client.put_config(wg_peer).await?;
                    info!("Wrote own WireGuard config to Consul");
                } else {
                    info!("Existing WireGuard config already known to Consul")
                }
            }
        } else {
            info!("No existing WireGuard configuration found on system");

            let networkd_config =
                NetworkdConfiguration::new(args.address, args.network, &args.wg_interface, peers)?;
            make_new_config(
                endpoint,
                &networkd_config,
                &args.networkd_dir,
                &consul_client,
            )
            .await?;
        }
    }

    // Enter main loop which periodically checks for updates to the list of WireGuard peers.
    // let main_loop = task::spawn(async move {
    //     loop {
    //         debug!("Checking Consul for peer updates");
    //         let peers = consul_client
    //             .get_peers()
    //             .await
    //             .expect("Can't fetch existing peers from Consul");
    //         let peers_networkd = peers_from_networkd(&args.wg_interface)?;
    //
    //         // If there is a mismatch, write a new networkd configuration.
    //         if peers != peers_networkd {
    //             // let (private_key, _) = get_existing_keys(&args.wg_interface)?;
    //             // generate_networkd_config(private_key, wg_address, &args.wg_interface, peers)?;
    //         }
    //         sleep(Duration::from_secs(args.update_period)).await;
    //     }
    // });
    //
    // main_loop.await?;

    Ok(())
}
