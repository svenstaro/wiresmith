use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::Result;
use tokio::{process::Command, time::sleep};

/// Wait a few seconds for the files to become available
pub async fn wait_for_files(files: Vec<&Path>) {
    let start_time = Instant::now();

    while !files.iter().all(|x| x.exists()) {
        sleep(Duration::from_millis(100)).await;

        if start_time.elapsed().as_secs() > 3 {
            panic!("Timeout waiting {files:?} to exist");
        }
    }
    println!(
        "Files available after waiting for {:?}",
        start_time.elapsed()
    );
}

#[derive(PartialEq)]
pub struct WiresmithContainer {
    /// Full unique container_name
    ///
    /// This is built using {name}-{consul-port}.
    pub container_name: String,
}

impl WiresmithContainer {
    /// Launch an instance of wiresmith in a podman container with systemd.
    pub async fn new(
        name: &str,
        network: &str,
        container_network: &str,
        consul_port: u16,
        args: &[&str],
        dir: &Path,
    ) -> Self {
        let container_name = format!("{name}-{consul_port}");

        // Launch archlinux container with systemd inside.
        Command::new("podman")
            .arg("run")
            .args(["--name", &container_name])
            .arg("--replace")
            .arg("--rm")
            .args(["--label", "testcontainer"])
            // SYS_ADMIN could be removed when https://github.com/systemd/systemd/pull/26478 is released
            .args(["--cap-add", "SYS_ADMIN,NET_ADMIN"])
            .args(["--network", container_network])
            .args([
                "-v",
                concat!(env!("CARGO_BIN_EXE_wiresmith"), ":/usr/bin/wiresmith"),
            ])
            .args([
                "-v",
                &format!("{}:/etc/systemd/network", dir.to_string_lossy()),
            ])
            .args(["--tz", "UTC"])
            .arg("wiresmith-testing")
            .stdout(Stdio::null())
            .spawn()
            .expect("Couldn't run systemd in podman");

        wait_for_systemd(&container_name)
            .await
            .expect("Error while waiting for systemd container");

        // Lastly, start wiresmith itself.
        Command::new("podman")
            .arg("exec")
            .arg(&container_name)
            .arg("wiresmith")
            .args([
                "--consul-address",
                &format!("http://consul-{consul_port}:{consul_port}"),
            ])
            .args(["--network", network])
            .args(["--endpoint-address", &container_name])
            .args(args)
            // To diagnose issues, it's sometimes helpful to comment out the following line so that
            // we can see log output from the wiresmith instances inside the containers.
            .stdout(Stdio::null())
            .spawn()
            .expect("Couldn't run systemd in podman");

        Self { container_name }
    }
}

impl Drop for WiresmithContainer {
    fn drop(&mut self) {
        // We can't use async here as drop isn't async so we just run this command blocking.
        use std::process::Command;

        // Using podman, stop all containers with the same testport label.
        Command::new("podman")
            .arg("kill")
            .arg(&self.container_name)
            .output()
            .unwrap_or_else(|_| panic!("Error trying to run podman kill {}", self.container_name));
    }
}

/// Wait a few seconds for systemd to boot
async fn wait_for_systemd(container_name: &str) -> Result<()> {
    let start_time = Instant::now();

    loop {
        let output = Command::new("podman")
            .arg("exec")
            .arg(container_name)
            .arg("systemctl")
            .arg("is-system-running")
            .output()
            .await?;
        // "degraded" is good enough for us, it just means that at least one unit has failed to
        // start but we don't usually care about that.
        if output.stdout.starts_with(b"degraded") || output.stdout.starts_with(b"running") {
            println!(
                "Test container '{container_name}' took {:?} to start",
                start_time.elapsed()
            );
            return Ok(());
        }

        sleep(Duration::from_millis(100)).await;

        if start_time.elapsed().as_secs() > 10 {
            dbg!(output);
            panic!("Timeout waiting for systemd container {container_name}",);
        }
    }
}
