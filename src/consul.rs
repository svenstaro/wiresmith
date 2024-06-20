use std::{collections::HashSet, future::Future, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    StatusCode, Url,
};
use serde::{Deserialize, Serialize};
use tokio::{task::JoinHandle, time::interval};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};
use uuid::Uuid;
use wireguard_keys::Pubkey;

use crate::wireguard::WgPeer;

#[derive(Clone, Debug)]
pub struct ConsulClient {
    pub http_client: reqwest::Client,
    api_base_url: Url,
    pub kv_api_base_url: Url,
    pub datacenter: Option<String>,
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

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum SessionInvalidationBehavior {
    /// Delete the keys corresponding to the locks held by this session when the session is
    /// invalidated.
    Delete,
}

#[derive(Copy, Clone)]
enum SessionDuration {
    Seconds(u32),
}

impl TryFrom<Duration> for SessionDuration {
    type Error = anyhow::Error;

    fn try_from(value: Duration) -> Result<Self> {
        // Consul only supports durations of up to 86400 seconds.
        let secs = value.as_secs();
        if secs > 86400 {
            bail!("Tried to convert a duration longer than 24 hours into SessionDuration");
        }
        Ok(SessionDuration::Seconds(secs as u32))
    }
}

impl Serialize for SessionDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&match self {
            Self::Seconds(s) => format!("{s}s"),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CreateSession {
    name: String,
    behavior: SessionInvalidationBehavior,
    /// How long the session will survive without being renewed.
    #[serde(rename = "TTL")]
    ttl: SessionDuration,
}

#[derive(Deserialize)]
struct CreateSessionResponse {
    #[serde(rename = "ID")]
    id: Uuid,
}

impl ConsulClient {
    pub fn new(
        consul_address: Url,
        consul_prefix: &str,
        consul_token: Option<&str>,
        consul_datacenter: Option<String>,
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
                HeaderValue::from_str(secret_token)?,
            );
            client_builder.default_headers(headers)
        } else {
            client_builder
        };

        let client = client_builder.build()?;

        Ok(ConsulClient {
            http_client: client,
            api_base_url: consul_address,
            kv_api_base_url,
            datacenter: consul_datacenter,
        })
    }

    /// Read out all configs.
    #[tracing::instrument(skip(self))]
    pub async fn get_peers(&self) -> Result<HashSet<WgPeer>> {
        let mut peers_url = self.kv_api_base_url.join("peers/")?;
        peers_url.query_pairs_mut().append_pair("recurse", "true");

        if let Some(dc) = &self.datacenter {
            peers_url.query_pairs_mut().append_pair("dc", dc);
        }

        let resp = self
            .http_client
            .get(peers_url)
            .send()
            .await?
            .error_for_status();
        match resp {
            Ok(resp) => {
                let kv_get: HashSet<ConsulKvGet> = resp.json().await?;
                let wgpeers: HashSet<_> = kv_get
                    .into_iter()
                    .map(|x| {
                        let decoded = &BASE64_STANDARD
                            .decode(x.value)
                            .expect("Can't decode base64");
                        serde_json::from_slice(decoded)
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
        let mut peer_url = self
            .kv_api_base_url
            .join("peers/")?
            .join(&wgpeer.public_key.to_base64_urlsafe())?;

        if let Some(dc) = &self.datacenter {
            peer_url.query_pairs_mut().append_pair("dc", dc);
        }

        self.http_client
            .put(peer_url)
            .json(&wgpeer)
            .send()
            .await?
            .error_for_status()?;
        info!("Wrote node config into Consul");
        Ok(())
    }

    /// Remove a peer config from Consul
    #[tracing::instrument(skip(self, public_key))]
    pub async fn delete_config(&self, public_key: Pubkey) -> Result<()> {
        let mut peer_url = self
            .kv_api_base_url
            .join("peers/")?
            .join(&public_key.to_base64_urlsafe())?;

        if let Some(dc) = &self.datacenter {
            peer_url.query_pairs_mut().append_pair("dc", dc);
        }

        self.http_client
            .delete(peer_url)
            .send()
            .await?
            .error_for_status()?;
        info!(
            "Deleted peer {} config from Consul",
            public_key.to_base64_urlsafe()
        );
        Ok(())
    }

    /// # Create a Consul session
    ///
    /// This starts a background task which renews the session based on the given session TTL. If
    /// renewing the session fails, the passed in cancellation token is cancelled. On cancellation
    /// the keys that locks are held for are deleted.
    ///
    /// See [`ConsulSession`] for more information.
    pub async fn create_session(
        &self,
        public_key: Pubkey,
        ttl: Duration,
        token: CancellationToken,
    ) -> Result<ConsulSession> {
        let url = self.api_base_url.join("v1/session/create")?;

        let res = self
            .http_client
            .put(url)
            .json(&CreateSession {
                name: format!("wiresmith-{}", public_key.to_base64_urlsafe()),
                behavior: SessionInvalidationBehavior::Delete,
                ttl: ttl.try_into()?,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<CreateSessionResponse>()
            .await?;

        let join_handle = tokio::spawn(
            session_handler(self.clone(), token, res.id, ttl)
                .context("failed to create Consul session handler")?,
        );

        Ok(ConsulSession { join_handle })
    }
}


pub struct ConsulSession {
    pub join_handle: JoinHandle<()>,
}

/// # Create a background task maintaining a Consul session
///
/// This function returns a future which will renew the given Consul session according to the given
/// session TTL. The returned future is expected to be spawned as a Tokio task.
///
/// The future will continue maintaining the session until either the `session_token`
/// [`CancellationToken`] is cancelled, in which case we will explicitly invalidate the session, or
/// until the session is invalidated by a third party, in which case the `parent_token` will be
/// cancelled to let the rest of the application know that the session is no longer valid.
fn session_handler(
    client: ConsulClient,
    token: CancellationToken,
    id: Uuid,
    ttl: Duration,
) -> Result<impl Future<Output = ()> + Send> {
    // We construct the URLs first so we can return an error before the task is even spawned.
    let id = id.to_string();
    let renewal_url = client
        .api_base_url
        .join("v1/session/renew/")
        .context("failed to build session renewal URL")?
        .join(&id)
        .context("failed to build session renewal URL")?;
    let destroy_url = client
        .api_base_url
        .join("v1/session/destroy/")
        .context("failed to build session destroy URL")?
        .join(&id)
        .context("failed to build session destroy URL")?;

    Ok(async move {
        // Renew the session at 2 times the TTL.
        let mut interval = interval(ttl / 2);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Wait for either cancellation or an interval tick to have passed.
            tokio::select! {
                _ = token.cancelled() => {
                    trace!("Consul session handler was cancelled");
                    break;
                },
                _ = interval.tick() => {},
            };

            trace!("Renewing Consul session");
            let res = client
                .http_client
                .put(renewal_url.clone())
                .send()
                .await
                .and_then(|res| res.error_for_status());
            if let Err(err) = res {
                error!("Renewing Consul session failed, aborting: {err}");
                token.cancel();
                return;
            }
        }

        trace!("Destroying Consul session");
        let res = client
            .http_client
            .put(destroy_url)
            .send()
            .await
            .and_then(|res| res.error_for_status());
        if let Err(err) = res {
            warn!("Destraying Consul session failed: {err}");
        }
    })
}
