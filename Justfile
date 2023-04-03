prepare-test:
    podman pull docker.io/hashicorp/consul docker.io/archlinux
    podman build -f Containerfile.testing --tag wiresmith-testing

test:
    cargo test

# Interactive test using tmux and podman
interactive:
    cargo build

    podman rm --depend -f --filter label=testcontainer
    podman run -t --replace --rm --name consul -l testcontainer -d -p 8500:8500 docker.io/hashicorp/consul agent -dev -client 0.0.0.0
    podman run -t --replace --rm --name wiresmith1 -l testcontainer -d -p 11111:11111/udp --hostname wiresmith1 --network slirp4netns:allow_host_loopback=true --cap-add SYS_ADMIN,NET_ADMIN -v ./target/debug/wiresmith:/usr/bin/wiresmith --tz UTC --entrypoint /sbin/init docker.io/archlinux
    podman run -t --replace --rm --name wiresmith2 -l testcontainer -d -p 22222:22222/udp --hostname wiresmith2 --network slirp4netns:allow_host_loopback=true --cap-add SYS_ADMIN,NET_ADMIN -v ./target/debug/wiresmith:/usr/bin/wiresmith --tz UTC --entrypoint /sbin/init docker.io/archlinux

    zellij --layout interactive_test.kdl

    sleep 1
    podman rm --depend -f --filter label=testcontainer

watch-interactive:
    cargo watch -cs 'just interactive'
