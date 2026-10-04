//! the veilid node the core runs on, and the few record operations every
//! part of it shares.

use crate::protocol::{
    decode_slot, ShareKeys, Slot, FIRST_SLOT_SUBKEY, PEER_SLOTS, RECORD_SUBKEYS,
};
use crate::rt::{now_ms, timeout, Flag};
use crate::{Error, Result};
use futures_util::future::join_all;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};
use veilid_core::{
    api_startup, BareKeyPair, BarePublicKey, BareSecretKey, BareSharedSecret, DHTSchema, KeyPair,
    PublicKey, RecordKey, RouteId, RoutingContext, SharedSecret, UpdateCallback, VeilidAPI,
    VeilidAppCall, VeilidConfig, VeilidUpdate, CRYPTO_KIND_VLD0,
};

// most nodes are ready within ten seconds. about one in seven, in a
// browser, is attached as soon, with peers, and is still not ready ten
// minutes later; detaching and attaching it again changes nothing. so a
// node that is not ready in this long is shut down and started over, this
// many times before whoever started it is told.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(40);
const START_ATTEMPTS: u32 = 5;
const UNREACHABLE: &str = "could not reach the veilid network in time";

pub struct Node {
    pub api: VeilidAPI,
    pub context: RoutingContext,
    pub calls: flume::Receiver<VeilidAppCall>,
    pub dead_routes: flume::Receiver<Vec<RouteId>>,
}

pub fn record_schema() -> Result<DHTSchema> {
    Ok(DHTSchema::dflt(RECORD_SUBKEYS)?)
}

/// the keypair every peer of a share writes its record with.
pub fn owner_keypair(keys: &ShareKeys) -> KeyPair {
    let public = BarePublicKey::new(&keys.owner_public);
    let secret = BareSecretKey::new(&keys.owner_secret);
    KeyPair::new(CRYPTO_KIND_VLD0, BareKeyPair::new(public, secret))
}

/// the record a share lives at, which is also its token.
pub async fn record_key(api: &VeilidAPI, keys: &ShareKeys) -> Result<RecordKey> {
    let owner = PublicKey::new(CRYPTO_KIND_VLD0, BarePublicKey::new(&keys.owner_public));
    let encryption = BareSharedSecret::new(&keys.encryption_key);
    let encryption = SharedSecret::new(CRYPTO_KIND_VLD0, encryption);
    Ok(api
        .get_dht_record_key(record_schema()?, owner, Some(encryption))
        .await?)
}

pub fn slot_subkeys() -> std::ops::Range<u32> {
    FIRST_SLOT_SUBKEY..FIRST_SLOT_SUBKEY + u32::from(PEER_SLOTS)
}

/// one peer slot as the network has it now: the peer it names, None when it
/// names none, or why it could not be read. a slot nobody has ever written
/// takes veilid its whole lookup timeout, ten seconds, to give up on, and so
/// does asking the record which slots are written, so a caller in a hurry
/// reads the slots side by side and uses each as it comes.
pub async fn read_slot(
    context: &RoutingContext,
    key: &RecordKey,
    subkey: u32,
) -> Result<Option<Slot>> {
    let began = now_ms();
    let value = context.get_dht_value(key.clone(), subkey, true).await;
    let slot = value.map(|value| value.and_then(|value| decode_slot(value.data())));
    let state = match &slot {
        Ok(Some(_)) => "a peer",
        Ok(None) => "nobody",
        Err(_) => "unreadable",
    };
    let took = now_ms().saturating_sub(began);
    debug!("slot {subkey} read in {took}ms: {state}");
    slot.map_err(Error::from)
}

/// every peer slot as the network has it now, by subkey.
pub async fn read_slots(
    context: &RoutingContext,
    key: &RecordKey,
) -> Vec<(u32, Result<Option<Slot>>)> {
    let read = |subkey| async move { (subkey, read_slot(context, key, subkey).await) };
    join_all(slot_subkeys().map(read)).await
}

/// `config` for a node that starts with none of what the last one kept: a
/// new identity, and nothing remembered of the network.
fn afresh(mut config: VeilidConfig) -> VeilidConfig {
    config.table_store.delete = true;
    config.protected_store.delete = true;
    config.block_store.delete = true;
    config
}

impl Node {
    /// start a node, attach it, and wait until routes and the dht are
    /// usable. where the node keeps its storage is the embedder's to say,
    /// in `config`.
    pub async fn start(config: VeilidConfig) -> Result<Node> {
        info!("connecting to veilid");
        if let Some(node) = Self::attempt(config.clone()).await? {
            return Ok(node);
        }
        let config = afresh(config);
        for _ in 2..START_ATTEMPTS {
            info!("still connecting to veilid; starting over as a new node");
            if let Some(node) = Self::attempt(config.clone()).await? {
                return Ok(node);
            }
        }
        Self::attempt(config).await?.ok_or(UNREACHABLE.into())
    }

    // one node, started and given `ATTEMPT_TIMEOUT` to become ready: None
    // when it has not, by when it is shut down again so that its storage is
    // free for the next.
    async fn attempt(config: VeilidConfig) -> Result<Option<Node>> {
        let (calls_tx, calls) = flume::unbounded();
        let (routes_tx, dead_routes) = flume::unbounded();
        let ready = Arc::new(Flag::default());
        let became_ready = ready.clone();
        let callback: UpdateCallback = Arc::new(move |update| match update {
            VeilidUpdate::AppCall(call) => drop(calls_tx.send(*call)),
            VeilidUpdate::RouteChange(change) => drop(routes_tx.send(change.dead_routes.clone())),
            VeilidUpdate::Attachment(state) if state.public_internet_ready => became_ready.raise(),
            _ => {}
        });

        // boxed because the startup future is very large.
        let api = Box::pin(api_startup(callback, config)).await?;
        let attaching = async {
            api.attach().await?;
            ready.wait().await;
            Ok::<_, Error>(api.routing_context()?)
        };
        match timeout(ATTEMPT_TIMEOUT, attaching).await {
            Some(Ok(context)) => Ok(Some(Node {
                api,
                context,
                calls,
                dead_routes,
            })),
            Some(Err(err)) => {
                api.shutdown().await;
                Err(err)
            }
            None => {
                api.shutdown().await;
                Ok(None)
            }
        }
    }

    pub async fn shutdown(self) {
        self.api.shutdown().await;
    }
}
