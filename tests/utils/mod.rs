use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use assert_cmd::prelude::*;

/// Launch an instance of wiresmith
///
/// As it's going to run indefinitely, you're going to have to kill it yourself.
pub fn spawn_wiresmith(
    network: &str,
    endpoint_address: &str,
    consul_port: u16,
    dir: &Path,
) -> Child {
    Command::cargo_bin("wiresmith")
        .expect("Couldn't find wiresmith binary")
        .arg("--consul-address")
        .arg(format!("http://localhost:{}", consul_port))
        .arg("--network")
        .arg(network)
        .arg("--endpoint-address")
        .arg(endpoint_address)
        .arg("--networkd-dir")
        .arg(dir)
        .stdout(Stdio::null())
        .spawn()
        .expect("Couldn't run wiresmith binary")
}

/// Wait a max of 3s for the files to become available
pub fn wait_for_files(files: Vec<&Path>) {
    let start_wait = Instant::now();

    while !files.iter().all(|x| x.exists()) {
        sleep(Duration::from_millis(100));

        if start_wait.elapsed().as_secs() > 3 {
            panic!("Timeout waiting {files:?} to exist");
        }
    }
}
