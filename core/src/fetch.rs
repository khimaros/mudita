//! fetching side: resolve a rendezvous record, then pull and verify pieces
//! from every peer that holds a slot in it. a server also serves what a
//! download has verified so far, which is why one can read its pieces back.

use crate::control::Found;
use crate::download_log::PROGRESS_INTERVAL;
use crate::merkle::{blake3_hash, leaf_hash, verify_proof, ProofStore};
use crate::node::{read_slot, record_key, slot_subkeys};
use crate::peer::{Peer, Pieces, Proven};
use crate::protocol::{
    chosen_name, common_names, decode_manifest, decode_peers_reply, decode_piece_reply,
    encode_notice, encode_piece_request, encode_progress_notice, encode_swarm_peers_request,
    hash_from_hex, is_error_reply, parse_link, piece_count, piece_range, save_name, share_keys,
    DownloadId, Event, FileEntry, Hash, Link, Manifest, FALLBACK_NAME, MANIFEST_SUBKEY,
    PEER_REFRESH, PEER_STRIKES, PIECE_PATIENCE, PIECE_TIMEOUT, SLOT_REREAD, SWARM_MAX,
};
use crate::rt::{
    now_ms, race, retry, retry_times, sleep, spawn, timeout, unique_bytes, unix_now, Either, Flag,
    MAX_RETRY_DELAY_FACTOR, RETRY_DELAY,
};
use crate::store::{BoxFuture, Sink, Sinks};
use crate::Result;
use futures_util::future::{join, join_all};
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, info, warn};
use veilid_core::{RecordKey, RouteId, RoutingContext, Target, VeilidAPI};

// each request is a slow multi-hop round trip, so several run at once. timed
// on a 5 MB file from one peer, 16 at once took about 3.5s where 8 took
// about 9; 32 took about 3, with answers twice as slow, so near the limit
// of what one peer keeps up with.
const CONCURRENT_REQUESTS: usize = 16;
// a peer that keeps failing is dropped once nothing has named it for this
// long: two rounds of looking, in the slots and among the other peers.
const FORGET_AFTER: Duration = Duration::from_secs(2 * PEER_REFRESH.as_secs());
const NO_ANSWER: &str = "no answer in time";
// few, because the command waits on the "finished" notice before it exits.
// enough tries to outlast the half minute a newly published route can be
// silent for: the waits between them add up to 20 seconds, the calls
// themselves to as much again.
const NOTICE_ATTEMPTS: u32 = 7;
const PERCENT: u64 = 100;
// a log is a stream, so progress is a line per step, not a redrawn counter.
const PROGRESS_STEP_PERCENT: u64 = 10;
const NO_PEERS: &str = "nobody is sharing this file right now";
// how long the slots are read for the names in them. a written slot usually
// comes back in a second, sometimes in five, and one nobody has written
// takes veilid ten to give up on. so the first name is waited for as long
// as an empty slot would take, and once it has come the others get only a
// short while: waiting for every slot would cost ten seconds every time.
// a peer is asked for the peers it knows every 5 seconds for 2 minutes,
// which outlasts the silence of a new route (30 seconds was seen).
const PEERS_ASK_RETRY: Duration = Duration::from_secs(5);
const PEERS_ASK_ATTEMPTS: u32 = 24;
const NAMES_WAIT: Duration = Duration::from_secs(10);
const NAMES_GRACE: Duration = Duration::from_millis(1500);
/// what asking for a library as if it were one file is answered with.
pub const LIBRARY_LINK: &str = "that is a link to a library, not to a file; \
    `mudita browse <link>` lists what it shares";

/// whether a peer that has not said which peers it knows, after `attempt`
/// tries, is asked once more: not for ever, and not once it has been dropped.
fn ask_again(attempt: u32, in_swarm: bool) -> bool {
    in_swarm && attempt < PEERS_ASK_ATTEMPTS
}

/// a peer as a slot or another peer named it. one that comes back with
/// another route is another peer, which is how a struck one starts clean.
struct Remote {
    blob: Vec<u8>,
    route: RouteId,
    // failed requests in a row.
    strikes: AtomicU32,
    // when a slot or a peer last named it, or it last delivered.
    heard: AtomicU64,
}

type Remotes = Arc<Vec<Arc<Remote>>>;

/// what a download that is itself part of the swarm brings along: the peer
/// it serves over, whose own slot is not somewhere to fetch from, and who
/// to tell about the libraries found among the file's peers.
pub struct Swarm {
    pub peer: Arc<Peer>,
    pub on_library: Box<dyn Fn(Vec<u8>) + Send + Sync>,
}

pub struct Fetch {
    api: VeilidAPI,
    context: RoutingContext,
    key: RecordKey,
    /// what the downloader calls the file: no part of what is verified.
    pub name: String,
    pub manifest: Manifest,
    total: u64,
    root: Hash,
    // where the verified pieces go, and are read back from to serve them.
    sink: Arc<dyn Sink>,
    swarm: Option<Swarm>,
    // how many piece requests run at once.
    concurrency: usize,
    next: AtomicU64,
    done: AtomicU64,
    // the pieces verified so far and their proofs, for serving them on.
    held: Mutex<(HashSet<u64>, ProofStore)>,
    // the peers to fetch from, which grow as slots are read and peers asked.
    peers: Mutex<Remotes>,
    // notified when a peer joins them, for workers with nobody to ask.
    found: event_listener::Event,
    // when the slots were last read, in milliseconds, so that workers
    // failing together, or again and again, share one read.
    reread: Mutex<Option<u64>>,
    // raised when the download ends, to stop the slot reads under way.
    stopped: Flag,
}

async fn read_subkey(context: &RoutingContext, key: &RecordKey, subkey: u32) -> Result<Vec<u8>> {
    retry(|| async move {
        let value = context.get_dht_value(key.clone(), subkey, true).await?;
        Ok(value.ok_or("record subkey is empty")?.data().to_vec())
    })
    .await
}

/// whether to stop asking a peer: it keeps failing, and neither a slot nor
/// another peer has named it for a while, so it has most likely left.
fn is_gone(strikes: u32, heard: u64, now: u64) -> bool {
    strikes >= PEER_STRIKES && now.saturating_sub(heard) > FORGET_AFTER.as_secs()
}

/// how long to wait before round `round` of a piece (the first is 0). the
/// first retry comes at once, since a failed peer is usually one of several;
/// later ones are spaced out, and with nobody to ask there is no point in
/// hurrying at all.
fn round_delay(round: u32, has_peers: bool) -> Duration {
    match (round, has_peers) {
        (0, _) => Duration::ZERO,
        (1, true) => Duration::ZERO,
        (round, _) => RETRY_DELAY * round.min(MAX_RETRY_DELAY_FACTOR),
    }
}

/// the order in which to ask peers for a piece: starting at the piece index,
/// which spreads pieces evenly, with struck peers after all the others.
fn peer_order(index: u64, strikes: &[u32]) -> Vec<usize> {
    let count = strikes.len();
    let start = (index % count.max(1) as u64) as usize;
    let rotated = (0..count).map(|offset| (start + offset) % count);
    let (struck, healthy): (Vec<usize>, Vec<usize>) =
        rotated.partition(|peer| strikes[*peer] >= PEER_STRIKES);
    [healthy, struck].concat()
}

impl Fetch {
    /// a download of the file `manifest` describes, here called `name`,
    /// into `sink`. its record `key` is open. `concurrency` is how many
    /// piece requests run at once, when an experiment wants other than the
    /// default.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        api: &VeilidAPI,
        context: &RoutingContext,
        key: &RecordKey,
        name: String,
        manifest: Manifest,
        sink: Arc<dyn Sink>,
        swarm: Option<Swarm>,
        concurrency: Option<usize>,
    ) -> Result<Arc<Fetch>> {
        Ok(Arc::new(Fetch {
            api: api.clone(),
            context: context.clone(),
            key: key.clone(),
            name,
            total: piece_count(manifest.size),
            root: hash_from_hex(&manifest.root)?,
            manifest,
            sink,
            swarm,
            concurrency: concurrency.unwrap_or(CONCURRENT_REQUESTS),
            next: AtomicU64::new(0),
            done: AtomicU64::new(0),
            held: Mutex::default(),
            peers: Mutex::default(),
            found: event_listener::Event::new(),
            reread: Mutex::default(),
            stopped: Flag::default(),
        }))
    }

    /// pieces verified so far, and how many there are.
    pub fn progress(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total)
    }

    /// drop what was written, for a download that was given up from outside.
    pub async fn discard(&self) {
        self.stopped.raise();
        self.sink.discard().await;
    }

    fn peers(&self) -> Remotes {
        self.peers.lock().unwrap().clone()
    }

    // this download's own route, if it serves what it has: not somewhere to
    // fetch from, and what it tells the peers it asks.
    fn own_route(&self) -> Vec<u8> {
        let own = self.swarm.as_ref().and_then(|swarm| swarm.peer.own_route());
        own.unwrap_or_default()
    }

    /// a slot or a peer names the peer at `blob`: fetch from it too. the
    /// route is imported even when the peer is known, because veilid forgets
    /// an imported route once requests over it have failed. a peer new to
    /// this download is asked for the peers it knows.
    fn add_peer(self: &Arc<Self>, blob: Vec<u8>) {
        if blob.is_empty() || blob == self.own_route() {
            return;
        }
        let Ok(route) = self.api.import_remote_private_route(blob.clone()) else {
            debug!("peer route rejected");
            return;
        };
        let fresh = {
            let mut peers = self.peers.lock().unwrap();
            if let Some(known) = peers.iter().find(|peer| peer.blob == blob) {
                known.heard.store(unix_now(), Ordering::Relaxed);
                return;
            }
            if peers.len() >= SWARM_MAX {
                return;
            }
            let fresh = Arc::new(Remote {
                blob,
                route,
                strikes: AtomicU32::new(0),
                heard: AtomicU64::new(unix_now()),
            });
            let grown: Vec<Arc<Remote>> = peers.iter().cloned().chain([fresh.clone()]).collect();
            info!("found {} peers", grown.len());
            *peers = Arc::new(grown);
            fresh
        };
        self.found.notify(usize::MAX);
        spawn(self.clone().ask_for_peers(fresh));
    }

    /// ask one peer which others it knows, and fetch from those too
    /// (DESIGN.md "peer exchange"). a peer that does not know the question,
    /// or does not answer, has simply told of nobody: it is no strike.
    /// the question is put again until it is answered: a route that was just
    /// published often answers nothing for its first half minute, and a peer
    /// asked only then would keep the peers beyond the slots to itself.
    async fn ask_for_peers(self: Arc<Self>, peer: Arc<Remote>) {
        let mut attempt = 0;
        loop {
            let request = encode_swarm_peers_request(&self.root, &self.own_route());
            let target = Target::RouteId(peer.route.clone());
            let asked = timeout(PIECE_TIMEOUT, self.context.app_call(target, request));
            if let Some(Ok(reply)) = asked.await {
                for route in decode_peers_reply(&reply).unwrap_or_default() {
                    self.add_peer(route);
                }
                return;
            }
            attempt += 1;
            let waited = race(sleep(PEERS_ASK_RETRY), self.stopped.wait()).await;
            let in_swarm = self.peers().iter().any(|known| Arc::ptr_eq(known, &peer));
            if matches!(waited, Either::Right(())) || !ask_again(attempt, in_swarm) {
                return;
            }
            let _ = self.api.import_remote_private_route(peer.blob.clone());
        }
    }

    // read one slot and take the peer it names, if it names a live one.
    async fn read_one_slot(self: &Arc<Self>, subkey: u32) {
        let slot = read_slot(&self.context, &self.key, subkey).await;
        let Ok(Some(slot)) = slot else { return };
        if slot.is_stale(unix_now()) {
            return;
        }
        if let (true, Some(swarm)) = (slot.library, &self.swarm) {
            (swarm.on_library)(slot.route.clone());
        }
        self.add_peer(slot.route);
    }

    /// read every slot in the background, taking each peer as its slot
    /// comes back: a slot nobody has written takes veilid ten seconds to
    /// give up on, and an inspect of the record takes as long, so nothing
    /// waits on those. such a read outlasts a quick download by seconds,
    /// and the record cannot be closed under it, so the reads stop when the
    /// download does. the flag returned is raised once all are done.
    fn read_slots(self: &Arc<Self>) -> Arc<Flag> {
        let (fetch, read) = (self.clone(), Arc::new(Flag::default()));
        let done = read.clone();
        spawn(async move {
            let reads = slot_subkeys().map(|subkey| fetch.read_one_slot(subkey));
            race(join_all(reads), fetch.stopped.wait()).await;
            done.raise();
        });
        read
    }

    /// read the slots again in the background, unless that was done a moment
    /// ago: workers fail in bursts, and none of them waits on this.
    fn reread_slots(self: &Arc<Self>) {
        let mut reread = self.reread.lock().unwrap();
        let recent = |last: u64| now_ms().saturating_sub(last) < SLOT_REREAD.as_millis() as u64;
        if reread.is_some_and(recent) {
            return;
        }
        *reread = Some(now_ms());
        self.read_slots();
    }

    // stop asking peers that keep failing and that nobody names any more.
    fn forget_gone(&self) {
        let mut peers = self.peers.lock().unwrap();
        let now = unix_now();
        let gone = |peer: &Arc<Remote>| {
            let strikes = peer.strikes.load(Ordering::Relaxed);
            is_gone(strikes, peer.heard.load(Ordering::Relaxed), now)
        };
        for peer in peers.iter().filter(|peer| gone(peer)) {
            let _ = self.api.release_private_route(peer.route.clone());
        }
        let kept: Vec<Arc<Remote>> = peers.iter().filter(|peer| !gone(peer)).cloned().collect();
        *peers = Arc::new(kept);
    }

    /// peers that join while a long download runs are worth having: look in
    /// the slots and ask the known peers again, every so often. never
    /// returns.
    async fn keep_refreshing(self: &Arc<Self>) {
        loop {
            sleep(PEER_REFRESH).await;
            self.forget_gone();
            self.reread_slots();
            for peer in self.peers().iter() {
                spawn(self.clone().ask_for_peers(peer.clone()));
            }
        }
    }

    fn verified(&self, index: u64, reply: &[u8]) -> Result<(Vec<u8>, Vec<Hash>)> {
        let (start, end) = piece_range(index, self.manifest.size);
        let (piece, proof) = decode_piece_reply(reply, (end - start) as usize)?;
        let leaf = leaf_hash(blake3_hash, piece);
        if !verify_proof(blake3_hash, &self.root, leaf, index, self.total, &proof) {
            return Err(format!("piece {index} failed verification").into());
        }
        Ok((piece.to_vec(), proof))
    }

    /// ask one peer for a piece and keep its strike count. an error reply is
    /// no strike: the peer is reachable and only lacks the piece.
    async fn request_piece(&self, index: u64, peer: &Remote) -> Result<(Vec<u8>, Vec<Hash>)> {
        let target = Target::RouteId(peer.route.clone());
        let request = encode_piece_request(&self.root, index as u32);
        let began = now_ms();
        let reply = match timeout(PIECE_TIMEOUT, self.context.app_call(target, request)).await {
            Some(reply) => reply.map_err(crate::Error::from),
            None => Err(NO_ANSWER.into()),
        };
        debug!(
            "piece {index} {} in {}ms",
            if reply.is_ok() { "answered" } else { "failed" },
            now_ms().saturating_sub(began)
        );
        let declined = reply.as_ref().is_ok_and(|reply| is_error_reply(reply));
        let piece = reply.and_then(|reply| self.verified(index, &reply));
        match &piece {
            Ok(_) => {
                peer.strikes.store(0, Ordering::Relaxed);
                peer.heard.store(unix_now(), Ordering::Relaxed);
            }
            Err(_) if !declined => {
                peer.strikes.fetch_add(1, Ordering::Relaxed);
                // veilid forgets an imported route once a request over it
                // has failed, after which every request fails at once.
                let _ = self.api.import_remote_private_route(peer.blob.clone());
            }
            Err(_) => {}
        }
        piece
    }

    /// one round: ask each peer in turn until one delivers.
    async fn request_from_peers(
        &self,
        index: u64,
        peers: &[Arc<Remote>],
    ) -> Result<(Vec<u8>, Vec<Hash>)> {
        let strikes: Vec<u32> = peers
            .iter()
            .map(|peer| peer.strikes.load(Ordering::Relaxed))
            .collect();
        let mut failure = NO_PEERS.into();
        for peer in peer_order(index, &strikes) {
            match self.request_piece(index, &peers[peer]).await {
                Err(err) => failure = err,
                piece => return piece,
            }
        }
        Err(failure)
    }

    // a large download runs for hours, over which routes die and requests
    // time out in bursts, so a piece is tried round after round for a good
    // while. a failed round has the slots read again, without waiting on
    // that: the peers it finds join the rounds as they turn up.
    async fn fetch_piece(self: &Arc<Self>, index: u64) -> Result<(Vec<u8>, Vec<Hash>)> {
        let began = now_ms();
        for failed in 1.. {
            let peers = self.peers();
            let patient = || now_ms().saturating_sub(began) < PIECE_PATIENCE.as_millis() as u64;
            let err = match self.request_from_peers(index, &peers).await {
                Err(err) if patient() => err,
                outcome => return outcome,
            };
            warn!("piece {index} failed, retrying: {err}");
            self.reread_slots();
            let delay = round_delay(failed, !peers.is_empty());
            // with nobody to ask, the wait ends as soon as somebody is found.
            if !delay.is_zero() {
                timeout(delay, self.found.listen()).await;
            }
        }
        unreachable!("a piece is fetched or given up on")
    }

    /// tell every usable peer a download began or ended. tried a few times
    /// and then dropped: a failure only costs the peer a log line.
    async fn notify(self: Arc<Self>, event: Event, id: DownloadId) {
        let usable = |peer: &&Arc<Remote>| peer.strikes.load(Ordering::Relaxed) < PEER_STRIKES;
        let peers = self.peers();
        let (context, root) = (&self.context, self.root);
        let notices = peers.iter().filter(usable).map(|peer| async move {
            let _ = retry_times(NOTICE_ATTEMPTS, || async move {
                let target = Target::RouteId(peer.route.clone());
                let notice = encode_notice(event, id, &root);
                Ok(context.app_call(target, notice).await.map(drop)?)
            })
            .await;
        });
        join_all(notices).await;
    }

    /// tell every known peer, every so often, how far the download has got,
    /// moved or not: it is also how a peer knows the download is still
    /// alive. each is sent once and not waited for, since the next is never
    /// far off, so a peer that does not answer holds nothing up. never
    /// returns.
    async fn report_progress(&self, id: DownloadId) {
        loop {
            sleep(PROGRESS_INTERVAL).await;
            let done = self.done.load(Ordering::Relaxed) as u32;
            for peer in self.peers().iter() {
                let context = self.context.clone();
                let target = Target::RouteId(peer.route.clone());
                let notice = encode_progress_notice(id, &self.root, done);
                spawn(async move {
                    let _ = context.app_call(target, notice).await;
                });
            }
        }
    }

    async fn worker(self: &Arc<Self>) -> Result<()> {
        loop {
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            if index >= self.total {
                return Ok(());
            }
            let (piece, proof) = self.fetch_piece(index).await?;
            self.sink.write(index, piece).await?;
            {
                let mut held = self.held.lock().unwrap();
                held.1.add(index, self.total, &proof);
                held.0.insert(index);
            }
            let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(percent) = progress_percent(done, self.total) {
                info!("{done}/{} pieces ({percent}%)", self.total);
            }
        }
    }

    /// run the workers to the last piece. the first one to give up on a
    /// piece ends the rest.
    async fn fetch_pieces(self: &Arc<Self>) -> Result<()> {
        let mut workers: FuturesUnordered<_> =
            (0..self.concurrency).map(|_| self.worker()).collect();
        let working = async {
            while let Some(outcome) = workers.next().await {
                outcome?;
            }
            Ok(())
        };
        match race(working, self.keep_refreshing()).await {
            Either::Left(outcome) => outcome,
            Either::Right(()) => Ok(()),
        }
    }

    /// start reading the slots and wait until one peer is known, or until
    /// every slot has been read and named nobody. the other peers join as
    /// their slots come back, while pieces are already being fetched.
    async fn find_first_peer(self: &Arc<Self>) {
        // listening before the reads begin, so that no peer slips by.
        let found = self.found.listen();
        *self.reread.lock().unwrap() = Some(now_ms());
        let read = self.read_slots();
        race(found, read.wait()).await;
    }

    /// download and verify every piece, then have the sink put the file in
    /// its place. returns where it was saved.
    pub async fn run(self: &Arc<Self>) -> Result<String> {
        info!("fetching {} ({} bytes)", self.name, self.manifest.size);
        self.find_first_peer().await;
        // unique enough to tell concurrent downloads apart in a peer's log.
        let id = unique_bytes();
        // in the background, so a slow or lost notice never holds up the pieces.
        spawn(self.clone().notify(Event::Started, id));
        let fetched = match race(self.fetch_pieces(), self.report_progress(id)).await {
            Either::Left(fetched) => fetched,
            Either::Right(()) => Ok(()),
        };
        self.stopped.raise();
        if let Err(err) = fetched {
            self.sink.discard().await;
            return Err(err);
        }
        let saved = self.sink.finish().await?;
        info!("downloaded {}", self.name);
        self.clone().notify(Event::Finished, id).await;
        Ok(saved)
    }
}

impl Pieces for Fetch {
    fn count(&self) -> u64 {
        self.total
    }

    fn size(&self) -> u64 {
        self.manifest.size
    }

    fn piece(&self, index: u64) -> BoxFuture<'_, Result<Option<Proven>>> {
        Box::pin(async move {
            let proof = {
                let held = self.held.lock().unwrap();
                held.0
                    .contains(&index)
                    .then(|| held.1.proof(index, self.total))
            };
            let Some(Some(proof)) = proof else {
                return Ok(None);
            };
            Ok(Some((self.sink.read(index).await?, proof)))
        })
    }
}

/// the percentage to report now that `done` of `total` pieces are in: Some
/// only for the piece that first reaches a new step, which always includes
/// the last piece.
fn progress_percent(done: u64, total: u64) -> Option<u64> {
    let step = |pieces: u64| pieces * PERCENT / total / PROGRESS_STEP_PERCENT;
    (step(done) > step(done - 1)).then(|| done * PERCENT / total)
}

/// the record key a share link or token names, and the name the link gives
/// the file, empty when it gives none. a link to a library names a record
/// too, but not one a file can be fetched from.
pub fn key_of(input: &str) -> Result<(RecordKey, String)> {
    match parse_link(input) {
        Link::File(token, name) => Ok((RecordKey::from_str(&token)?, name)),
        Link::Library(_) => Err(LIBRARY_LINK.into()),
    }
}

/// the record key of a file another library lists, which is its share
/// token: derived from its size and contents alone.
pub async fn file_key(api: &VeilidAPI, file: &FileEntry) -> Result<RecordKey> {
    let root = hash_from_hex(&file.root)?;
    record_key(api, &share_keys(blake3_hash, &root, file.size)).await
}

/// open a share's record for reading. a record this node has never seen
/// takes a network lookup, which is retried.
pub async fn open_record(context: &RoutingContext, key: &RecordKey) -> Result<()> {
    retry(|| async move {
        let opened = context.open_dht_record(key.clone(), None).await;
        Ok(opened.map(drop)?)
    })
    .await
}

/// read the manifest and hold it to the link: the record key is derived
/// from the size and contents, so a manifest that derives another key was
/// not written by someone sharing this file.
pub async fn read_manifest(
    api: &VeilidAPI,
    context: &RoutingContext,
    key: &RecordKey,
) -> Result<Manifest> {
    let manifest = decode_manifest(&read_subkey(context, key, MANIFEST_SUBKEY).await?)?;
    let root = hash_from_hex(&manifest.root)?;
    let keys = share_keys(blake3_hash, &root, manifest.size);
    if record_key(api, &keys).await? != *key {
        return Err("the link does not match the file shared there".into());
    }
    Ok(manifest)
}

/// how much longer to wait on the slots for names, `elapsed` milliseconds
/// into reading them, when the first name came at `first_heard`. None when
/// the reading is over.
fn names_wait(elapsed: u64, first_heard: Option<u64>) -> Option<Duration> {
    let whole = NAMES_WAIT.as_millis() as u64;
    let until = first_heard.map_or(whole, |at| whole.min(at + NAMES_GRACE.as_millis() as u64));
    (elapsed < until).then(|| Duration::from_millis(until - elapsed))
}

/// every name the peers in an open record's slots give its file, the most
/// common first, as far as the slots answer in time (see `NAMES_WAIT`).
pub async fn slot_names(context: &RoutingContext, key: &RecordKey) -> Vec<String> {
    let mut reads: FuturesUnordered<_> = slot_subkeys()
        .map(|subkey| read_slot(context, key, subkey))
        .collect();
    let (started, mut first_heard) = (now_ms(), None);
    let mut heard = Vec::new();
    while let Some(wait) = names_wait(now_ms() - started, first_heard) {
        let Some(Some(read)) = timeout(wait, reads.next()).await else {
            break;
        };
        let slot = read.ok().flatten();
        if let Some(slot) = slot.filter(|slot| !slot.is_stale(unix_now())) {
            first_heard.get_or_insert(now_ms() - started);
            heard.push(slot.name);
        }
    }
    common_names(heard.iter().map(String::as_str))
}

/// what to call the file in an open record: the name asked for, else the
/// one in its link, else the most common one in its slots, which are only
/// read when it comes to that.
pub async fn name_of(
    context: &RoutingContext,
    key: &RecordKey,
    asked: Option<&str>,
    link: &str,
) -> String {
    let mut name = chosen_name(asked, link, &[]);
    if name.is_empty() {
        name = chosen_name(None, "", &slot_names(context, key).await);
    }
    if name.is_empty() {
        name = FALLBACK_NAME.to_owned();
    }
    name
}

/// what the file behind a share link or token is, without downloading it.
pub async fn look_up(api: &VeilidAPI, context: &RoutingContext, input: &str) -> Result<Found> {
    let (key, name) = key_of(input)?;
    open_record(context, &key).await?;
    let read = join(read_manifest(api, context, &key), slot_names(context, &key));
    let (manifest, names) = read.await;
    context.close_dht_record(key.clone()).await?;
    let manifest = manifest?;
    Ok(Found {
        token: key.to_string(),
        size: manifest.size,
        root: manifest.root,
        name: chosen_name(None, &name, &names),
        names,
    })
}

async fn download(
    api: &VeilidAPI,
    context: &RoutingContext,
    (key, name): (&RecordKey, String),
    sinks: &dyn Sinks,
    concurrency: Option<usize>,
) -> Result<String> {
    let manifest = read_manifest(api, context, key).await?;
    let sink = sinks.create(&save_name(&name), manifest.size).await?;
    let fetch = Fetch::new(api, context, key, name, manifest, sink, None, concurrency)?;
    fetch.run().await
}

/// download the file behind a share link or token into a sink of `sinks`,
/// under `name` when one is given, verifying every piece against the merkle
/// root in the manifest. returns where it was saved.
pub async fn fetch(
    api: &VeilidAPI,
    context: &RoutingContext,
    input: &str,
    name: Option<&str>,
    sinks: &dyn Sinks,
    concurrency: Option<usize>,
) -> Result<String> {
    let (key, link_name) = key_of(input)?;
    open_record(context, &key).await?;
    let name = name_of(context, &key, name, &link_name).await;
    let outcome = download(api, context, (&key, name), sinks, concurrency).await;
    context.close_dht_record(key).await?;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    #[test]
    fn names_are_waited_for_until_one_comes_and_a_little_longer() {
        let (wait, grace) = (
            NAMES_WAIT.as_millis() as u64,
            NAMES_GRACE.as_millis() as u64,
        );
        let ms = Duration::from_millis;
        // with no name yet, the slow slot that holds one is waited out.
        assert_eq!(names_wait(0, None), Some(NAMES_WAIT));
        assert_eq!(names_wait(5_500, None), Some(ms(wait - 5_500)));
        assert_eq!(names_wait(wait, None), None);
        // once one has come, the others get a short while and no more.
        assert_eq!(names_wait(400, Some(400)), Some(NAMES_GRACE));
        assert_eq!(names_wait(1_000, Some(400)), Some(ms(grace - 600)));
        assert_eq!(names_wait(400 + grace, Some(400)), None);
        // and never past the whole wait.
        assert_eq!(names_wait(wait - 100, Some(wait - 200)), Some(ms(100)));
    }

    #[test]
    fn progress_is_reported_at_each_step_and_at_completion() {
        assert_eq!(progress_percent(33, 340), None);
        assert_eq!(progress_percent(34, 340), Some(10));
        assert_eq!(progress_percent(35, 340), None);
        assert_eq!(progress_percent(340, 340), Some(100));
        let reports = (1..=1000).filter_map(|done| progress_percent(done, 1000));
        assert_eq!(
            reports.collect::<Vec<_>>(),
            [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]
        );
    }

    #[test]
    fn a_small_download_reports_every_piece_that_reaches_a_step() {
        assert_eq!(progress_percent(1, 1), Some(100));
        let reports = (1..=3).map(|done| progress_percent(done, 3));
        assert_eq!(reports.collect::<Vec<_>>(), [Some(33), Some(66), Some(100)]);
    }

    #[test]
    fn a_peer_that_does_not_answer_is_asked_for_its_peers_again() {
        assert!(ask_again(1, true));
        assert!(ask_again(PEERS_ASK_ATTEMPTS - 1, true));
        assert!(!ask_again(PEERS_ASK_ATTEMPTS, true));
        assert!(!ask_again(1, false));
    }

    #[test]
    fn a_peer_is_dropped_only_when_it_fails_and_nobody_names_it() {
        let long_ago = NOW - FORGET_AFTER.as_secs() - 1;
        assert!(is_gone(PEER_STRIKES, long_ago, NOW));
        // still named by a slot or a peer: it may only be slow to come back.
        assert!(!is_gone(PEER_STRIKES, NOW - FORGET_AFTER.as_secs(), NOW));
        // delivering, however long since anyone named it.
        assert!(!is_gone(PEER_STRIKES - 1, long_ago, NOW));
    }

    #[test]
    fn the_first_retry_is_at_once_and_later_ones_are_spaced() {
        assert_eq!(round_delay(1, true), Duration::ZERO);
        assert_eq!(round_delay(2, true), 2 * RETRY_DELAY);
        assert_eq!(round_delay(99, true), RETRY_DELAY * MAX_RETRY_DELAY_FACTOR);
        // with nobody to ask there is nothing to hurry for.
        assert_eq!(round_delay(1, false), RETRY_DELAY);
    }

    #[test]
    fn rounds_fill_the_patience_for_a_piece_without_hammering() {
        // even if every round failed at once, the rounds a piece gets within
        // its patience are in the hundreds, not the thousands.
        let mut waited = Duration::ZERO;
        let rounds = (1..).take_while(|round| {
            waited += round_delay(*round, true);
            waited < PIECE_PATIENCE
        });
        assert!((60..300).contains(&rounds.count()));
    }

    #[test]
    fn pieces_start_at_different_peers() {
        assert_eq!(peer_order(0, &[0, 0, 0]), [0, 1, 2]);
        assert_eq!(peer_order(1, &[0, 0, 0]), [1, 2, 0]);
        assert_eq!(peer_order(5, &[0, 0, 0]), [2, 0, 1]);
        assert_eq!(peer_order(7, &[]), [0usize; 0]);
    }

    #[test]
    fn struck_peers_are_asked_last() {
        let strikes = [PEER_STRIKES, 0, PEER_STRIKES - 1, PEER_STRIKES + 1];
        assert_eq!(peer_order(0, &strikes), [1, 2, 0, 3]);
        assert_eq!(peer_order(3, &strikes), [1, 2, 3, 0]);
    }
}
