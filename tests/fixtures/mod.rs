use std::{
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use assert_fs::fixture::TempDir;
use port_check::free_local_port;
use rstest::fixture;

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

/// Wait a max of 3s for the port to become available
fn wait_for_port(port: u16) {
    let start_wait = Instant::now();

    while !port_check::is_port_reachable(format!("localhost:{port}")) {
        sleep(Duration::from_millis(100));

        if start_wait.elapsed().as_secs() > 3 {
            panic!("Timeout waiting for port {port}");
        }
    }
}

pub struct Consul {
    pub http_port: u16,
    pub client: ConsulClient,
    child: Child,
}

impl Consul {
    fn new(port: u16, child: Child) -> Self {
        let client = ConsulClient::new(
            format!("http://localhost:{port}").parse().unwrap(),
            "wiresmith",
            None,
        )
        .unwrap();
        Self {
            http_port: port,
            client,
            child,
        }
    }
}
impl Drop for Consul {
    fn drop(&mut self) {
        self.child.kill().expect("Couldn't kill Consul agent");
        self.child.wait().unwrap();
    }
}

/// Run Consul in dev mode
///
/// Start with a free port and some optional arguments then wait for a while for the server setup
/// to complete.
#[fixture]
pub fn consul<I>(#[default(&[] as &[&str])] args: I) -> Consul
where
    I: IntoIterator + Clone,
    I::Item: AsRef<std::ffi::OsStr>,
{
    let http_port = port();
    let serf_lan_port = port();
    let server_port = port();
    let child = Command::new("consul")
        .arg("agent")
        .arg("-dev")
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

    wait_for_port(http_port);
    println!("Started Consul with HTTP port {http_port}");
    Consul::new(http_port, child)
}
