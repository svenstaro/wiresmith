use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use assert_fs::fixture::TempDir;
use port_check::free_local_port;
use rstest::fixture;

use tokio::time::sleep;
use wiresmith::consul::ConsulClient;

/// Get a free port.
#[fixture]
pub fn port() -> u16 {
    free_local_port().expect("Couldn't find a free local port")
}

/// Test fixture which creates a temporary directory
#[fixture]
pub fn tmpdir() -> TempDir {
    assert_fs::TempDir::new().expect("Couldn't create a temp dir for tests")
}

/// Wait for a few seconds for the port to become available
async fn wait_for_api(consul: &ConsulContainer) -> Result<()> {
    let start_wait = Instant::now();

    loop {
        let req = consul
            .client
            .http_client
            .get(consul.client.kv_api_base_url.join("/v1/status/leader")?);

        if let Ok(resp) = req.send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }

        sleep(Duration::from_millis(100)).await;

        // It could take a few seconds on the initial pull or on CI.
        if start_wait.elapsed().as_secs() > 15 {
            panic!(
                "Timeout waiting for Consul API at {}",
                consul.client.kv_api_base_url
            );
        }
    }
}

pub struct ConsulContainer {
    pub http_port: u16,
    pub client: ConsulClient,
}

impl ConsulContainer {
    fn new(port: u16) -> Self {
        let client = ConsulClient::new(
            format!("http://localhost:{port}").parse().unwrap(),
            "wiresmith",
            None,
        )
        .unwrap();
        Self {
            http_port: port,
            client,
        }
    }
}

impl Drop for ConsulContainer {
    fn drop(&mut self) {
        let container_name = format!("consul-{}", self.http_port);
        // Using podman, stop all containers with the same testport label.
        Command::new("podman")
            .arg("stop")
            .arg(&container_name)
            .output()
            .unwrap_or_else(|_| panic!("Error trying to run podman stop {}", container_name));
    }
}

/// Run Consul in dev mode
///
/// Start with a free port and some optional arguments then wait for a while for the server setup
/// to complete.
#[fixture]
pub async fn consul<I>(#[default(&[] as &[&str])] args: I) -> ConsulContainer
where
    I: IntoIterator + Clone,
    I::Item: AsRef<std::ffi::OsStr>,
{
    let http_port = port();
    let serf_lan_port = port();
    let server_port = port();
    Command::new("podman")
        .arg("run")
        .arg("--name")
        .arg(format!("consul-{http_port}"))
        .arg("--replace")
        .arg("--rm")
        .arg("--label")
        .arg("testcontainer")
        .arg("-p")
        .arg(format!("{http_port}:{http_port}"))
        .arg("-l")
        .arg(format!("testport={http_port}"))
        .arg("docker.io/hashicorp/consul")
        .arg("agent")
        .arg("-dev")
        .arg("-client")
        .arg("0.0.0.0")
        .arg("-http-port")
        .arg(http_port.to_string())
        .arg("-grpc-port")
        .arg("0")
        .arg("-grpc-tls-port")
        .arg("0")
        .arg("-dns-port")
        .arg("0")
        .arg("-serf-lan-port")
        .arg(serf_lan_port.to_string())
        .arg("-serf-wan-port")
        .arg("0")
        .arg("-server-port")
        .arg(server_port.to_string())
        .args(args.clone())
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't run Consul binary");

    let consul = ConsulContainer::new(http_port);
    wait_for_api(&consul)
        .await
        .expect("Error while waiting for Consul API");
    println!("Started Consul with HTTP port {http_port}");
    consul
}
