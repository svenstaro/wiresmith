use std::{net::IpAddr, path::PathBuf};

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

    /// Update period - how often to check for peer updates in seconds
    #[arg(short, long, default_value = "10")]
    pub update_period: u64,

    /// WireGuard interface name
    #[arg(short = 'i', long, default_value = "wg0")]
    pub wg_interface: String,

    /// WireGuard UDP listen port
    #[arg(short = 'p', long, default_value = "51820")]
    pub wg_port: u16,

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
    /// You need to provide either this or --endpoint-interface.
    #[arg(
        long,
        required_unless_present = "endpoint_interface",
        conflicts_with = "endpoint_interface"
    )]
    pub endpoint_address: Option<IpAddr>,

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

    /// Be verbose.
    #[arg(short, long)]
    pub verbose: bool,
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
