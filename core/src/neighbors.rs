//! the other libraries this one knows of, and what it asks them (DESIGN.md
//! "libraries"): their catalogs, a search across them, and whom they know.
//! libraries are learned from the lobby, from the slots of files being
//! downloaded, and from each other.

use crate::control::{Hit, Listed};
use crate::fetch::{file_key, open_record};
use crate::merkle::blake3_hash;
use crate::node::read_slots;
use crate::peer::{Peer, Peers, Pieces, Proven, Share};
use crate::protocol::{
    common_names, decode_files_reply, decode_peers_reply, decode_slot, encode_catalog_request,
    encode_peers_request, encode_search_request, share_keys, to_hex, FileEntry, FilesPage, Slot,
    LIBRARY_SLOT_SUBKEY, LOBBY_ROOT, SEARCH_PEERS,
};
use crate::rt::{retry_times, spawn, unix_now};
use crate::seed::{open_share, JOIN_ATTEMPTS};
use crate::store::BoxFuture;
use crate::Result;
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info};
use veilid_core::{RecordKey, RoutingContext, Target, VeilidAPI};

// enough to widen a search well past the lobby without growing forever.
const NEIGHBORS_MAX: usize = 128;
// libraries asked side by side during one search.
const SEARCH_CONCURRENCY: usize = 4;
// requests time out now and then, so one is tried this often before the
// library is given up on.
const ASK_ATTEMPTS: u32 = 3;
const LIBRARY_OFFLINE: &str = "that library is offline";
// a library has no name: this much of a hash of its route tells two apart.
const ID_BYTES: usize = 8;

/// the lobby is a record like any share's, but nothing is served from it.
struct NoPieces;

impl Pieces for NoPieces {
    fn count(&self) -> u64 {
        0
    }

    fn size(&self) -> u64 {
        0
    }

    fn piece(&self, _index: u64) -> BoxFuture<'_, Result<Option<Proven>>> {
        Box::pin(async { Ok(None) })
    }
}

/// a file a search found: one whatever the libraries that have it call it.
struct Match {
    // as the first library to answer listed it.
    file: FileEntry,
    token: String,
    // the name in every listing of it, so as often as it is listed.
    heard: Vec<String>,
    sources: usize,
    // what the libraries that have it are called, as each answered.
    libraries: Vec<String>,
}

impl Match {
    fn hit(&self) -> Hit {
        let names = common_names(self.heard.iter().map(String::as_str));
        let name = names.first().cloned().unwrap_or_default();
        Hit {
            file: FileEntry {
                name,
                ..self.file.clone()
            },
            token: self.token.clone(),
            names,
            sources: self.sources,
            libraries: self.libraries.clone(),
        }
    }
}

/// one search: the hits so far, in the order they were first found.
#[derive(Default)]
pub struct Search {
    hits: Mutex<Vec<Match>>,
    asked: AtomicUsize,
    done: AtomicBool,
}

impl Search {
    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().iter().map(Match::hit).collect()
    }

    /// libraries that have answered or failed so far.
    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

/// count one library's files, each with its share token, into the hits. a
/// file is its size and contents, as a token is: a library that has it
/// under two names is still one source of it.
fn add_hits(hits: &mut Vec<Match>, files: Vec<Listed>, library: &str) {
    let mut counted = HashSet::new();
    // a library without a name is a source like any other, with no name
    // to list.
    let named = |fresh: bool| (fresh && !library.is_empty()).then(|| library.to_owned());
    for Listed { file, token } in files {
        let fresh = counted.insert(token.clone());
        match hits.iter_mut().find(|hit| hit.token == token) {
            Some(hit) => {
                hit.heard.push(file.name);
                hit.sources += usize::from(fresh);
                hit.libraries.extend(named(fresh));
            }
            None => hits.push(Match {
                heard: vec![file.name.clone()],
                file,
                token,
                sources: 1,
                libraries: named(true).into_iter().collect(),
            }),
        }
    }
}

/// a library heard of, and what is known of it.
#[derive(Debug, PartialEq, Clone)]
pub struct Known {
    pub route: Vec<u8>,
    /// what its owner calls it. None until it has answered once, or a
    /// slot of its that says so was read.
    pub name: Option<String>,
    /// when there last was evidence that it is alive, in unix seconds: it
    /// answered, or a fresh slot of its was read. None for one that only
    /// another library has spoken of.
    pub seen: Option<u64>,
    /// how many files its catalog has, once a page of it has been seen.
    pub files: Option<usize>,
    /// the token of its record, where it was reached by its link: the
    /// lobby says a library's route, and not where its record is.
    pub token: Option<String>,
}

/// put a library first among those known, keeping what is known of it and
/// dropping the oldest past the cap.
fn remember(known: &mut Vec<Known>, route: Vec<u8>) -> &mut Known {
    let held = known.iter().position(|library| library.route == route);
    let library = match held {
        Some(at) => known.remove(at),
        None => Known {
            route,
            name: None,
            seen: None,
            files: None,
            token: None,
        },
    };
    known.insert(0, library);
    known.truncate(NEIGHBORS_MAX);
    &mut known[0]
}

/// what a library is called towards clients: its route changes over time
/// and is too long to show.
pub fn library_id(route: &[u8]) -> String {
    to_hex(&blake3_hash(route)[..ID_BYTES])
}

pub struct Neighbors {
    api: VeilidAPI,
    context: RoutingContext,
    // this node's own peers, whose routes are no news.
    peers: Arc<Peers>,
    // most recently heard of first.
    known: Mutex<Vec<Known>>,
    lobby: OnceLock<RecordKey>,
    // one opening of the lobby's record at a time.
    opening: async_lock::Mutex<()>,
    // the libraries in the lobby's slots as last read, in slot order.
    in_lobby: Mutex<Vec<Vec<u8>>>,
}

/// the routes of the libraries holding fresh lobby slots, in slot order.
fn lobby_routes(slots: &[(u32, Result<Option<Slot>>)], now: u64) -> Vec<Vec<u8>> {
    let held = slots
        .iter()
        .filter_map(|(_, slot)| slot.as_ref().ok()?.as_ref());
    let libraries = held.filter(|slot| slot.library && !slot.is_stale(now));
    libraries.map(|slot| slot.route.clone()).collect()
}

impl Neighbors {
    pub fn new(api: &VeilidAPI, context: &RoutingContext, peers: &Arc<Peers>) -> Arc<Neighbors> {
        Arc::new(Neighbors {
            api: api.clone(),
            context: context.clone(),
            peers: peers.clone(),
            known: Mutex::default(),
            lobby: OnceLock::new(),
            opening: Default::default(),
            in_lobby: Mutex::default(),
        })
    }

    /// what the lobby said when it was last read: what a library tells
    /// whoever asks it which libraries there are.
    pub fn in_lobby(&self) -> Vec<Vec<u8>> {
        self.in_lobby.lock().unwrap().clone()
    }

    /// the libraries heard of, most recently first.
    pub fn known(&self) -> Vec<Known> {
        self.known.lock().unwrap().clone()
    }

    pub fn routes(&self) -> Vec<Vec<u8>> {
        let route = |library: Known| library.route;
        self.known().into_iter().map(route).collect()
    }

    /// the route of the library a client names by id.
    pub fn route_of(&self, id: &str) -> Option<Vec<u8>> {
        self.routes()
            .into_iter()
            .find(|route| library_id(route) == id)
    }

    // remember the library reachable at `route`, and note something about
    // it. our own routes are not news.
    fn note(&self, route: &[u8], about: impl FnOnce(&mut Known)) {
        if route.is_empty() || self.peers.own_routes().iter().any(|own| own == route) {
            return;
        }
        about(remember(&mut self.known.lock().unwrap(), route.to_vec()));
    }

    /// another library says there is a library at `route`. that is worth
    /// trying, and no evidence that it is alive.
    pub fn learn(&self, route: Vec<u8>) {
        self.note(&route, |_| {});
    }

    /// the library at `route` is alive just now: it answered, or a fresh
    /// slot naming it was read.
    pub fn saw(&self, route: &[u8]) {
        self.note(route, |library| library.seen = Some(unix_now()));
    }

    // a files reply came from the library at `route`, which says its name.
    fn named(&self, route: &[u8], name: &str) {
        self.note(route, |library| library.name = Some(name.to_owned()));
    }

    // a fresh slot of a library was read: it is alive, and the slot says
    // what it is called, unless it has no name.
    fn met(&self, slot: &Slot) {
        self.saw(&slot.route);
        if !slot.name.is_empty() {
            self.named(&slot.route, &slot.name);
        }
    }

    fn forget(&self, route: &[u8]) {
        let mut known = self.known.lock().unwrap();
        known.retain(|library| library.route != route);
    }

    /// the lobby's record, where libraries first meet, open on this node:
    /// to read it, and for the libraries here that are listed to take a
    /// slot in.
    pub async fn open_lobby(&self) -> Result<RecordKey> {
        let _opening = self.opening.lock().await;
        if let Some(key) = self.lobby.get() {
            return Ok(key.clone());
        }
        let keys = share_keys(blake3_hash, &LOBBY_ROOT, 0);
        let key = open_share(&self.api, &self.context, &keys).await?;
        Ok(self.lobby.get_or_init(|| key).clone())
    }

    /// take a slot in the lobby for the library `peer` is, which is what
    /// lists it. a full lobby can still be read. the share returned is
    /// what `peer` gives up to leave the lobby again.
    pub async fn join_lobby(&self, peer: &Peer) -> Result<Arc<Share>> {
        let key = self.open_lobby().await?;
        // the peer writes the library's name into a lobby slot, not a file's.
        let (pieces, root) = (Arc::new(NoPieces), LOBBY_ROOT);
        peer.add(key, root, "", pieces, true, JOIN_ATTEMPTS).await
    }

    /// learn the libraries holding lobby slots right now.
    pub async fn read_lobby(&self) {
        let Ok(key) = self.open_lobby().await else {
            return;
        };
        let (now, before) = (unix_now(), self.routes().len());
        let slots = read_slots(&self.context, &key).await;
        *self.in_lobby.lock().unwrap() = lobby_routes(&slots, now);
        for (_, slot) in slots {
            if let Ok(Some(slot)) = slot {
                if slot.library && !slot.is_stale(now) {
                    self.met(&slot);
                }
            }
        }
        let known = self.routes().len();
        if known != before {
            info!("{known} libraries known");
        }
    }

    // one request to the library at `route`, tried a few times: the first
    // request over a route often times out. the route is imported each
    // time, because veilid forgets one after a request over it does.
    pub async fn ask(&self, route: &[u8], request: Vec<u8>) -> Result<Vec<u8>> {
        let request = &request;
        let reply = retry_times(ASK_ATTEMPTS, || async move {
            let target = Target::RouteId(self.api.import_remote_private_route(route.to_vec())?);
            Ok(self.context.app_call(target, request.clone()).await?)
        })
        .await?;
        // it answered, whatever it said.
        self.saw(route);
        Ok(reply)
    }

    /// the route of the library a library token names, read from the record
    /// the library keeps current. it is remembered like any library heard
    /// of. the record is closed again: nothing else here reads it.
    pub async fn find_library(&self, token: &str) -> Result<Vec<u8>> {
        let key = RecordKey::from_str(token)?;
        open_record(&self.context, &key).await?;
        let read = self
            .context
            .get_dht_value(key.clone(), LIBRARY_SLOT_SUBKEY, true);
        let value = read.await;
        let _ = self.context.close_dht_record(key).await;
        let slot = value?.and_then(|value| decode_slot(value.data()));
        match slot {
            Some(slot) if !slot.is_stale(unix_now()) => {
                self.met(&slot);
                let token = Some(token.to_owned());
                self.note(&slot.route, |library| library.token = token);
                Ok(slot.route)
            }
            _ => Err(LIBRARY_OFFLINE.into()),
        }
    }

    // a files request to the library at `route`, whose answer also says
    // what the library is called.
    async fn ask_for_files(&self, route: &[u8], request: Vec<u8>) -> Result<FilesPage> {
        let page = decode_files_reply(&self.ask(route, request).await?)?;
        self.named(route, &page.library);
        Ok(page)
    }

    /// a page of the catalog of the library at `route`.
    pub async fn catalog(&self, route: &[u8], offset: u32) -> Result<FilesPage> {
        let request = encode_catalog_request(offset);
        let page = self.ask_for_files(route, request).await?;
        self.note(route, |library| library.files = Some(page.total));
        Ok(page)
    }

    /// files as another library lists them, each with its share token. one
    /// whose root is no hash cannot be downloaded, and is left out.
    pub async fn listed(&self, files: Vec<FileEntry>) -> Vec<Listed> {
        let mut listed = Vec::new();
        for file in files {
            if let Ok(key) = file_key(&self.api, &file).await {
                let token = key.to_string();
                listed.push(Listed { file, token });
            }
        }
        listed
    }

    /// ask the libraries known for files matching `query`, and each for the
    /// libraries it knows, widening until SEARCH_PEERS have been asked or
    /// none are left. the search fills in as answers arrive.
    pub fn search(self: &Arc<Self>, query: &str) -> Result<Arc<Search>> {
        let request = encode_search_request(query)?;
        let search = Arc::new(Search::default());
        spawn(self.clone().run_search(search.clone(), request));
        Ok(search)
    }

    async fn run_search(self: Arc<Self>, search: Arc<Search>, request: Vec<u8>) {
        let mut asked: HashSet<Vec<u8>> = HashSet::new();
        let mut asking = FuturesUnordered::new();
        loop {
            while asking.len() < SEARCH_CONCURRENCY && asked.len() < SEARCH_PEERS {
                let Some(route) = self
                    .routes()
                    .into_iter()
                    .find(|route| !asked.contains(route))
                else {
                    break;
                };
                asked.insert(route.clone());
                let (neighbors, search, request) = (&self, &search, request.clone());
                asking.push(async move { neighbors.search_one(search, &route, request).await });
            }
            if asking.next().await.is_none() {
                break;
            }
            search.asked.fetch_add(1, Ordering::Relaxed);
        }
        search.done.store(true, Ordering::Relaxed);
    }

    // a library that fails or turns out not to be one is forgotten; if it is
    // still around, the lobby or another library will name it again.
    async fn search_one(&self, search: &Search, route: &[u8], request: Vec<u8>) {
        let answered = async {
            let page = self.ask_for_files(route, request).await?;
            let found = self.listed(page.files).await;
            add_hits(&mut search.hits.lock().unwrap(), found, &page.library);
            // asked as nobody: a library that is listed is in the lobby,
            // and one that is not does not say where it is.
            let known = self.ask(route, encode_peers_request(&[])).await?;
            decode_peers_reply(&known)
        };
        match answered.await {
            Ok(known) => known.into_iter().for_each(|route| self.learn(route)),
            Err(err) => {
                debug!("library did not answer: {err}");
                self.forget(route);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::HASH_BYTES;

    // a file as a library lists it: the token stands for its contents.
    fn file(name: &str, token: &str) -> Listed {
        let file = FileEntry {
            name: name.to_owned(),
            size: 1,
            root: "ab".repeat(HASH_BYTES),
            folder: String::new(),
        };
        let token = token.to_owned();
        Listed { file, token }
    }

    fn found(hits: &[Match]) -> Vec<(String, Vec<String>, String, usize)> {
        let shown = |hit: Hit| (hit.file.name, hit.names, hit.token, hit.sources);
        hits.iter().map(|hit| shown(hit.hit())).collect()
    }

    #[test]
    fn a_file_found_twice_is_one_hit_with_two_sources() {
        let mut hits = Vec::new();
        add_hits(&mut hits, vec![file("a", "x"), file("b", "y")], "one");
        add_hits(&mut hits, vec![file("b", "y"), file("b", "z")], "two");
        let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
        let expected: Vec<(String, Vec<String>, String, usize)> = vec![
            ("a".into(), names(&["a"]), "x".into(), 1),
            ("b".into(), names(&["b"]), "y".into(), 2),
            ("b".into(), names(&["b"]), "z".into(), 1),
        ];
        assert_eq!(found(&hits), expected);
        // each says which libraries have it, by what they are called.
        let libraries: Vec<Vec<String>> = hits.iter().map(|hit| hit.hit().libraries).collect();
        assert_eq!(
            libraries,
            [names(&["one"]), names(&["one", "two"]), names(&["two"])]
        );
    }

    #[test]
    fn a_file_under_several_names_is_one_hit_called_by_the_most_common() {
        let mut hits = Vec::new();
        add_hits(&mut hits, vec![file("old.bin", "x")], "");
        add_hits(&mut hits, vec![file("new.bin", "x")], "shelf");
        // one library with it twice is one source, and both names count.
        let twice = vec![file("new.bin", "x"), file("copy.bin", "x")];
        add_hits(&mut hits, twice, "attic");
        // a library is named once, and one without a name not at all.
        assert_eq!(hits[0].hit().libraries, ["shelf", "attic"]);
        let [(name, names, _, sources)] = &found(&hits)[..] else {
            panic!("one file is one hit");
        };
        assert_eq!((name.as_str(), *sources), ("new.bin", 3));
        assert_eq!(names, &["new.bin", "copy.bin", "old.bin"]);
    }

    #[test]
    fn the_newest_route_comes_first_and_the_oldest_falls_off() {
        let mut known = Vec::new();
        for n in 0..NEIGHBORS_MAX as u32 + 2 {
            remember(&mut known, n.to_be_bytes().to_vec());
        }
        assert_eq!(known.len(), NEIGHBORS_MAX);
        assert_eq!(known[0].route, (NEIGHBORS_MAX as u32 + 1).to_be_bytes());
        let oldest = 0u32.to_be_bytes();
        assert!(!known.iter().any(|library| library.route == oldest));
        // hearing of a known one again moves it to the front.
        let again = known[5].route.clone();
        remember(&mut known, again.clone());
        assert_eq!((known.len(), &known[0].route), (NEIGHBORS_MAX, &again));
    }

    #[test]
    fn what_is_known_of_a_library_survives_hearing_of_it_again() {
        let mut known = Vec::new();
        let (first, second) = (vec![1], vec![2]);
        // one only spoken of by another library has not been seen.
        assert_eq!(remember(&mut known, first.clone()).seen, None);
        remember(&mut known, second.clone()).seen = Some(100);
        remember(&mut known, second.clone()).name = Some("the shelf".into());
        // being spoken of again changes neither its name nor when it was seen.
        remember(&mut known, first.clone());
        let again = remember(&mut known, second.clone());
        assert_eq!(
            (again.name.as_deref(), again.seen),
            (Some("the shelf"), Some(100))
        );
        again.seen = Some(160);
        assert_eq!(known[0].seen, Some(160));
        assert_eq!((known[1].name.clone(), known[1].seen), (None, None));
    }

    #[test]
    fn what_is_told_of_the_lobby_is_its_fresh_libraries_in_slot_order() {
        const NOW: u64 = 1_000_000;
        let slot = |route: u8, library: bool, time: u64| {
            Ok(Some(Slot {
                complete: true,
                library,
                time,
                name: String::new(),
                route: vec![route],
            }))
        };
        let slots = vec![
            (1, slot(5, true, NOW)),
            (2, Ok(None)),
            (3, slot(3, false, NOW)),
            (
                4,
                slot(4, true, NOW - crate::protocol::SLOT_STALE_SECONDS - 1),
            ),
            (5, Err("unreachable".into())),
            (6, slot(1, true, NOW)),
        ];
        assert_eq!(lobby_routes(&slots, NOW), [vec![5], vec![1]]);
    }

    #[test]
    fn a_library_id_is_sixteen_hex_characters_of_its_route() {
        let id = library_id(&[1, 2, 3]);
        assert_eq!(id.len(), 2 * ID_BYTES);
        assert_eq!(id, to_hex(&blake3_hash(&[1, 2, 3])[..ID_BYTES]));
        assert_ne!(id, library_id(&[1, 2, 4]));
    }
}
