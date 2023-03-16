use std::collections::HashSet;

use anyhow::{anyhow, Result};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    StatusCode, Url,
};
use serde::Deserialize;
use tracing::info;

use crate::wireguard::WgPeer;

#[derive(Debug)]
pub struct ConsulClient {
    pub http_client: reqwest::Client,
    pub kv_api_base_url: Url,
}

#[derive(Debug, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConsulKvGet {
    pub create_index: u64,
    pub flags: u64,
    pub key: String,
    pub lock_index: u64,
    pub modify_index: u64,
    pub value: String,
}

impl ConsulClient {
    pub fn new(
        consul_address: Url,
        consul_prefix: &str,
        consul_token: Option<&str>,
    ) -> Result<ConsulClient> {
        // Make sure the consul prefix ends with a /.
        let consul_prefix = if consul_prefix.ends_with('/') {
            consul_prefix.to_string()
        } else {
            format!("{}/", consul_prefix)
        };
        let kv_api_base_url = consul_address
            .join("v1/")?
            .join("kv/")?
            .join(&consul_prefix)?;

        let client_builder = reqwest::Client::builder();
        let client_builder = if let Some(secret_token) = consul_token {
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_static("X-Consul-Token"),
                HeaderValue::from_str(&secret_token)?,
            );
            client_builder.default_headers(headers)
        } else {
            client_builder
        };

        let client = client_builder.build()?;

        Ok(ConsulClient {
            http_client: client,
            kv_api_base_url,
        })
    }

    /// Read out all configs.
    #[tracing::instrument(skip(self))]
    pub async fn get_peers(&self) -> Result<HashSet<WgPeer>> {
        let resp = self
            .http_client
            .get(self.kv_api_base_url.join("?recurse=true")?)
            .send()
            .await?
            .error_for_status();
        match resp {
            Ok(resp) => {
                let kv_get: HashSet<ConsulKvGet> = resp.json().await?;
                let wgpeers: HashSet<_> = kv_get
                    .into_iter()
                    .map(|x| {
                        serde_json::from_slice(
                            &BASE64_STANDARD
                                .decode(x.value)
                                .expect("Can't decode base64"),
                        )
                        .expect("Can't interpret JSON out of decoded base64")
                    })
                    .collect();
                Ok(wgpeers)
            }
            Err(resp) => {
                if resp.status() == Some(StatusCode::NOT_FOUND) {
                    return Ok(HashSet::new());
                }
                Err(anyhow!(resp))
            }
        }
    }

    /// Add own config.
    #[tracing::instrument(skip(self, wgpeer))]
    pub async fn put_config(&self, wgpeer: WgPeer) -> Result<()> {
        self.http_client
            .put(
                self.kv_api_base_url
                    .join(&wgpeer.public_key.to_base64_urlsafe())?,
            )
            .json(&wgpeer)
            .send()
            .await?
            .error_for_status()?;
        info!("Wrote node config into Consul");
        Ok(())
    }
}
