# wiresmith - Auto-config WireGuard clients into a mesh

[![CI](https://github.com/svenstaro/wiresmith/workflows/CI/badge.svg)](https://github.com/svenstaro/wiresmith/actions)
[![Crates.io](https://img.shields.io/crates/v/wiresmith.svg)](https://crates.io/crates/wiresmith)
[![license](http://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/svenstaro/wiresmith/blob/master/LICENSE)
[![Lines of Code](https://tokei.rs/b1/github/svenstaro/wiresmith)](https://github.com/svenstaro/wiresmith)

**wiresmith** automatically discovers other peers using a shared backend and adds them to the local
network configuration while also publishing the local node so that others can talk to it. In short,
it will create a self-maintaining mesh network using WireGuard.

You choose to let it figure out the addresses by itself or provide static addresses yourself. It
can also clean up dead peers if desired.

## Features

- Simple usage
- Automatic address allocation
- Mesh connectivity
- IPv4/IPv6
- Value store backends: Consul
- Network configuration backends: systemd-networkd
- Cleanup of dead peers
- Pretty logging!

## How to use

You need to at least provide the internal netork to use and the local node's endpoint. The endpoint
can either be an interface or a specific local interface address. For instance, one of the simplest
invocations would be:

    wiresmith --network 192.168.0.0/24 --endpoint-interface eth0

This will:

1. Connect to a local Consul agent
2. Generate or load a local WireGuard configuration for `systemd-networkd`
3. Use an address within the `192.168.0.0/24` WireGuard network for internal addressing
4. Pick a usable global address from `eth0` and uses that to communicate with other peers

The endpoint interface needs to be reachable from all the other peers.

By default, peers that we haven't received a handshake from within 10 minutes are removed.

## Usage

    Auto-config WireGuard clients into a mesh

    Usage: wiresmith [OPTIONS] --network <NETWORK>

    Options:
          --consul-address <CONSUL_ADDRESS>
              Consul backend socket address

              [default: http://127.0.0.1:8500]

          --consul-token <CONSUL_TOKEN>
              Consul secret token

          --consul-prefix <CONSUL_PREFIX>
              Consul KV prefix

              [default: wiresmith]

      -u, --update-period <UPDATE_PERIOD>
              Update period - how often to check for peer updates

              Parses human-friendly time, e.g. 15s

              [default: 10s]

      -i, --wg-interface <WG_INTERFACE>
              WireGuard interface name

              [default: wg0]

      -p, --wg-port <WG_PORT>
              WireGuard UDP listen port

              [default: 51820]

      -t, --peer-timeout <PEER_TIMEOUT>
              Remove disconnected peers after this duration

              Parses human-friendly time, e.g. 5min
              Set to 0 in order to disable.

              [default: 10min]

          --endpoint-interface <ENDPOINT_INTERFACE>
              Public endpoint interface name

              You need to provide either this or --endpoint-address.

          --endpoint-address <ENDPOINT_ADDRESS>
              Public endpoint address

              Can be a hostname or IP address. You need to provide either this or --endpoint-interface.

          --network-backend <NETWORK_BACKEND>
              Network configuration backend

              [default: networkd]
              [possible values: networkd]

          --networkd-dir <NETWORKD_DIR>
              Directory in which to place the generated networkd configuration

              [default: /etc/systemd/network/]

      -a, --address <ADDRESS>
              Address to allocate

              If not provided, will allocate available address from the subnet. For instance 10.0.0.4 or fc00::4

      -n, --network <NETWORK>
              Network to use

              Must be the same for all clients. For instance 10.0.0.0/24 or fc00::/64

      -v, --verbose
              Be verbose

      -h, --help
              Print help (see a summary with '-h')

      -V, --version
              Print version

## How to install

Pre-compiled binaries for supported platforms are available on the
[releases](https://github.com/svenstaro/wiresmith/releases) page.

If you are on Arch Linux, you can just

    pacman -S wiresmith

Alternatively, you can use the provided OCI images using Podman or Docker:

    podman run --rm --name wiresmith --cap-add SYS_ADMIN,NET_ADMIN --network host ghcr.io/svenstaro/wiresmith
    docker run --rm --name wiresmith --privileged --network host ghcr.io/svenstaro/wiresmith

You can also use the provided systemd service.

## Development

This project uses Podman in rootless mode to facilitate rapid local testing. Before starting a
development session, run

    just prepare-test

to make sure you have the necessary images.

Make sure you have [just](https://github.com/casey/just) and [zellij](https://zellij.dev/)
installed locally and then run either `just test` for automatic testing or `just interactive` for
interactive testing. The interactive session will spawn two systemds in containers and then run one
instance of `wiresmith` in each of them so you can watch them and see how they interact.

## Releasing

This is mostly a note for me on how to release this thing:

- Make sure `CHANGELOG.md` is up to date.
- `cargo release <version>`
- `cargo release --execute <version>`
- OCI images and binaries will automatically be deployed by Github Actions.
- Update Arch package.
