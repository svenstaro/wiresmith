use std::{net::IpAddr, path::PathBuf, time::Duration};

use clap::{Parser, ValueEnum};
use ipnet::IpNet;
use pnet::datalink::{self, NetworkInterface};
use reqwest::Url;

#[derive(Copy, Clone, ValueEnum)]
pub enum NetworkBackend {
    Networkd,
    // Wgquick
}

#[derive(Parser)]
#[command(name = "wiresmith", author, about, version)]
pub struct CliArgs {
    /// Consul backend socket address
    #[arg(long, default_value = "http://127.0.0.1:8500")]
    pub consul_address: Url,

    /// Consul secret token
    #[arg(long)]
    pub consul_token: Option<String>,

    /// Consul KV prefix
    #[arg(long, default_value = "wiresmith")]
    pub consul_prefix: String,

    /// Update period - how often to check for peer updates
    #[arg(short, long, default_value = "10s", value_parser = humantime::parse_duration)]
    pub update_period: Duration,

    /// WireGuard interface name
    #[arg(short = 'i', long, default_value = "wg0")]
    pub wg_interface: String,

    /// WireGuard UDP listen port
    #[arg(short = 'p', long, default_value = "51820")]
    pub wg_port: u16,

    /// Remove disconnected peers after this duration
    ///
    /// Set to 0 in order to disable.
    #[arg(short = 't', long, default_value = "10min", value_parser = humantime::parse_duration)]
    pub peer_timeout: Duration,

    /// Public endpoint interface name
    ///
    /// You need to provide either this or --endpoint-address.
    #[arg(long,
        required_unless_present = "endpoint_address",
        conflicts_with = "endpoint_address",
        value_parser = network_interface
    )]
    pub endpoint_interface: Option<NetworkInterface>,

    /// Public endpoint address
    ///
    /// Can be a hostname or IP address.
    /// You need to provide either this or --endpoint-interface.
    #[arg(
        long,
        required_unless_present = "endpoint_interface",
        conflicts_with = "endpoint_interface"
    )]
    pub endpoint_address: Option<String>,

    /// Network configuration backend
    #[arg(long, default_value = "networkd")]
    pub network_backend: NetworkBackend,

    /// Directory in which to place the generated networkd configuration
    #[arg(long, default_value = "/etc/systemd/network/")]
    pub networkd_dir: PathBuf,

    /// Address to allocate
    ///
    /// If not provided, will allocate available address from the subnet.
    /// For instance 10.0.0.4 or fc00::4
    #[arg(short, long)]
    pub address: Option<IpAddr>,

    /// Network to use
    ///
    /// Must be the same for all clients.
    /// For instance 10.0.0.0/24 or fc00::/64
    #[arg(short, long)]
    pub network: IpNet,

    /// Be verbose
    ///
    /// Provide twice for very verbose.
    #[arg(short, long, action = clap::ArgAction::Count, value_parser = clap::value_parser!(u8).range(0..=2))]
    pub verbose: u8,
}

fn network_interface(s: &str) -> Result<NetworkInterface, String> {
    let interfaces = datalink::interfaces();
    let interface = interfaces
        .iter()
        .find(|e| e.is_up() && !e.is_loopback() && !e.ips.is_empty() && e.name == s);
    match interface {
        Some(i) => Ok(i.clone()),
        None => Err(format!("No usable interface found for '{}'", s)),
    }
}
