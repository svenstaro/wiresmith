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
            .arg("kill")
            .arg(&container_name)
            .output()
            .unwrap_or_else(|_| panic!("Error trying to run podman kill {}", container_name));

        // Remove test container network.
        Command::new("podman")
            .arg("network")
            .arg("rm")
            // Remove wiresmith container as well, if still present.
            .arg("-f")
            .arg(format!("wiresmith-{}", self.http_port))
            .stdout(Stdio::null())
            .spawn()
            .expect("Couldn't remove test container network");
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
    let start_time = Instant::now();

    let http_port = port();

    // Create a dedicated container network for each test using
    // this fixture.
    Command::new("podman")
        .arg("network")
        .arg("create")
        .arg(format!("wiresmith-{http_port}"))
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't create test container network");

    // Wait for podman to setup the network.
    sleep(Duration::from_millis(100)).await;

    Command::new("podman")
        .arg("run")
        .args(["--name", &format!("consul-{http_port}")])
        .arg("--replace")
        .arg("--rm")
        .args(["--label", "testcontainer"])
        .args(["--label", &format!("testport={http_port}")])
        .args(["--network", &format!("wiresmith-{http_port}")])
        .args(["-p", &format!("{http_port}:{http_port}")])
        .arg("docker.io/hashicorp/consul")
        .arg("agent")
        .arg("-dev")
        .args(["-bind", "{{ GetInterfaceIP \"eth0\" }}"])
        .args(["-client", "0.0.0.0"])
        .args(["-http-port", &http_port.to_string()])
        .args(["-grpc-port", "0"])
        .args(["-grpc-tls-port", "0"])
        .args(["-dns-port", "0"])
        .args(["-serf-lan-port", &port().to_string()])
        .args(["-server-port", &port().to_string()])
        .args(args.clone())
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't run Consul binary");

    let consul = ConsulContainer::new(http_port);
    wait_for_api(&consul)
        .await
        .expect("Error while waiting for Consul API");
    println!(
        "Started Consul after {:?} on HTTP port {http_port}",
        start_time.elapsed()
    );
    consul
}

/// Run a federated Consul cluster with two datacenters
///
/// Start with a free port and some optional arguments then wait for a while for the server setup
/// to complete.
#[fixture]
pub async fn federated_consul_cluster<I>(
    #[default(&[] as &[&str])] args: I,
) -> (ConsulContainer, ConsulContainer)
where
    I: IntoIterator + Clone,
    I::Item: AsRef<std::ffi::OsStr>,
{
    let start_time = Instant::now();

    let http_port_dc1 = port();

    // Create a dedicated container network for each test using
    // this fixture.
    Command::new("podman")
        .arg("network")
        .arg("create")
        .arg(format!("wiresmith-{http_port_dc1}"))
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't create test container network");

    // Wait for podman to setup the network.
    sleep(Duration::from_millis(100)).await;

    Command::new("podman")
        .arg("run")
        .args(["--name", &format!("consul-{http_port_dc1}")])
        .arg("--replace")
        .arg("--rm")
        .args(["--label", "testcontainer"])
        .args(["--label", &format!("testport={http_port_dc1}")])
        .args(["--network", &format!("wiresmith-{http_port_dc1}")])
        .args(["-p", &format!("{http_port_dc1}:{http_port_dc1}")])
        .arg("docker.io/hashicorp/consul")
        .arg("agent")
        .arg("-dev")
        .args(["-datacenter", "dc1"])
        .args(["-bind", "{{ GetInterfaceIP \"eth0\" }}"])
        .args(["-client", "0.0.0.0"])
        .args(["-http-port", &http_port_dc1.to_string()])
        .args(["-grpc-port", "0"])
        .args(["-grpc-tls-port", "0"])
        .args(["-dns-port", "0"])
        .args(["-serf-lan-port", &port().to_string()])
        .args(["-server-port", &port().to_string()])
        .args(args.clone())
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't run Consul binary");

    let consul_dc1 = ConsulContainer::new(http_port_dc1);
    wait_for_api(&consul_dc1)
        .await
        .expect("Error while waiting for Consul API");
    println!(
        "Started Consul in dc1 after {:?} on HTTP port {http_port_dc1}",
        start_time.elapsed()
    );

    let http_port_dc2 = port();
    Command::new("podman")
        .arg("run")
        .args(["--name", &format!("consul-{http_port_dc2}")])
        .arg("--replace")
        .arg("--rm")
        .args(["--label", "testcontainer"])
        .args(["--label", &format!("testport={http_port_dc2}")])
        .args(["--network", &format!("wiresmith-{http_port_dc1}")])
        .args(["-p", &format!("{http_port_dc2}:{http_port_dc2}")])
        .arg("docker.io/hashicorp/consul")
        .arg("agent")
        .arg("-dev")
        .args(["--datacenter", "dc2"])
        // This is the part that makes this a federated cluster.
        .args(["-retry-join-wan", &format!("consul-{http_port_dc1}")])
        .args(["-bind", "{{ GetInterfaceIP \"eth0\" }}"])
        .args(["-client", "0.0.0.0"])
        .args(["-http-port", &http_port_dc2.to_string()])
        .args(["-grpc-port", "0"])
        .args(["-grpc-tls-port", "0"])
        .args(["-dns-port", "0"])
        .args(["-serf-lan-port", &port().to_string()])
        .args(["-server-port", &port().to_string()])
        .args(args.clone())
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't run Consul binary");

    let consul_dc2 = ConsulContainer::new(http_port_dc2);
    wait_for_api(&consul_dc1)
        .await
        .expect("Error while waiting for Consul API");
    println!(
        "Started Consul in dc2 after {:?} on HTTP port {http_port_dc2}",
        start_time.elapsed()
    );

    (consul_dc1, consul_dc2)
}
