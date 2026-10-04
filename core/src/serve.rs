//! the engine: one veilid node and everything mudita does on it, for
//! whoever embeds the core. with libraries to share (DESIGN.md "libraries")
//! it shares what its host lists, each library as one of its own, meets
//! other people's libraries, downloads on request, and answers the control
//! methods of control.rs, which is all of `mudita serve` but its socket.
//! without them it is the node `send` and `fetch` run on, and what a
//! browser shares and downloads single files with.

use crate::control::{
    self, encode_response, nothing, uploads_of, Call, Discovered, Discoveries, Found,
    LibraryStatus, Page, Request, Saving, SearchResults, SharedFiles, Status, Upload, NOT_READY,
    UNKNOWN_METHOD,
};
use crate::downloads::{Downloads, Target};
use crate::fetch;
use crate::library::Libraries;
use crate::neighbors::{library_id, Known, Neighbors, Search};
use crate::node::Node;
use crate::owned::{self, checked_name, Owned};
use crate::peer::{Peer, Peers};
use crate::protocol::{
    decode_files_reply, encode_files_reply, encode_piece_request, hash_from_hex, is_error_reply,
    library_link_for, parse_link, FilesPage, Link, LINK_BASE,
};
use crate::rt::{race, sleep, Either, Flag};
use crate::shares::Shares;
use crate::store::{Files, Host, Sinks, Source, StateStore};
use crate::Result;
use futures_util::pin_mut;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::info;
use veilid_core::{RoutingContext, VeilidAPI, VeilidConfig};

// files that appear where the host looks are found this often, unasked.
const RESCAN_INTERVAL: Duration = Duration::from_secs(60);
// how often the lobby is read for libraries that have joined since.
const LOBBY_INTERVAL: Duration = Duration::from_secs(60);
const NOT_A_LIBRARY: &str = "this node shares no libraries";
const NO_PATHS: &str = "files are not named by path here";

/// settings only experiments and tests change.
#[derive(Default, Clone, Copy)]
pub struct Tuning {
    /// how many piece requests a download runs at once.
    pub concurrency: Option<usize>,
    /// the highest slot a peer will take, to fill a swarm with few peers.
    pub slot_limit: Option<u32>,
    /// answer the `probe` control method, by which a test asks one library
    /// for a piece of a file whoever serves it.
    pub probe: bool,
}

/// what an engine needs from its embedder to share libraries.
pub struct LibraryParts {
    pub host: Arc<dyn Host>,
    pub state: Arc<dyn StateStore>,
}

/// what whoever embeds an engine gives it. every part may be missing: with
/// none of them the engine still shares the files it is handed, looks links
/// up, and downloads into the sinks it is handed.
#[derive(Default)]
pub struct Embedder {
    /// what makes the engine share libraries.
    pub library: Option<LibraryParts>,
    /// where a download goes that does not say where.
    pub sinks: Option<Arc<dyn Sinks>>,
    /// files by path, for a client that shares or saves by one.
    pub files: Option<Arc<dyn Files>>,
}

/// what an engine is once its node can reach the network.
struct Live {
    api: VeilidAPI,
    context: RoutingContext,
    // one peer for single files, and one for each library besides.
    peers: Arc<Peers>,
    neighbors: Arc<Neighbors>,
    shares: Arc<Shares>,
    downloads: Arc<Downloads>,
    // only a node with libraries has folders of files of its own.
    library: Option<Arc<Libraries>>,
}

pub struct Engine {
    embedder: Embedder,
    tuning: Tuning,
    // set once: callers may come before the node is usable.
    live: OnceLock<Live>,
    // raised when the node has started, or never will.
    settled: Flag,
    // the latest search; starting one drops the one before.
    search: Mutex<Option<(u64, Arc<Search>)>>,
    next_search: AtomicU64,
}

// a library as `status` lists it while it has no link yet.
fn unpublished(owned: Owned) -> LibraryStatus {
    LibraryStatus {
        name: owned.name,
        folders: owned.folders,
        listed: owned.listed,
        token: None,
        link: None,
        files: 0,
        shared: 0,
    }
}

/// every shared folder but the one downloads go to: what a client that
/// knows of folders only shows.
fn folders_of(all: &[Owned], downloads: &str) -> Vec<String> {
    let folders = all.iter().flat_map(|owned| owned.folders.iter());
    let shared = folders.filter(|folder| *folder != downloads);
    shared.cloned().collect()
}

fn results_of(search: &Search) -> Result<Value> {
    Ok(serde_json::to_value(SearchResults {
        asked: search.asked(),
        done: search.done(),
        results: search.hits(),
    })?)
}

// meet the other libraries, and keep what is shared in line with what the
// host lists. never returns.
async fn keep_up(library: &Arc<Libraries>, neighbors: &Neighbors) {
    // the lobby is read again and again: a library that joins later is
    // found, and so is one whose slot the dht had not caught up on.
    let meeting = async {
        loop {
            neighbors.read_lobby().await;
            sleep(LOBBY_INTERVAL).await;
        }
    };
    let scanning = async {
        loop {
            library.rescan().await;
            sleep(RESCAN_INTERVAL).await;
        }
    };
    futures_util::join!(meeting, scanning);
}

impl Engine {
    /// an engine that is a library when its embedder gives it a library's
    /// parts, and a bare node for single files when not. it answers `call`
    /// at once, and does everything else once `run` has started its node.
    pub fn new(embedder: Embedder, tuning: Tuning) -> Arc<Engine> {
        Arc::new(Engine {
            embedder,
            tuning,
            live: OnceLock::new(),
            settled: Flag::default(),
            search: Mutex::default(),
            next_search: AtomicU64::new(1),
        })
    }

    fn live(&self) -> Result<&Live> {
        self.live.get().ok_or_else(|| NOT_READY.into())
    }

    fn library(&self) -> Result<&Arc<Libraries>> {
        let library = self.live()?.library.as_ref();
        library.ok_or_else(|| NOT_A_LIBRARY.into())
    }

    fn host(&self) -> Result<&Arc<dyn Host>> {
        let parts = self.embedder.library.as_ref().ok_or(NOT_A_LIBRARY)?;
        Ok(&parts.host)
    }

    fn files_by_path(&self) -> Result<&Arc<dyn Files>> {
        self.embedder.files.as_ref().ok_or_else(|| NO_PATHS.into())
    }

    /// wait until the node is usable. an error if it never will be.
    pub async fn ready(&self) -> Result<()> {
        self.settled.wait().await;
        self.live().map(drop)
    }

    /// share one file, in the background, for as long as the engine runs
    /// or until the share is stopped. returns the share's id, by which
    /// `shares` lists it.
    pub fn add_share(&self, source: Arc<dyn Source>) -> Result<u64> {
        Ok(self.live()?.shares.add(source))
    }

    /// the share token of a share, once it is hashed and has its slot. an
    /// error if it could not be shared.
    pub async fn share_token(&self, id: u64) -> Result<String> {
        self.live()?.shares.token(id).await
    }

    /// the downloads of this run, newest first, as `downloads` lists them.
    pub fn downloads(&self) -> Result<Vec<control::Download>> {
        Ok(self.live()?.downloads.views())
    }

    /// what the file behind a share link or token is, without downloading
    /// it.
    pub async fn look_up(&self, link: &str) -> Result<Found> {
        let live = self.live()?;
        fetch::look_up(&live.api, &live.context, link).await
    }

    /// download the file behind a share link or token into a sink of
    /// `sinks`, under `name` when one is given. returns where it was saved.
    pub async fn download(
        &self,
        link: &str,
        name: Option<&str>,
        sinks: &dyn Sinks,
    ) -> Result<String> {
        let live = self.live()?;
        let concurrency = self.tuning.concurrency;
        fetch::fetch(&live.api, &live.context, link, name, sinks, concurrency).await
    }

    // a page of another library's catalog as clients get it: every file
    // with the token to download it by.
    async fn page_of(&self, page: FilesPage) -> Result<Page> {
        Ok(Page {
            total: page.total,
            library: page.library,
            files: self.live()?.neighbors.listed(page.files).await,
        })
    }

    /// a page of the catalog a library link leads to. a link to one of
    /// this node's own libraries is answered from its own catalog, cut into
    /// pages the same way: its record is open here for writing, and is not
    /// to be reopened as a reader's.
    pub async fn browse(&self, link: &str, offset: u32) -> Result<Page> {
        let Link::Library(token) = parse_link(link) else {
            return Err("that is not a link to a library".into());
        };
        let live = self.live()?;
        let library = live.library.as_ref();
        if let Some((name, files)) = library.and_then(|library| library.own_catalog(&token)) {
            let page = encode_files_reply(&files, offset as usize, &name)?;
            return self.page_of(decode_files_reply(&page)?).await;
        }
        let route = live.neighbors.find_library(&token).await?;
        self.page_of(live.neighbors.catalog(&route, offset).await?)
            .await
    }

    // the node's own libraries as the host has them set out, each with its
    // link once the node is up and its record published.
    fn own_libraries(&self) -> Vec<LibraryStatus> {
        let library = self.live.get().and_then(|live| live.library.as_ref());
        let set_out = || {
            self.host()
                .map_or_else(|_| Vec::new(), |host| host.libraries())
        };
        library.map_or_else(
            || set_out().into_iter().map(unpublished).collect(),
            |library| library.statuses(),
        )
    }

    /// the downloads others are making from this node, or have made: of
    /// files shared on their own and of every library's alike.
    pub fn uploads(&self) -> Result<Vec<Upload>> {
        Ok(uploads_of(self.live()?.peers.uploads()))
    }

    // an engine that shares no libraries has no folders or links to show.
    fn status(&self) -> Result<Value> {
        let host = self.host().ok();
        let live = self.live.get();
        let library = live.and_then(|live| live.library.as_ref());
        let (files, shared) = library.map_or((0, 0), |library| library.counts());
        let downloads = host.map_or_else(String::new, |host| host.downloads());
        let set_out = host.map_or_else(Vec::new, |host| host.libraries());
        Ok(serde_json::to_value(Status {
            ready: live.is_some(),
            folders: folders_of(&set_out, &downloads),
            downloads,
            files,
            shared,
            discovered: live.map_or(0, |live| live.neighbors.routes().len()),
            uploading: self.uploads().map_or(0, |uploads| {
                uploads.iter().filter(|upload| upload.running()).count()
            }),
            libraries: self.own_libraries(),
        })?)
    }

    // set the libraries out as a call asks, for good, and share what they
    // hold now.
    async fn rearrange(&self, call: Call) -> Result<Value> {
        let downloads = self.host()?.downloads();
        let renamed = match &call {
            Call::RenameLibrary { name, to } => Some((name.clone(), checked_name(to)?)),
            _ => None,
        };
        let change = |all: &mut Vec<Owned>| match &call {
            Call::AddFolder { path, library } => match library {
                Some(name) => owned::add_folder(all, name, path),
                None => owned::add_loose_folder(all, path),
            },
            Call::RemoveFolder { path, library } => match library {
                Some(name) => owned::remove_folder(all, name, path, &downloads),
                None => owned::remove_loose_folder(all, path, &downloads),
            },
            Call::AddLibrary { name, listed } => owned::add(all, name, *listed),
            Call::RemoveLibrary(name) => owned::remove(all, name, &downloads),
            Call::RenameLibrary { name, to } => owned::rename(all, name, to),
            Call::SetLibrary { name, listed } => owned::set_listed(all, name, *listed),
            _ => Err(UNKNOWN_METHOD.into()),
        };
        let renamed = renamed.as_ref().map(|(from, to)| (&**from, &**to));
        self.library()?.rearrange(change, renamed).await?;
        self.status()
    }

    fn start_search(&self, query: &str) -> Result<Value> {
        self.library()?;
        let search = self.live()?.neighbors.search(query)?;
        let id = self.next_search.fetch_add(1, Ordering::Relaxed);
        *self.search.lock().unwrap() = Some((id, search));
        Ok(json!({"search": id}))
    }

    fn search_results(&self, wanted: Option<u64>) -> Result<Value> {
        let search = self.search.lock().unwrap();
        match (&*search, wanted) {
            (Some((id, search)), Some(wanted)) if *id == wanted => results_of(search),
            (Some((_, search)), None) => results_of(search),
            (_, Some(_)) => Err("no such search".into()),
            (None, None) => Err("no search yet".into()),
        }
    }

    async fn browse_known(&self, library: &str, offset: u32) -> Result<Value> {
        self.library()?;
        let neighbors = &self.live()?.neighbors;
        let route = neighbors.route_of(library).ok_or("no such library")?;
        let page = self.page_of(neighbors.catalog(&route, offset).await?);
        Ok(serde_json::to_value(page.await?)?)
    }

    // other people's libraries, as far as this node has discovered them.
    fn discover(&self) -> Result<Value> {
        self.library()?;
        let known = self.live()?.neighbors.known();
        let listed = |library: Known| Discovered {
            id: library_id(&library.route),
            name: library.name,
            seen: library.seen,
            files: library.files,
            link: library
                .token
                .map(|token| library_link_for(LINK_BASE, &token)),
        };
        Ok(serde_json::to_value(Discoveries {
            libraries: known.into_iter().map(listed).collect(),
        })?)
    }

    fn files(&self, offset: usize, limit: usize) -> Result<Value> {
        let (total, files) = self.library()?.files(offset, limit);
        Ok(serde_json::to_value(SharedFiles { total, files })?)
    }

    // to exactly the path asked for, where one is, or else to where the
    // embedder's downloads go, under the name asked for or the file's own.
    fn start_download(&self, target: Target, saving: Saving) -> Result<Value> {
        let sinks = match saving.path {
            Some(path) => Some(self.files_by_path()?.sink_at(&path)),
            None => None,
        };
        let id = self.live()?.downloads.start(target, saving.name, sinks)?;
        Ok(json!({"download": id}))
    }

    // what the library a link leads to answers when asked for the first
    // piece of a file: "piece", or its refusal, word for word. only where
    // the embedder's tuning turns it on, which only tests do.
    async fn probe(&self, link: &str, root: &str) -> Result<Value> {
        if !self.tuning.probe {
            return Err(UNKNOWN_METHOD.into());
        }
        let Link::Library(token) = parse_link(link) else {
            return Err("that is not a link to a library".into());
        };
        let neighbors = &self.live()?.neighbors;
        let route = neighbors.find_library(&token).await?;
        let request = encode_piece_request(&hash_from_hex(root)?, 0);
        let reply = neighbors.ask(&route, request).await?;
        let answer = match reply.split_first() {
            Some((_, refusal)) if is_error_reply(&reply) => String::from_utf8_lossy(refusal),
            _ => "piece".into(),
        };
        Ok(json!({"answer": answer}))
    }

    async fn add_share_at(&self, path: &str) -> Result<Value> {
        let live = self.live()?;
        let source = self.files_by_path()?.open(path).await?;
        Ok(json!({"share": live.shares.add(source)}))
    }

    async fn handle(&self, call: Call) -> Result<Value> {
        match call {
            Call::Status => self.status(),
            Call::Files { offset, limit } => self.files(offset, limit),
            Call::AddFolder { .. }
            | Call::RemoveFolder { .. }
            | Call::AddLibrary { .. }
            | Call::RemoveLibrary(_)
            | Call::RenameLibrary { .. }
            | Call::SetLibrary { .. } => self.rearrange(call).await,
            Call::Libraries => Ok(json!({"libraries": self.own_libraries()})),
            Call::Rescan => {
                self.library()?.rescan().await;
                Ok(nothing())
            }
            Call::Discover => self.discover(),
            Call::RefreshDiscover => {
                self.library()?;
                self.live()?.neighbors.read_lobby().await;
                Ok(nothing())
            }
            Call::Browse { library, offset } => self.browse_known(&library, offset).await,
            Call::BrowseLink { link, offset } => {
                Ok(serde_json::to_value(self.browse(&link, offset).await?)?)
            }
            Call::StartSearch(query) => self.start_search(&query),
            Call::PollSearch(id) => self.search_results(Some(id)),
            Call::LastSearch => self.search_results(None),
            Call::DownloadFile(file, saving) => self.start_download(Target::File(file), saving),
            Call::DownloadToken(token, saving) => self.start_download(Target::Token(token), saving),
            Call::Downloads => Ok(json!({"downloads": self.downloads()?})),
            Call::CancelDownload(id) => self.live()?.downloads.cancel(id).map(|()| nothing()),
            Call::Shares => Ok(json!({"shares": self.live()?.shares.views()})),
            Call::Uploads => Ok(json!({"uploads": self.uploads()?})),
            Call::AddShare(path) => self.add_share_at(&path).await,
            Call::StopShare(id) => self.live()?.shares.stop(id).map(|()| nothing()),
            Call::Lookup(link) => Ok(serde_json::to_value(self.look_up(&link).await?)?),
            Call::Probe { link, root } => self.probe(&link, &root).await,
        }
    }

    /// answer one control request (see control.rs): one line of json in,
    /// one line out. a line that is not a request has no id to answer
    /// under, so it is answered under zero.
    pub async fn call(&self, line: &str) -> String {
        match serde_json::from_str::<Request>(line) {
            Ok(request) => {
                let id = request.id;
                let outcome = match Call::of(request) {
                    Ok(call) => self.handle(call).await,
                    Err(err) => Err(err),
                };
                encode_response(id, outcome)
            }
            Err(err) => encode_response(0, Err(format!("bad request: {err}").into())),
        }
    }

    // what the engine is on a node that has started.
    fn come_alive(&self, node: &Node) -> &Live {
        let (api, context) = (&node.api, &node.context);
        let (peers, slots) = (Peers::new(), self.tuning.slot_limit);
        let neighbors = Neighbors::new(api, context, &peers);
        let library = self.embedder.library.as_ref().map(|parts| {
            let (host, state) = (&parts.host, &parts.state);
            Libraries::new(api, context, &peers, slots, host, state, &neighbors)
        });
        // single files are served over a peer that is no library. so are
        // the downloads of a node that has none; with libraries they are
        // served over the library they are saved in.
        let plain = Peer::new(api, context, &peers, false, slots);
        let downloading = library
            .as_ref()
            .and_then(|library| library.downloads_peer());
        let downloads = Downloads::new(
            api,
            context,
            &downloading.unwrap_or_else(|| plain.clone()),
            &neighbors,
            library.clone(),
            self.embedder.sinks.clone(),
            self.tuning.concurrency,
        );
        self.live.get_or_init(|| Live {
            api: api.clone(),
            context: context.clone(),
            shares: Shares::new(api, context, &plain),
            downloads,
            peers,
            neighbors,
            library,
        })
    }

    // answer requests and, as a library, keep up with the files and the
    // other libraries. never returns.
    async fn work(&self, live: &Live, node: &Node) {
        let serving = live.peers.run(&node.calls, &node.dead_routes);
        let Some(library) = &live.library else {
            return serving.await;
        };
        futures_util::join!(serving, keep_up(library, &live.neighbors));
    }

    /// start a node from `config` and do the engine's work on it until
    /// `stop` is done, then give up every slot and shut the node down.
    pub async fn run(&self, config: VeilidConfig, stop: impl Future<Output = ()>) -> Result<()> {
        pin_mut!(stop);
        let started = race(Node::start(config), &mut stop).await;
        let node = match started {
            Either::Left(Ok(node)) => node,
            Either::Left(Err(err)) => {
                self.settled.raise();
                return Err(err);
            }
            Either::Right(()) => {
                self.settled.raise();
                return Ok(());
            }
        };
        let live = self.come_alive(&node);
        self.settled.raise();
        race(self.work(live, &node), stop).await;
        if live.library.is_some() {
            info!("leaving the swarm");
        }
        live.peers.leave().await;
        node.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{decode_response, encode_request};
    use futures_util::FutureExt;

    // an engine whose node was never started answers at once, so nothing
    // here waits.
    fn answer(engine: &Engine, method: &str, params: Value) -> Result<Value> {
        let line = encode_request(1, method, params);
        let reply = engine.call(&line).now_or_never().expect("no waiting");
        decode_response(&reply).unwrap().1
    }

    #[test]
    fn an_engine_without_libraries_has_a_status_all_the_same() {
        let engine = Engine::new(Embedder::default(), Tuning::default());
        let status = answer(&engine, "status", nothing()).unwrap();
        let expected = json!({"ready": false, "folders": [], "downloads": "", "files": 0, "shared": 0, "discovered": 0, "uploading": 0,"libraries": []});
        assert_eq!(status, expected);
        let own = answer(&engine, "libraries", nothing()).unwrap();
        assert_eq!(own, json!({"libraries": []}));
    }

    #[test]
    fn the_folders_shown_are_every_librarys_but_the_download_folder() {
        let library = |name: &str, folders: &[&str]| Owned {
            name: name.into(),
            folders: folders.iter().map(|folder| folder.to_string()).collect(),
            listed: true,
        };
        let all = [
            library("tunes", &["/music", "/more"]),
            library("downloads", &["/d", "/extra"]),
        ];
        assert_eq!(folders_of(&all, "/d"), ["/music", "/more", "/extra"]);
        assert!(folders_of(&[], "/d").is_empty());
    }

    #[test]
    fn a_node_without_libraries_has_none_to_set_out() {
        let engine = Engine::new(Embedder::default(), Tuning::default());
        for (method, params) in [
            ("library.add", json!({"name": "tunes"})),
            ("library.remove", json!({"name": "tunes"})),
            ("library.rename", json!({"name": "tunes", "to": "music"})),
            ("library.set", json!({"name": "tunes", "listed": false})),
            ("folders.add", json!({"path": "/music", "library": "tunes"})),
        ] {
            let refused = answer(&engine, method, params).unwrap_err();
            assert_eq!(refused.to_string(), NOT_A_LIBRARY, "{method}");
        }
    }

    #[test]
    fn a_probe_is_answered_only_where_tests_turn_it_on() {
        let asked = json!({"link": "library/VLD0:a:b", "root": "ab".repeat(32)});
        let engine = Engine::new(Embedder::default(), Tuning::default());
        let refused = answer(&engine, "probe", asked.clone()).unwrap_err();
        assert_eq!(refused.to_string(), UNKNOWN_METHOD);
        let tuning = Tuning {
            probe: true,
            ..Default::default()
        };
        let engine = Engine::new(Embedder::default(), tuning);
        let refused = answer(&engine, "probe", asked).unwrap_err();
        assert_eq!(refused.to_string(), NOT_READY);
    }

    #[test]
    fn what_needs_the_network_says_so_until_the_node_is_up() {
        let engine = Engine::new(Embedder::default(), Tuning::default());
        let link = json!({"link": "VLD0:a:b"});
        for (method, params) in [
            ("shares", nothing()),
            ("uploads", nothing()),
            ("downloads", nothing()),
            ("lookup", link.clone()),
            ("browse", json!({"link": "library/VLD0:a:b"})),
            ("download", json!({"token": "VLD0:a:b"})),
            ("share.stop", json!({"id": 1})),
            ("download.cancel", json!({"id": 1})),
        ] {
            let refused = answer(&engine, method, params).unwrap_err();
            assert_eq!(refused.to_string(), NOT_READY, "{method}");
        }
    }

    #[test]
    fn an_embedder_without_paths_does_not_share_or_save_by_one() {
        let engine = Engine::new(Embedder::default(), Tuning::default());
        let by_path = json!({"token": "VLD0:a:b", "path": "/tmp/out.bin"});
        let refused = answer(&engine, "download", by_path).unwrap_err();
        assert_eq!(refused.to_string(), NO_PATHS);
    }

    #[test]
    fn a_link_to_a_file_is_not_browsed() {
        let engine = Engine::new(Embedder::default(), Tuning::default());
        let browsed = engine.browse("VLD0:a:b", 0).now_or_never().unwrap();
        assert!(browsed.is_err_and(|err| err.to_string().contains("not a link to a library")));
    }
}
