use std::{
    collections::HashSet, fmt, fs::Permissions, net::IpAddr, os::unix::prelude::PermissionsExt,
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use file_owner::set_group;
use ipnet::IpNet;
use tokio::{fs, process::Command};
use wireguard_keys::{Privkey, Pubkey};

use crate::wireguard::WgPeer;

/// Find a free address in a network given a list of occupied addresses.
///
/// Returns `None` if there are no free addresses.
#[tracing::instrument]
fn get_free_address(network: &IpNet, peers: &HashSet<WgPeer>) -> Option<IpAddr> {
    let occupied_addresses = peers
        .iter()
        .map(|x| x.address.addr())
        .collect::<HashSet<_>>();
    for host in network.hosts() {
        if !occupied_addresses.contains(&host) {
            return Some(host);
        }
    }
    None
}

pub struct NetworkdConfiguration {
    pub wg_address: IpNet,
    pub wg_interface: String,
    pub wg_port: u16,
    pub peers: HashSet<WgPeer>,
    pub private_key: Privkey,
    pub public_key: Pubkey,
}

impl fmt::Debug for NetworkdConfiguration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetworkdConfiguration")
            .field("wg_address", &self.wg_address)
            .field("wg_interface", &self.wg_interface)
            .field("wg_port", &self.wg_port)
            .field("peers", &self.peers)
            .field("private_key", &"[REDACTED]")
            .field("public_key", &self.public_key.to_base64_urlsafe())
            .finish()
    }
}

impl NetworkdConfiguration {
    /// Build a new config
    #[tracing::instrument]
    pub fn new(
        address: Option<IpAddr>,
        network: IpNet,
        port: u16,
        wg_interface: &str,
        peers: HashSet<WgPeer>,
    ) -> Result<Self> {
        let address = if let Some(address) = address {
            address
        } else {
            get_free_address(&network, &peers).context("Couldn't find usable address")?
        };

        let wg_address = IpNet::new(address, network.prefix_len())?;
        let private_key = wireguard_keys::Privkey::generate();
        Ok(Self {
            wg_address,
            wg_interface: wg_interface.to_string(),
            wg_port: port,
            peers,
            private_key,
            public_key: private_key.pubkey(),
        })
    }

    /// Read and parse existing config from existing location on disk
    #[tracing::instrument]
    pub async fn from_config(networkd_dir: &Path, wg_interface: &str) -> Result<Self> {
        // Get the list of peers in networkd.
        let netdev_path = networkd_dir.join(wg_interface).with_extension("netdev");
        let netdev_ini = ini::Ini::load_from_file(&netdev_path)?;

        let wg_port = netdev_ini
            .section(Some("WireGuard"))
            .context("Couldn't find [WireGuard] section")?
            .get("ListenPort")
            .context("Couldn't find ListenPort in [WireGuard] section")?
            .parse()?;
        let private_key: Privkey = netdev_ini
            .section(Some("WireGuard"))
            .context("Couldn't find [WireGuard] section")?
            .get("PrivateKey")
            .context("Couldn't find PrivateKey in [WireGuard] section")?
            .parse()?;
        let public_key = private_key.pubkey();

        let mut peers = HashSet::new();
        for peer in netdev_ini.section_all(Some("WireGuardPeer")) {
            let public_key = peer
                .get("PublicKey")
                .context("No PublicKey attribute on WireGuardPeer")?;
            let endpoint = peer
                .get("Endpoint")
                .context("No Endpoint attribute on WireGuardPeer")?;
            let allowed_ips = peer
                .get("AllowedIPs")
                .context("No AllowedIPs attribute on WireGuardPeer")?;
            peers.insert(WgPeer {
                public_key: Pubkey::from_base64(public_key)?,
                endpoint: endpoint.parse()?,
                address: allowed_ips.parse()?,
            });
        }

        let network_path = networkd_dir.join(wg_interface).with_extension("network");
        let network_ini = ini::Ini::load_from_file(network_path)?;

        let wg_address = network_ini
            .section(Some("Network"))
            .context("Couldn't find [Network] section")?
            .get("Address")
            .context("Couldn't find Address in [Network] section")?
            .parse()?;

        Ok(Self {
            wg_interface: wg_interface.to_string(),
            wg_address,
            wg_port,
            peers,
            private_key,
            public_key,
        })
    }

    /// Generate and write systemd-networkd config
    #[tracing::instrument]
    pub async fn write_config(&self, networkd_dir: &Path) -> Result<()> {
        let network_file = format!(
            "\
[Match]
Name={}

[Network]
Address={}\n",
            self.wg_interface, self.wg_address
        );

        let mut netdev_file = format!(
            "\
[NetDev]
Name={}
Kind=wireguard
Description=WireGuard client
MTUBytes=1280

[WireGuard]
ListenPort={}
PrivateKey={}\n",
            self.wg_interface, self.wg_port, self.private_key
        );

        for peer in &self.peers {
            let peer_str = format!(
                "\n
[WireGuardPeer]
PublicKey={}
Endpoint={}
AllowedIPs={}
PersistentKeepalive=25",
                peer.public_key, peer.endpoint, peer.address
            );
            netdev_file.push_str(&peer_str);
        }
        let network_path = networkd_dir
            .join(&self.wg_interface)
            .with_extension("network");
        let netdev_path = networkd_dir
            .join(&self.wg_interface)
            .with_extension("netdev");

        fs::write(&network_path, network_file)
            .await
            .context(format!("Couldn't write config to {network_path:?}"))?;
        fs::write(&netdev_path, netdev_file)
            .await
            .context(format!("Couldn't write config to {netdev_path:?}"))?;
        fs::set_permissions(&netdev_path, Permissions::from_mode(0o640)).await?;
        set_group(netdev_path, "systemd-network")?;

        Ok(())
    }

    /// Restart systemd-networkd
    #[tracing::instrument]
    pub async fn restart() -> Result<()> {
        let restart_output = Command::new("systemctl")
            .arg("restart")
            .arg("systemd-networkd")
            .output()
            .await?;
        if !restart_output.status.success() {
            let stderr = String::from_utf8_lossy(&restart_output.stderr);
            let journalctl_output = Command::new("journalctl")
                .arg("-u")
                .arg("systemd-networkd")
                .output()
                .await?;
            let journalctl_stdout = String::from_utf8_lossy(&journalctl_output.stdout);
            return Err(anyhow!("Failed to restart systemd-networkd: {stderr}\njournalctl -xeu systemd-networkd: {journalctl_stdout}"));
        }
        Ok(())
    }
}
