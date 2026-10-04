//! one member of the swarm (DESIGN.md "peer slots"): holds a private route
//! and answers the requests that arrive on it. each file it serves is a
//! share, with a slot in that file's rendezvous record pointing at the
//! route. `send` runs a peer with one share; `serve` runs one for each
//! library, with a share per file, which answers the library requests
//! too, each for that library alone (see `Peers`).

use crate::download_log::{DownloadLog, Level, Line, Of, Seen, SeenFile, PROGRESS_INTERVAL};
use crate::members::Members;
use crate::node::read_slots;
use crate::protocol::{
    decode_peers_reply, decode_request, decode_slot, encode_error_reply, encode_files_reply,
    encode_ok_reply, encode_peers_reply, encode_piece_reply, encode_slot,
    encode_swarm_peers_request, matches_query, to_hex, DownloadId, Event, FileEntry, Hash, Request,
    Slot, LIBRARY_SLOT_SUBKEY, LOBBY_ROOT, PEER_SLOTS, SLOT_HEARTBEAT,
};
use crate::rt::{
    now_ms, race, retry, retry_times, sleep, spawn, unique_bytes, unix_now, Either, Flag,
};
use crate::store::BoxFuture;
use crate::Result;
use futures_util::stream::{self, FuturesUnordered, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tracing::{debug, info, warn};
use veilid_core::{
    PrivateSpec, RecordKey, RouteBlob, RouteId, RoutingContext, Stability, Target, VeilidAPI,
    VeilidAppCall,
};

// two peers picking at once can take the same slot; the loser picks again.
const CLAIM_ATTEMPTS: u16 = PEER_SLOTS;
// each slot write is preceded by a read that takes seconds, so the slots of
// many shares are written a few at a time.
const PUBLISH_CONCURRENCY: usize = 4;
// a quarter of a heartbeat: how much sooner than SLOT_HEARTBEAT a beat may
// come.
const BEAT_SPREAD_DIVISOR: u64 = 4;
// an unanswered announcement is said again every 5 seconds for a minute.
const ANNOUNCE_ATTEMPTS: u32 = 12;
const ANNOUNCE_RETRY: Duration = Duration::from_secs(5);
const LEAVING: &str = "the peer is leaving the swarm";
const NOT_SHARED: &str = "not shared here";
const NOT_HELD: &str = "piece not held";
const OUT_OF_RANGE: &str = "piece out of range";
const BAD_REQUEST: &str = "malformed request";

/// a piece, and the hashes that prove it against the file's root.
pub type Proven = (Vec<u8>, Vec<Hash>);

/// the pieces of one file, as far as this peer holds them.
pub trait Pieces: Send + Sync {
    fn count(&self) -> u64;

    /// how many bytes the whole file has.
    fn size(&self) -> u64;

    /// a piece with its proof, or None when it is not held yet.
    fn piece(&self, index: u64) -> BoxFuture<'_, Result<Option<Proven>>>;
}

/// what makes a peer a library (DESIGN.md "libraries").
pub trait LibraryIndex: Send + Sync {
    /// what the library's owner calls it; empty when it has no name.
    fn name(&self) -> String;

    /// every file shared, in the fixed order the catalog is paged in.
    fn files(&self) -> Vec<FileEntry>;

    /// the routes in the lobby's slots as they were last read, in slot
    /// order: what anyone who reads the lobby knows, and nothing a library
    /// learned another way, so that two libraries on one node never hand
    /// out one private list (DESIGN.md "a node's libraries").
    fn lobby(&self) -> Vec<Vec<u8>>;
}

/// what the peers of one node have in common (DESIGN.md "a node's
/// libraries"): a node with libraries is a peer for each, every one with a
/// route of its own, and a request is answered by the peer whose route it
/// arrived over, which knows of that library alone. a node that shares
/// single files has one peer, which answers everything.
pub struct Peers {
    all: Mutex<Vec<Weak<Peer>>>,
    // peers on their way to `run`, which keeps their slots up.
    added: (flume::Sender<Arc<Peer>>, flume::Receiver<Arc<Peer>>),
    // one claim at a time in any one record: two peers of this node picking
    // a slot of the lobby side by side would both write the same one.
    claims: Mutex<HashMap<String, Arc<async_lock::Mutex<()>>>>,
}

/// which of the peers, whose routes are `routes`, a request that arrived
/// over `arrived` is for. a node's only peer answers whatever arrives; of
/// several, the one whose route it is, and a request over no route of
/// theirs is nobody's.
fn addressed<T: PartialEq>(routes: &[Option<T>], arrived: Option<&T>) -> Option<usize> {
    let held = |route: &Option<T>| route.is_some() && route.as_ref() == arrived;
    match (routes.iter().position(held), routes.len()) {
        (Some(at), _) => Some(at),
        (None, 1) => Some(0),
        _ => None,
    }
}

impl Peers {
    pub fn new() -> Arc<Peers> {
        Arc::new(Peers {
            all: Mutex::default(),
            added: flume::unbounded(),
            claims: Mutex::default(),
        })
    }

    fn live(&self) -> Vec<Arc<Peer>> {
        let all = self.all.lock().unwrap();
        let live = all.iter().filter_map(Weak::upgrade);
        live.filter(|peer| !peer.closed.is_raised()).collect()
    }

    /// every download others are making from this node, or have made, as
    /// their own notices say, each with the library its file is in: None
    /// for a file shared on its own. for the node's owner alone: nothing of
    /// this is ever said to the network, where each peer speaks for itself.
    pub fn uploads(&self) -> Vec<(Option<String>, SeenFile)> {
        let of_peer = |peer: Arc<Peer>| {
            let library = peer.library.get().map(|library| library.name());
            let listed = peer.downloads.lock().unwrap().all();
            listed.into_iter().map(move |file| (library.clone(), file))
        };
        self.live().into_iter().flat_map(of_peer).collect()
    }

    /// the route blobs this node is reached by just now, whichever peer's.
    pub fn own_routes(&self) -> Vec<Vec<u8>> {
        let routes = self.live().into_iter().filter_map(|peer| peer.own_route());
        routes.collect()
    }

    // whether any peer of the node still serves from a record.
    fn holds(&self, key: &RecordKey) -> bool {
        let serves = |peer: &Arc<Peer>| peer.shares().iter().any(|held| held.key == *key);
        self.live().iter().any(serves)
    }

    fn claim_lock(&self, key: &RecordKey) -> Arc<async_lock::Mutex<()>> {
        let mut claims = self.claims.lock().unwrap();
        claims.entry(key.to_string()).or_default().clone()
    }

    fn addressee(&self, arrived: Option<&RouteId>) -> Option<Arc<Peer>> {
        let peers = self.live();
        let routes: Vec<Option<RouteId>> = peers.iter().map(|peer| peer.route_id()).collect();
        addressed(&routes, arrived).map(|at| peers[at].clone())
    }

    /// give every slot of every peer up, as the node shuts down.
    pub async fn leave(&self) {
        for peer in self.live() {
            peer.leave().await;
        }
    }

    /// hand each request to the peer it is for, tell every peer of the
    /// routes that died, and keep each one's slots up. never returns.
    pub async fn run(
        &self,
        calls: &flume::Receiver<VeilidAppCall>,
        dead_routes: &flume::Receiver<Vec<RouteId>>,
    ) {
        let answering = async {
            while let Ok(call) = calls.recv_async().await {
                // a request over a route no peer has any more goes
                // unanswered, as one to a route that is gone does.
                if let Some(peer) = self.addressee(call.route_id()) {
                    spawn(peer.answer(call));
                }
            }
        };
        let burying = async {
            while let Ok(dead) = dead_routes.recv_async().await {
                for peer in self.live() {
                    let _ = peer.dead.0.send(dead.clone());
                }
            }
        };
        let keeping = async {
            let mut kept = FuturesUnordered::new();
            loop {
                let ended = async {
                    match kept.is_empty() {
                        true => std::future::pending().await,
                        false => kept.next().await,
                    }
                };
                let added = race(self.added.1.recv_async(), ended).await;
                if let Either::Left(Ok(peer)) = added {
                    kept.push(async move { peer.keep().await });
                }
            }
        };
        futures_util::join!(answering, burying, keeping);
    }
}

/// one file a peer serves, and the slot it holds for it.
pub struct Share {
    /// the file's record, open for writing.
    pub key: RecordKey,
    pub root: Hash,
    // what this peer calls the file, in its slot and in the log.
    name: String,
    pieces: Arc<dyn Pieces>,
    // whether every piece is held, which the slot says. a download's share
    // becomes so when its last piece is in.
    complete: AtomicBool,
    served: AtomicU64,
    // the slot held and the route blob last written there. behind an async
    // lock so that one refresh of a share runs at a time.
    claimed: async_lock::Mutex<Option<(u32, Vec<u8>)>>,
    // the other peers of the file, to tell whoever asks.
    members: Mutex<Members>,
}

pub struct Peer {
    api: VeilidAPI,
    context: RoutingContext,
    // the node's other peers, if it has any.
    peers: Arc<Peers>,
    is_library: bool,
    // the highest slot this peer will take, when a test narrows them to
    // fill a swarm with two peers instead of nine. never set otherwise.
    slot_limit: Option<u32>,
    // set once the library exists, which itself needs the peer.
    library: OnceLock<Arc<dyn LibraryIndex>>,
    // the library's own record, which a link to the whole library points
    // at. its one slot names this peer's route.
    home: OnceLock<RecordKey>,
    // None once veilid reports the route dead, until it is replaced.
    route: async_lock::Mutex<Option<RouteBlob>>,
    // a copy for callers that cannot wait on the route lock.
    current: Mutex<Option<(RouteId, Vec<u8>)>>,
    // the routes veilid reports dead, as `Peers::run` passes them on.
    dead: (flume::Sender<Vec<RouteId>>, flume::Receiver<Vec<RouteId>>),
    // a list, not a map by root: a library can hold one file under two
    // names, which are two shares with one root, and two slots in the one
    // record, each saying its name.
    shares: Mutex<Vec<Arc<Share>>>,
    served: AtomicU64,
    // what the downloaders have said of themselves, and what to log of it.
    downloads: Mutex<DownloadLog>,
    // raised when the peer starts giving its slots up.
    leaving: Flag,
    // raised when the peer is gone for good, and its route with it.
    closed: Flag,
}

// a share on its way to holding a slot.
struct Pending<'a> {
    peer: &'a Peer,
    share: &'a Arc<Share>,
    claimed: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.claimed {
            self.peer.forget(self.share);
        }
    }
}

/// the routes of the peers a slot read names, other than our `own`.
fn slot_routes(slots: &[(u32, Result<Option<Slot>>)], now: u64, own: &[u8]) -> Vec<Vec<u8>> {
    let named = slots
        .iter()
        .filter_map(|(_, slot)| slot.as_ref().ok()?.as_ref());
    let fresh = named.filter(|slot| !slot.is_stale(now) && slot.route != own);
    fresh.map(|slot| slot.route.clone()).collect()
}

// the vacant slots a peer may take, given the limit a test put on them.
fn usable(vacant: Vec<u32>, limit: Option<u32>) -> Vec<u32> {
    let allowed = |subkey: &u32| limit.is_none_or(|limit| *subkey <= limit);
    vacant.into_iter().filter(allowed).collect()
}

/// the slots a peer may take: those naming no peer or one gone stale. a slot
/// that could not be read is left alone, since a live peer may hold it.
fn vacant_slots(slots: &[(u32, Result<Option<Slot>>)], now: u64) -> Vec<u32> {
    let vacant = |slot: &Result<Option<Slot>>| matches!(slot, Ok(peer) if peer.as_ref().is_none_or(|peer| peer.is_stale(now)));
    let free = slots.iter().filter(|(_, slot)| vacant(slot));
    free.map(|(subkey, _)| *subkey).collect()
}

impl Peer {
    /// one more peer of the node whose peers are `peers`, with a route of
    /// its own once it has something to serve.
    pub fn new(
        api: &VeilidAPI,
        context: &RoutingContext,
        peers: &Arc<Peers>,
        is_library: bool,
        slot_limit: Option<u32>,
    ) -> Arc<Peer> {
        let peer = Arc::new(Peer {
            api: api.clone(),
            context: context.clone(),
            peers: peers.clone(),
            is_library,
            slot_limit,
            library: OnceLock::new(),
            home: OnceLock::new(),
            route: Default::default(),
            current: Default::default(),
            dead: flume::unbounded(),
            shares: Default::default(),
            served: AtomicU64::new(0),
            downloads: Default::default(),
            leaving: Flag::default(),
            closed: Flag::default(),
        });
        peers.all.lock().unwrap().push(Arc::downgrade(&peer));
        let _ = peers.added.0.send(peer.clone());
        peer
    }

    pub fn set_library(&self, library: Arc<dyn LibraryIndex>) {
        let _ = self.library.set(library);
    }

    /// the blob others reach this peer by, which is how a fetcher tells its
    /// own slot from the rest. None while there is no live route.
    pub fn own_route(&self) -> Option<Vec<u8>> {
        let current = self.current.lock().unwrap();
        current.as_ref().map(|(_, blob)| blob.clone())
    }

    fn route_id(&self) -> Option<RouteId> {
        let current = self.current.lock().unwrap();
        current.as_ref().map(|(id, _)| id.clone())
    }

    fn shares(&self) -> Vec<Arc<Share>> {
        self.shares.lock().unwrap().clone()
    }

    // what the library's owner calls it, as its slots say; empty for a peer
    // that is no library, and for a library without a name.
    fn library_name(&self) -> String {
        let library = self.library.get();
        library.map_or_else(String::new, |library| library.name())
    }

    /// the blob of this peer's route, allocating one if it has none. shares
    /// publishing side by side wait on the one allocation.
    async fn route_blob(&self) -> Result<Vec<u8>> {
        let mut route = self.route.lock().await;
        // a peer that is gone for good takes no route again: whatever was
        // still on its way to publishing something of it gives up here.
        if self.closed.is_raised() {
            return Err(LEAVING.into());
        }
        if route.is_none() {
            let spec = PrivateSpec {
                stability: Stability::Reliable,
                ..Default::default()
            };
            *route = Some(self.api.new_custom_private_route(spec).await?);
        }
        let held = route
            .as_ref()
            .map(|route| (route.route_id.clone(), route.blob.clone()));
        self.current.lock().unwrap().clone_from(&held);
        Ok(held.map(|(_, blob)| blob).unwrap_or_default())
    }

    /// write a slot as this peer's, with the current time. false when the
    /// network already holds a newer value, so another peer has the slot.
    async fn write_slot(&self, share: &Share, subkey: u32, route: Vec<u8>) -> Result<bool> {
        // the lobby is no file: a slot there says what the library is called.
        let name = match share.root {
            LOBBY_ROOT => self.library_name(),
            _ => share.name.clone(),
        };
        let value = encode_slot(&Slot {
            complete: share.complete.load(Ordering::Relaxed),
            library: self.is_library,
            time: unix_now(),
            name,
            route,
        });
        let key = share.key.clone();
        let newer = self
            .context
            .set_dht_value(key, subkey, value.clone(), None)
            .await?;
        Ok(newer.is_none_or(|newer| newer.data() == value))
    }

    /// whether no other peer has taken a slot since `blob` was written to it.
    async fn still_holds(&self, share: &Share, subkey: u32, blob: &[u8]) -> Result<bool> {
        let key = share.key.clone();
        let value = self.context.get_dht_value(key, subkey, true).await?;
        let peer = value.and_then(|value| decode_slot(value.data()));
        Ok(peer.is_none_or(|peer| peer.route == blob))
    }

    /// take a vacant slot, picked at random so that peers arriving together
    /// seldom collide. a full swarm is left for the next heartbeat.
    async fn claim(&self, share: &Share, blob: &[u8]) -> Result<Option<(u32, Vec<u8>)>> {
        let claims = self.peers.claim_lock(&share.key);
        let _claiming = claims.lock().await;
        for _ in 0..CLAIM_ATTEMPTS {
            let slots = read_slots(&self.context, &share.key).await;
            share.meet(&slot_routes(&slots, unix_now(), blob));
            let vacant = usable(vacant_slots(&slots, unix_now()), self.slot_limit);
            if vacant.is_empty() {
                if let Some((_, Err(err))) = slots.into_iter().find(|(_, slot)| slot.is_err()) {
                    return Err(err);
                }
                warn!("swarm is full, not serving");
                return Ok(None);
            }
            let pick = u64::from_be_bytes(unique_bytes()) as usize % vacant.len();
            if self.write_slot(share, vacant[pick], blob.to_vec()).await? {
                info!("serving from slot {}", vacant[pick]);
                return Ok(Some((vacant[pick], blob.to_vec())));
            }
        }
        Err("could not claim a peer slot".into())
    }

    /// make the share's record point at the current route: rewrite the slot
    /// held, or claim one if none is held or it was lost to another peer.
    async fn refresh(&self, share: &Arc<Share>) -> Result<()> {
        let mut claimed = share.claimed.lock().await;
        // looked at under the lock `leave` takes to give the slot up: a
        // claim either is in before the leaving, and is vacated by it, or
        // is not made at all. nothing is served from a slot taken after.
        if self.leaving.is_raised() {
            return Ok(());
        }
        let blob = self.route_blob().await?;
        if let Some((subkey, written)) = claimed.clone() {
            if self.still_holds(share, subkey, &written).await?
                && self.write_slot(share, subkey, blob.clone()).await?
            {
                *claimed = Some((subkey, blob.clone()));
                self.announce(share, &blob);
                return Ok(());
            }
        }
        *claimed = None;
        *claimed = self.claim(share, &blob).await?;
        self.announce(share, &blob);
        Ok(())
    }

    /// tell the other peers of a file where this one is, and learn from
    /// each whom else it knows (DESIGN.md "peer exchange"). it is how a peer
    /// that found the slots full is found at all, and how the peers in the
    /// slots keep word of each other between slot reads. not waited for.
    fn announce(&self, share: &Arc<Share>, blob: &[u8]) {
        let request = encode_swarm_peers_request(&share.root, blob);
        for route in share.members.lock().unwrap().to_call(unix_now(), blob) {
            let (api, context) = (self.api.clone(), self.context.clone());
            let (share, request) = (share.clone(), request.clone());
            spawn(async move {
                let Ok(target) = api.import_remote_private_route(route.clone()) else {
                    return;
                };
                let Some(told) = announce_until_heard(&context, target, request).await else {
                    return;
                };
                let (mut members, now) = (share.members.lock().unwrap(), unix_now());
                members.met(&route, now);
                told.iter().for_each(|route| members.heard_of(route, now));
            });
        }
    }

    /// serve a file and claim a slot in its record, trying `attempts` times.
    /// `complete` says whether every piece is held.
    pub async fn add(
        &self,
        key: RecordKey,
        root: Hash,
        name: &str,
        pieces: Arc<dyn Pieces>,
        complete: bool,
        attempts: u32,
    ) -> Result<Arc<Share>> {
        let share = Arc::new(Share {
            key,
            root,
            name: name.to_owned(),
            pieces,
            complete: AtomicBool::new(complete),
            served: AtomicU64::new(0),
            claimed: Default::default(),
            members: Default::default(),
        });
        self.shares.lock().unwrap().push(share.clone());
        // unless the claim succeeds the share is forgotten again, also when
        // the caller gives up waiting and drops this future.
        let mut pending = Pending {
            peer: self,
            share: &share,
            claimed: false,
        };
        retry_times(attempts, || self.refresh(&share)).await?;
        if self.leaving.is_raised() {
            return Err(LEAVING.into());
        }
        pending.claimed = true;
        Ok(share.clone())
    }

    /// every piece of a share is held now: say so in its slot.
    pub async fn complete(&self, share: &Arc<Share>) {
        share.complete.store(true, Ordering::Relaxed);
        if let Err(err) = self.refresh(share).await {
            // the next heartbeat writes the slot again.
            debug!("could not mark a share complete: {err}");
        }
    }

    // a share is no longer served; nor are its file's downloads listed,
    // unless another share of this peer is of the same file.
    fn forget(&self, share: &Arc<Share>) {
        let mut shares = self.shares.lock().unwrap();
        shares.retain(|held| !Arc::ptr_eq(held, share));
        if shares.iter().all(|held| held.root != share.root) {
            let mut downloads = self.downloads.lock().unwrap();
            downloads.forget(&to_hex(&share.root));
        }
    }

    /// stop serving a file and give its slot up, so fetchers stop trying a
    /// peer that no longer has it.
    pub async fn remove(&self, share: &Arc<Share>) {
        self.forget(share);
        let Some((subkey, _)) = share.claimed.lock().await.take() else {
            return;
        };
        if let Err(err) = self.write_slot(share, subkey, Vec::new()).await {
            warn!("could not give up slot {subkey}: {err}");
        }
    }

    /// close a record, unless a share of this node is still served from it:
    /// one file under two names, or in two libraries, is two shares in one
    /// record.
    pub async fn close(&self, key: &RecordKey) {
        if !self.peers.holds(key) {
            let _ = self.context.close_dht_record(key.clone()).await;
        }
    }

    /// stop serving a file for good: give its slot up and close its record.
    pub async fn release(&self, share: &Arc<Share>) {
        self.remove(share).await;
        self.close(&share.key).await;
    }

    /// give every slot up, as the peer shuts down, and take no other: a
    /// share still on its way to a slot when this is called gets none.
    pub async fn leave(&self) {
        self.leaving.raise();
        for share in self.shares() {
            self.remove(&share).await;
        }
        if let Err(err) = self.write_home(Vec::new()).await {
            warn!("could not vacate the library record: {err}");
        }
    }

    /// stop being a peer for good, as a library that is no longer shared
    /// does: give every slot up and let the route go.
    pub async fn retire(&self) {
        self.leave().await;
        if let Some(route) = self.route.lock().await.take() {
            let _ = self.api.release_private_route(route.route_id);
        }
        *self.current.lock().unwrap() = None;
        self.closed.raise();
    }

    /// write the slots again now, where what they say has changed: the
    /// library's name.
    pub async fn republish(self: &Arc<Self>) {
        if let Err(err) = self.refresh_all().await {
            // the next heartbeat tries again.
            warn!("slot update failed: {err}");
        }
    }

    /// point the library's own record at this peer, now and at every refresh
    /// from here on. the record is open for writing under `key`.
    pub async fn publish_home(&self, key: RecordKey) -> Result<()> {
        let _ = self.home.set(key);
        self.refresh_home().await
    }

    // write the library record's one slot, if this peer has such a record.
    // it is the library's alone, so there is nothing to claim or check.
    async fn write_home(&self, route: Vec<u8>) -> Result<()> {
        let Some(key) = self.home.get() else {
            return Ok(());
        };
        let value = encode_slot(&Slot {
            complete: true,
            library: self.is_library,
            time: unix_now(),
            name: self.library_name(),
            route,
        });
        let written = self
            .context
            .set_dht_value(key.clone(), LIBRARY_SLOT_SUBKEY, value, None);
        written.await.map(drop).map_err(Into::into)
    }

    async fn refresh_home(&self) -> Result<()> {
        if self.home.get().is_none() {
            return Ok(());
        }
        let route = self.route_blob().await?;
        retry(|| self.write_home(route.clone())).await
    }

    /// refresh every share's slot, a few at a time, and the library record.
    async fn refresh_all(self: &Arc<Self>) -> Result<()> {
        let home = self.refresh_home().await;
        let refresh = |share: Arc<Share>| async move { retry(|| self.refresh(&share)).await };
        let refreshes = stream::iter(self.shares()).map(refresh);
        let outcomes: Vec<Result<()>> = refreshes
            .buffer_unordered(PUBLISH_CONCURRENCY)
            .collect()
            .await;
        let shares = outcomes.into_iter().collect::<Result<Vec<()>>>();
        home.and(shares.map(drop))
    }

    /// forget the route if it is among the dead ones. true if it was.
    async fn drop_route(&self, dead: &[RouteId]) -> bool {
        let mut route = self.route.lock().await;
        let died = route
            .as_ref()
            .is_some_and(|route| dead.contains(&route.route_id));
        if died {
            *route = None;
            *self.current.lock().unwrap() = None;
        }
        died
    }

    /// refresh at every heartbeat and whenever the route dies. never returns.
    async fn maintain(self: &Arc<Self>) {
        let beat = || now_ms() + beat_delay(u64::from_be_bytes(unique_bytes()));
        // the beats keep their own time, whatever happens between them.
        let mut next_beat = beat();
        loop {
            let until_beat = Duration::from_millis(next_beat.saturating_sub(now_ms()));
            let replaced = match race(self.dead.1.recv_async(), sleep(until_beat)).await {
                Either::Left(Ok(dead)) => self.drop_route(&dead).await,
                // the peer itself holds the other end: it never closes.
                Either::Left(Err(_)) => return std::future::pending().await,
                Either::Right(()) => {
                    next_beat = beat();
                    false
                }
            };
            match self.refresh_all().await {
                Ok(()) if replaced => warn!("route replaced"),
                Ok(()) => {}
                // the next heartbeat tries again.
                Err(err) => warn!("slot update failed: {err}"),
            }
        }
    }

    // keep the slots up and say how the downloads are getting on, until the
    // peer is gone for good. requests are answered apart from this
    // (`Peers::run`), so that a slow dht write never holds up a piece.
    async fn keep(self: Arc<Self>) {
        let kept = futures_util::future::join(self.maintain(), self.report());
        race(kept, self.closed.wait()).await;
    }

    async fn answer(self: Arc<Self>, call: VeilidAppCall) {
        let reply = self.reply(call.message()).await;
        let reply = reply.unwrap_or_else(|err| encode_error_reply(&err.to_string()));
        if let Err(err) = self.api.app_call_reply(call.id(), reply).await {
            // the caller gave up waiting; it will ask again.
            warn!("reply failed: {err}");
        }
    }

    async fn reply(&self, request: &[u8]) -> Result<Vec<u8>> {
        let library = self.library.get();
        match (decode_request(request)?, library) {
            (Request::Piece(root, index), _) => self.piece_reply(&root, u64::from(index)).await,
            (Request::Notice(event, id, root), _) => self.note_download(event, id, &root),
            (Request::Progress(id, root, done), _) => self.note_progress(id, &root, done),
            (Request::SwarmPeers(root, caller), _) => self.swarm_peers_reply(&root, &caller),
            (Request::Catalog(offset), Some(library)) => {
                let files = library.files();
                emit(vec![listing_line(offset, files.len())]);
                encode_files_reply(&files, offset as usize, &library.name())
            }
            (Request::Search(query), Some(library)) => {
                let matches = |file: &FileEntry| matches_query(&file.name, &query);
                let found: Vec<FileEntry> = library.files().into_iter().filter(matches).collect();
                encode_files_reply(&found, 0, &library.name())
            }
            (Request::Peers(caller), Some(library)) => {
                let own = self.own_route().unwrap_or_default();
                Ok(peers_reply(library.lobby(), &own, &caller))
            }
            // a peer that is no library does not know these.
            (_, None) => Err(BAD_REQUEST.into()),
        }
    }

    // any share of the file will do: they hold the same pieces, though a
    // download among them may not have this one yet.
    async fn piece_reply(&self, root: &Hash, index: u64) -> Result<Vec<u8>> {
        let shares = self.shares();
        let of_file: Vec<&Arc<Share>> = shares.iter().filter(|share| share.root == *root).collect();
        let first = of_file.first().ok_or(NOT_SHARED)?;
        if index >= first.pieces.count() {
            return Err(OUT_OF_RANGE.into());
        }
        for share in of_file {
            if let Some((piece, proof)) = share.pieces.piece(index).await? {
                self.served.fetch_add(1, Ordering::Relaxed);
                share.served.fetch_add(1, Ordering::Relaxed);
                return Ok(encode_piece_reply(&piece, &proof));
            }
        }
        Err(NOT_HELD.into())
    }

    // the other peers of a file this one serves, less the caller, who is
    // then one of them if it gave its route. two shares of one file (one
    // name each) each keep word of its swarm, and the caller is told what
    // both know.
    fn swarm_peers_reply(&self, root: &Hash, caller: &[u8]) -> Result<Vec<u8>> {
        let shares = self.shares();
        let mut of_file = shares.iter().filter(|share| share.root == *root).peekable();
        of_file.peek().ok_or(NOT_SHARED)?;
        let (own, now) = (self.own_route().unwrap_or_default(), unix_now());
        let mut known: Vec<Vec<u8>> = Vec::new();
        for share in of_file {
            let mut members = share.members.lock().unwrap();
            for route in members.to_pass_on(now, caller, &own) {
                if !known.contains(&route) {
                    known.push(route);
                }
            }
            members.met(caller, now);
        }
        Ok(encode_peers_reply(&known))
    }

    // the name and piece count of the file with this root, as notices about
    // it are logged.
    fn file_of(&self, root: &Hash) -> Result<(String, u64, u64)> {
        let shares = self.shares();
        let share = shares.iter().find(|share| share.root == *root);
        let share = share.ok_or(NOT_SHARED)?;
        let pieces = &share.pieces;
        Ok((share.name.clone(), pieces.count(), pieces.size()))
    }

    // downloaders are anonymous, so their own notices are the only way to
    // tell the user what they are doing. what is said, and when, is the
    // download log's to decide.
    fn note_download(&self, event: Event, id: DownloadId, root: &Hash) -> Result<Vec<u8>> {
        let (name, total, size) = self.file_of(root)?;
        let (id, now, file) = (to_hex(&id), unix_now(), to_hex(root));
        let of = Of {
            file: &file,
            name: &name,
            total,
            size,
        };
        let served = self.served.load(Ordering::Relaxed);
        let mut downloads = self.downloads.lock().unwrap();
        emit(match event {
            Event::Started => downloads.started(now, &id, of),
            Event::Finished => downloads.finished(now, &id, of, served),
        });
        Ok(encode_ok_reply())
    }

    fn note_progress(&self, id: DownloadId, root: &Hash, done: u32) -> Result<Vec<u8>> {
        let (name, total, size) = self.file_of(root)?;
        let file = to_hex(root);
        let of = Of {
            file: &file,
            name: &name,
            total,
            size,
        };
        let mut downloads = self.downloads.lock().unwrap();
        emit(downloads.progress(unix_now(), &to_hex(&id), of, u64::from(done)));
        Ok(encode_ok_reply())
    }

    /// who is downloading the file with this root from this peer, or has
    /// been, as far as their own notices say: oldest first.
    pub fn downloaders(&self, root: &Hash) -> Vec<Seen> {
        self.downloads.lock().unwrap().of_file(&to_hex(root))
    }

    /// say how the downloads are getting on, every so often. never returns.
    async fn report(&self) {
        loop {
            emit(self.downloads.lock().unwrap().tick(unix_now()));
            sleep(PROGRESS_INTERVAL).await;
        }
    }
}

// write the download log's lines to the real log.
/// what to say when someone asks for the catalog. a browse asks for it a
/// page at a time, and only its first page is worth a visible line. the
/// request is anonymous, so there is no saying who asked.
fn listing_line(offset: u32, total: usize) -> Line {
    let (level, message) = match offset {
        0 => (
            Level::Info,
            format!("library listing requested ({total} files)"),
        ),
        _ => (
            Level::Debug,
            format!("library listing page requested (offset {offset})"),
        ),
    };
    Line { level, message }
}

fn emit(lines: Vec<Line>) {
    for Line { level, message } in lines {
        match level {
            Level::Debug => debug!("{message}"),
            Level::Info => info!("{message}"),
            Level::Warn => warn!("{message}"),
        }
    }
}

impl Share {
    /// pieces of this file served so far.
    pub fn served(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }

    // there is direct word of the peers at these routes.
    fn meet(&self, routes: &[Vec<u8>]) {
        let (mut members, now) = (self.members.lock().unwrap(), unix_now());
        routes.iter().for_each(|route| members.met(route, now));
    }
}

// the libraries in the lobby, less the one asked and the one asking. the
// node's other libraries are told of like anyone else's when they are in
// the lobby: leaving them out would say whose they are. the caller is
// not remembered, so nothing is told here that the lobby does not say.
fn peers_reply(lobby: Vec<Vec<u8>>, own: &[u8], caller: &[u8]) -> Vec<u8> {
    let others = |route: &Vec<u8>| route != own && route != caller;
    let known: Vec<Vec<u8>> = lobby.into_iter().filter(others).collect();
    encode_peers_reply(&known)
}

/// one announcement to one peer, said again until it is answered: a route
/// that was just published is often silent for its first half minute, and a
/// peer beyond the slots that was not heard then would stay unknown until
/// its next heartbeat. gives up well before that heartbeat comes.
async fn announce_until_heard(
    context: &RoutingContext,
    target: RouteId,
    request: Vec<u8>,
) -> Option<Vec<Vec<u8>>> {
    for attempt in 0..ANNOUNCE_ATTEMPTS {
        if attempt > 0 {
            sleep(ANNOUNCE_RETRY).await;
        }
        let call = context.app_call(Target::RouteId(target.clone()), request.clone());
        if let Ok(reply) = call.await {
            return decode_peers_reply(&reply).ok();
        }
    }
    None
}

/// milliseconds until a peer's next heartbeat: SLOT_HEARTBEAT at most, and
/// by `chance` up to a quarter less. slots say when they were written, so
/// the peers of one node, each drawing its own delay at every beat, do not
/// write theirs at the same instants beat after beat.
fn beat_delay(chance: u64) -> u64 {
    let whole = SLOT_HEARTBEAT.as_millis() as u64;
    whole - chance % (whole / BEAT_SPREAD_DIVISOR + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SLOT_STALE_SECONDS;

    const NOW: u64 = 1_000_000;

    fn peer(time: u64) -> Result<Option<Slot>> {
        Ok(Some(Slot {
            complete: true,
            library: false,
            time,
            name: String::new(),
            route: vec![1],
        }))
    }

    #[test]
    fn only_the_first_page_of_a_listing_is_logged_visibly() {
        let first = listing_line(0, 12);
        assert_eq!(first.level, Level::Info);
        assert_eq!(first.message, "library listing requested (12 files)");
        for offset in [1, 200, u32::MAX] {
            let later = listing_line(offset, 12);
            assert_eq!(later.level, Level::Debug);
            let message = format!("library listing page requested (offset {offset})");
            assert_eq!(later.message, message);
        }
    }

    #[test]
    fn only_slots_known_to_be_free_are_vacant() {
        let slots = vec![
            (1, peer(NOW)),
            (2, Ok(None)),
            (3, peer(NOW - SLOT_STALE_SECONDS - 1)),
            (4, Err("unreachable".into())),
            (5, peer(NOW - SLOT_STALE_SECONDS)),
        ];
        assert_eq!(vacant_slots(&slots, NOW), [2, 3]);
    }

    #[test]
    fn a_slot_read_names_the_fresh_peers_other_than_us() {
        let named = |route: u8, time| {
            Ok(Some(Slot {
                complete: true,
                library: false,
                time,
                name: String::new(),
                route: vec![route],
            }))
        };
        let slots = vec![
            (1, named(1, NOW)),
            (2, Ok(None)),
            (3, named(3, NOW - SLOT_STALE_SECONDS - 1)),
            (4, Err("unreachable".into())),
            (5, named(5, NOW)),
        ];
        assert_eq!(slot_routes(&slots, NOW, &[5]), [vec![1]]);
        assert_eq!(slot_routes(&slots, NOW, &[]), [vec![1], vec![5]]);
    }

    #[test]
    fn a_peers_reply_is_the_lobby_less_the_one_asked_and_the_one_asking() {
        let lobby = || vec![vec![1, 2, 3], vec![7], vec![9, 9]];
        let told = |own: &[u8], caller: &[u8]| {
            decode_peers_reply(&peers_reply(lobby(), own, caller)).unwrap()
        };
        assert_eq!(told(&[7], &[9, 9]), [vec![1, 2, 3]]);
        // in the lobby's own order, whoever is asked: two libraries of one
        // node say what two strangers who read the lobby would.
        assert_eq!(told(&[7], &[]), [vec![1, 2, 3], vec![9, 9]]);
        assert_eq!(told(&[9, 9], &[]), [vec![1, 2, 3], vec![7]]);
        // one that is not in the lobby tells of everyone in it.
        assert_eq!(told(&[4], &[]), lobby());
    }

    #[test]
    fn a_request_is_for_the_peer_whose_route_it_arrived_over() {
        let routes = [Some(10), None, Some(30)];
        assert_eq!(addressed(&routes, Some(&30)), Some(2));
        assert_eq!(addressed(&routes, Some(&10)), Some(0));
        // over a route none of them has, or over none, it is nobody's: a
        // peer without a route is not handed what it was never asked.
        assert_eq!(addressed(&routes, Some(&20)), None);
        assert_eq!(addressed(&routes, None), None);
        // a node's only peer answers whatever arrives, as it always has.
        assert_eq!(addressed(&[Some(10)], Some(&20)), Some(0));
        assert_eq!(addressed(&[None::<u8>], None), Some(0));
        assert_eq!(addressed(&[] as &[Option<u8>], Some(&1)), None);
    }

    #[test]
    fn heartbeats_come_within_a_quarter_of_the_longest_wait() {
        let whole = SLOT_HEARTBEAT.as_millis() as u64;
        let spread = whole / BEAT_SPREAD_DIVISOR;
        let delays = (0..5000).map(|chance| beat_delay(chance * 7919));
        assert!(delays
            .into_iter()
            .all(|delay| (whole - spread..=whole).contains(&delay)));
        assert_eq!(beat_delay(0), whole);
        assert_eq!(beat_delay(spread), whole - spread);
        assert_eq!(beat_delay(spread + 1), whole);
        // two peers that draw apart write apart.
        assert_ne!(beat_delay(1), beat_delay(2));
    }
}
