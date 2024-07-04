use std::{collections::HashSet, future::Future, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use futures::future::join_all;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    StatusCode, Url,
};
use serde::{Deserialize, Serialize};
use tokio::{
    task::{JoinError, JoinHandle},
    time::interval,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};
use uuid::Uuid;
use wireguard_keys::Pubkey;

use crate::{wireguard::WgPeer, CONSUL_TTL};

/// Allows for gracefully telling a background task to shut down and to then join it.
#[must_use]
pub struct TaskCancellator {
    join_handle: JoinHandle<()>,
    token: CancellationToken,
}

impl TaskCancellator {
    #[tracing::instrument(skip(self))]
    pub async fn cancel(self) -> Result<(), JoinError> {
        self.token.cancel();
        self.join_handle.await
    }
}

#[derive(Clone, Debug)]
pub struct ConsulClient {
    pub http_client: reqwest::Client,
    api_base_url: Url,
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
        })
    }

    /// # Read all peer configs
    ///
    /// This reads the WireGuard peer configs from all available Consul DCs and merges the sets
    /// together.
    #[tracing::instrument(skip(self))]
    pub async fn get_peers(&self) -> Result<HashSet<WgPeer>> {
        let dcs = self
            .http_client
            .get(self.api_base_url.join("v1/catalog/datacenters")?)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<String>>()
            .await?;

        let mut peers = HashSet::new();
        for dc_peers in join_all(dcs.iter().map(|dc| self.get_peers_for_dc(dc))).await {
            let dc_peers = dc_peers?;
            peers.extend(dc_peers);
        }

        Ok(peers)
    }

    /// # Read peers for a single DC
    ///
    /// This will read the all of the WireGuard peers from a given Consul DC. This should only be
    /// called by [`Self::get_peers`].
    #[tracing::instrument(skip(self))]
    async fn get_peers_for_dc(&self, dc: &str) -> Result<HashSet<WgPeer>> {
        // When the Consul server which is the Raft leader is restarted all KV reads by default
        // return 500 errors until a new Raft leader is elected. For our usecase it's fine if the
        // read value is a bit stale though, so prevent spurious errors by always performing stale
        // reads.
        let mut peers_url = self.kv_api_base_url.join("peers/")?;
        peers_url
            .query_pairs_mut()
            .append_pair("recurse", "true")
            .append_pair("dc", dc)
            .append_pair("stale", "1");

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
        parent_token: CancellationToken,
    ) -> Result<ConsulSession> {
        let url = self.api_base_url.join("v1/session/create")?;

        let res = self
            .http_client
            .put(url)
            .json(&CreateSession {
                name: format!("wiresmith-{}", public_key.to_base64_urlsafe()),
                behavior: SessionInvalidationBehavior::Delete,
                ttl: CONSUL_TTL.try_into()?,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<CreateSessionResponse>()
            .await?;

        let session_token = CancellationToken::new();
        let join_handle = tokio::spawn(
            session_handler(self.clone(), session_token.clone(), parent_token, res.id)
                .context("failed to create Consul session handler")?,
        );

        trace!("Created Consul session with id {}", res.id);

        Ok(ConsulSession {
            client: self.clone(),
            id: res.id,
            cancellator: TaskCancellator {
                join_handle,
                token: session_token,
            },
        })
    }
}

/// # Create a background task maintaining a Consul session
///
/// This function returns a future which will renew the given Consul session according to the
/// hardcoded session TTL (currently 15 seconds). The returned future is expected to be spawned as
/// a Tokio task.
///
/// The future will continue maintaining the session until either the `session_token`
/// [`CancellationToken`] is cancelled, in which case we will explicitly invalidate the session, or
/// until the session is invalidated by a third party, in which case the `parent_token` will be
/// cancelled to let the rest of the application know that the session is no longer valid.
fn session_handler(
    client: ConsulClient,
    session_token: CancellationToken,
    parent_token: CancellationToken,
    session_id: Uuid,
) -> Result<impl Future<Output = ()> + Send> {
    // We construct the URLs first so we can return an error before the task is even spawned.
    let session_id = session_id.to_string();
    let renewal_url = client
        .api_base_url
        .join("v1/session/renew/")
        .context("failed to build session renewal URL")?
        .join(&session_id)
        .context("failed to build session renewal URL")?;
    let destroy_url = client
        .api_base_url
        .join("v1/session/destroy/")
        .context("failed to build session destroy URL")?
        .join(&session_id)
        .context("failed to build session destroy URL")?;

    Ok(async move {
        // Renew the session at 2 times the TTL.
        let mut interval = interval(CONSUL_TTL / 2);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Wait for either cancellation or an interval tick to have passed.
            tokio::select! {
                _ = session_token.cancelled() => {
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
                error!("Renewing Consul session failed, aborting: {err:?}");
                parent_token.cancel();
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
            warn!("Destroying Consul session failed: {err:?}");
        }
    })
}

/// # An active Consul session
///
/// Sessions are a mechanism provided by Consul to allow for implementing distributed locks and
/// TTL'd keys in the Consul key/value store.
///
/// When a session is created you specify a TTL that the session is valid for unless it's renewed.
/// The session ID can then be used to acquire locks in the key/value store that belongs to that
/// session. If the session is either explicitly invalidated or is not renewed within the given TTL
/// the lock is invalidated and depending on the how the session was created the keys for which
/// locks are held are deleted.
///
/// [`ConsulClient::create_session`] creates a session with a given TTL, using the deletion
/// invalidation behavior, and accepts a parent [`CancellationToken`] which will be cancelled if
/// the session is invalidated.
///
/// This struct represents a Consul session which is actively being renewed by a spawned background
/// task. If renewing the session fails for any reason (either because the session was invalidated
/// by some other mechanism or because we can no longer access the Consul API) the background task
/// cancels the [`CancellationToken`] that it was passed in to prevent any further work from being
/// done with the session no longer being valid.
pub struct ConsulSession {
    client: ConsulClient,
    id: Uuid,
    cancellator: TaskCancellator,
}

impl ConsulSession {
    /// # Cancel the session
    ///
    /// This will cause the background task maintaining the session to exit its loop and invalidate
    /// the session, thus invalidating all locks belonging to it.
    #[tracing::instrument(skip(self))]
    pub async fn cancel(self) -> Result<(), JoinError> {
        // The background task being cancelled here is defined in `session_handler`.
        self.cancellator.cancel().await
    }

    /// # Add own WireGuard peer config
    ///
    /// This locks the key with this session's ID to ensure that the key is deleted if the session
    /// is invalidated.
    ///
    /// A background task is spawned that ensures that the key continues existing. If it cannot be
    /// fetched the parent [`CancellationToken`] is cancelled.
    #[tracing::instrument(skip(self, wgpeer))]
    pub async fn put_config(
        &self,
        wgpeer: &WgPeer,
        parent_token: CancellationToken,
    ) -> Result<TaskCancellator> {
        let peer_url = self
            .client
            .kv_api_base_url
            .join("peers/")?
            .join(&wgpeer.public_key.to_base64_urlsafe())?;

        let mut put_url = peer_url.clone();
        put_url
            .query_pairs_mut()
            .append_pair("acquire", &self.id.to_string());

        // KV PUT requests return a boolean saying whether the upsert attempt was successful. If
        // another session already holds the lock this request will fail.
        let got_lock = self
            .client
            .http_client
            .put(put_url)
            .json(wgpeer)
            .send()
            .await?
            .error_for_status()
            .context("failed to put node config into Consul")?
            .json::<bool>()
            .await
            .context("Failed to parse Consul KV put response")?;
        if !got_lock {
            bail!("Did not get Consul lock for node config");
        }

        info!("Wrote node config into Consul");

        let client = self.client.clone();
        let config_token = CancellationToken::new();
        let join_handle = tokio::spawn(config_handler(
            client,
            self.id,
            peer_url,
            config_token.clone(),
            parent_token,
        ));

        Ok(TaskCancellator {
            join_handle,
            token: config_token,
        })
    }
}

/// # Background task ensuring own config key exists
///
/// Makes sure that the config key we created still exists by long polling. If getting it fails for
/// whatever reason we trigger the parent [`CancellationToken`] to cancel since we can no longer be
/// sure that we have a valid locked session. If the key exists but locked by the wrong session we
/// also trigger a cancellation.
async fn config_handler(
    client: ConsulClient,
    session_id: Uuid,
    peer_url: Url,
    config_token: CancellationToken,
    parent_token: CancellationToken,
) {
    // Consul documents that stale results are generally consistent within 50 ms, so let's sleep
    // for that amount of time before we start checking to try to prevent spurious errors returned.
    //
    // We still perform 5 retries at 1 second intervals since the maximum delay is theoretically
    // unbounded.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut failed_fetches = 0;
    let mut index = None;

    let mut interval = interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Wait for an interval tick to have passed to prevent us from hammering the server with
        // requests.
        tokio::select! {
            _ = config_token.cancelled() => {
                trace!("Consul config handler was cancelled");
                break;
            },
            _ = interval.tick() => {},
        };

        let res = tokio::select! {
            _ = config_token.cancelled() => {
                trace!("Consul config handler was cancelled");
                break;
            },
            res = ensure_config_exists(&client, peer_url.clone(), &mut index) => res,
        };

        match res {
            Ok(owner_id) => {
                // Reset the failure counter on any successful response.
                failed_fetches = 0;

                // Check that the key is actually locked by us and not some other session.
                // Otherwise it means that something else invalidated the lock and we need to
                // cancel the parent task.
                if owner_id != session_id {
                    error!(
                        "Consul key is locked by {owner_id}, expected it to be us ({session_id})"
                    );
                    parent_token.cancel();
                    break;
                }
            }
            Err(err) => {
                // Allow up to 5 API failures before we cancel the parent task and exit to deal
                // with spurious Consul API error when e.g. the cluster leader goes down.
                failed_fetches += 1;
                if failed_fetches >= 5 {
                    error!("Failed to fetch own node config {failed_fetches} times, cancelling");
                    parent_token.cancel();
                    break;
                }

                error!("Could not get own node config from Consul ({failed_fetches} failed fetches): {err:?}");
                continue;
            }
        };

        trace!("Successfully fetched own node config from Consul");
    }
}

/// # Consul KV store read response
///
/// The Consul KV store Read Key API returns a list of objects corresponding to this struct. This
/// is currently only used by `ensure_config_exists` to ensure that the key is locked by the
/// expected session ID.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ReadKeyResponse {
    session: Option<Uuid>,
}

/// # Ensure that a given WireGuard peer config exists
///
/// The `index` parameter is used to store the Consul index which is used to allow for blocking
/// reads. Consul will only respond to the request if the current index is different from the
/// passed in one, or the request timed out. Users are expected to pass in a mutable reference to a
/// value that defaults to `None`, and pass in references to the same value whenever the function
/// is called with the same URL.
///
/// Returns the session ID that holds the config locked.
async fn ensure_config_exists(
    client: &ConsulClient,
    peer_url: Url,
    index: &mut Option<String>,
) -> Result<Uuid> {
    let query: &[_] = if let Some(index) = index {
        &[("index", index)]
    } else {
        &[]
    };

    let res = client
        .http_client
        .get(peer_url)
        .query(query)
        .send()
        .await?
        .error_for_status()?;

    if let Some(new_index) = res.headers().get("X-Consul-Index") {
        let new_index = new_index
            .to_str()
            .context("Failed to convert new Consul index to String")?
            .to_string();
        index.replace(new_index);
    };

    let res = res
        .json::<Vec<ReadKeyResponse>>()
        .await
        .context("Failed to parse KV response")?;

    res.first()
        .context("Consul unexpectedly returned an empty array")?
        .session
        .ok_or_else(|| anyhow!("Key was not locked by any session"))
}
