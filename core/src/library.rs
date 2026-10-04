//! the libraries of a node and the files in them. every file is hashed
//! once, shared like any single file (it has a link, and `fetch` and the
//! app can download it), and listed in the catalog of the library it is
//! in, which others browse and search. which files those are, and in which
//! library each is, is the embedder's to say: `mudita serve` lists the
//! folders in its config. every library is one of its own to the network
//! (DESIGN.md "a node's libraries"), with its files served over its own
//! peer, and nothing there says which libraries are one node's.

use crate::control::{LibraryStatus, SharedFile};
use crate::library_record::{inherit_kept, move_kept, open_library_record};
use crate::merkle::blake3_hash;
use crate::neighbors::Neighbors;
use crate::owned::{keeps_downloads, validate, Owned};
use crate::peer::{LibraryIndex, Peer, Peers, Share};
use crate::protocol::{
    library_link_for, link_for, piece_count, to_hex, FileEntry, Hash, HASH_BYTES, LINK_BASE,
};
use crate::rt::{sleep, spawn};
use crate::seed::{hash_leaves, manifest_of, share_file, JOIN_ATTEMPTS};
use crate::store::{Host, Source, StateStore};
use crate::units::Unit;
use crate::Result;
use async_lock::Semaphore;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};
use veilid_core::{RoutingContext, VeilidAPI};

// each new file reads its record's slots before claiming one, which takes
// seconds, so a few are published side by side.
const SHARE_CONCURRENCY: usize = 4;
const LEAVES_FOLDER: &str = "leaves";
// what kept leaves start with: the file's size and modified time.
const STAMP_BYTES: usize = 16;
// how long to wait before trying again to publish a library's record.
const RECORD_RETRY: Duration = Duration::from_secs(30);

/// what a file looked like when it was listed: its size and when it was
/// last modified, in milliseconds. a file that still looks so has not
/// changed.
pub type Stamp = (u64, u64);

/// a file the embedder found to share.
#[derive(Clone)]
pub struct Found {
    /// what tells this file from every other one listed, and names it to
    /// the owner: its path, where there are paths.
    pub id: String,
    pub stamp: Stamp,
    /// the library it is in, by name.
    pub library: String,
    /// where it is inside its library, '/'-separated: empty at the top.
    /// the library's catalog shows it there.
    pub folder: String,
    pub source: Arc<dyn Source>,
}

enum State {
    Hashing,
    Publishing,
    Shared(Arc<Share>),
    Failed(String),
}

struct Entry {
    stamp: Stamp,
    name: String,
    // the library it is shared in, and where inside it.
    unit: Arc<Unit>,
    folder: String,
    // pieces hashed so far.
    hashed: AtomicU64,
    state: Mutex<State>,
}

pub struct Libraries {
    api: VeilidAPI,
    context: RoutingContext,
    peers: Arc<Peers>,
    slot_limit: Option<u32>,
    host: Arc<dyn Host>,
    state: Arc<dyn StateStore>,
    neighbors: Arc<Neighbors>,
    // the libraries shared just now.
    units: Mutex<Vec<Arc<Unit>>>,
    // held while the libraries are set out anew, and while what is shared
    // is brought in line with them: one of those at a time.
    arranging: async_lock::Mutex<()>,
    // the kept records are read and written together, by one at a time.
    records: async_lock::Mutex<()>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    // the files in the order they were last listed, which is the order
    // their owner is shown them in.
    order: Mutex<Vec<String>>,
    publishing: Arc<Semaphore>,
}

/// one library's files and the libraries in the lobby, as its peer hands
/// them to callers: nothing of any other library of the node.
struct Index {
    libraries: Arc<Libraries>,
    unit: Arc<Unit>,
}

impl LibraryIndex for Index {
    fn name(&self) -> String {
        self.unit.name()
    }

    fn files(&self) -> Vec<FileEntry> {
        self.libraries.catalog_of(&self.unit)
    }

    fn lobby(&self) -> Vec<Vec<u8>> {
        self.libraries.neighbors.in_lobby()
    }
}

/// whether what is listed for a file is still what is shared of it: the
/// same bytes, in the same library, at the same place in it.
fn unchanged(listed: Option<&Found>, stamp: Stamp, library: &str, folder: &str) -> bool {
    listed
        .is_some_and(|file| file.stamp == stamp && file.library == library && file.folder == folder)
}

fn leaves_name(id: &str) -> String {
    format!("{LEAVES_FOLDER}/{}", to_hex(&blake3_hash(id.as_bytes())))
}

fn stamp_bytes((size, modified): Stamp) -> Vec<u8> {
    [size.to_be_bytes(), modified.to_be_bytes()].concat()
}

/// the leaf hashes kept for a file, if it still looks as it did then.
pub async fn kept_leaves(state: &dyn StateStore, id: &str, stamp: Stamp) -> Option<Vec<Hash>> {
    let kept = state.get(&leaves_name(id)).await?;
    let (written, leaves) = kept.split_at_checked(STAMP_BYTES)?;
    let whole = leaves.len() == piece_count(stamp.0) as usize * HASH_BYTES;
    let hashes = leaves.chunks_exact(HASH_BYTES);
    (written == stamp_bytes(stamp) && whole)
        .then(|| hashes.map(|h| h.try_into().unwrap()).collect())
}

/// keep a file's leaf hashes, so that it is hashed once.
pub async fn keep_leaves(
    state: &dyn StateStore,
    id: &str,
    stamp: Stamp,
    leaves: &[Hash],
) -> Result<()> {
    let kept = [stamp_bytes(stamp), leaves.concat()].concat();
    state.put(&leaves_name(id), kept).await
}

/// the order the catalog is paged in: by folder, then name, then root.
fn catalog_order(mut files: Vec<FileEntry>) -> Vec<FileEntry> {
    let key = |file: &FileEntry| (file.folder.clone(), file.name.clone(), file.root.clone());
    files.sort_by_key(key);
    files
}

impl Entry {
    fn shared(&self) -> bool {
        matches!(*self.state.lock().unwrap(), State::Shared(..))
    }

    // the file as other libraries see it, once it is shared.
    fn listed(&self) -> Option<FileEntry> {
        let State::Shared(share) = &*self.state.lock().unwrap() else {
            return None;
        };
        Some(FileEntry {
            name: self.name.clone(),
            size: self.stamp.0,
            root: to_hex(&share.root),
            folder: self.folder.clone(),
        })
    }

    fn view(&self, id: &str) -> SharedFile {
        let state = self.state.lock().unwrap();
        let (name, served, token, error) = match &*state {
            State::Shared(share) => ("shared", share.served(), Some(share.key.to_string()), None),
            State::Failed(error) => ("error", 0, None, Some(error.clone())),
            State::Hashing => ("hashing", 0, None, None),
            State::Publishing => ("publishing", 0, None, None),
        };
        let hashed = self.hashed.load(Ordering::Relaxed);
        SharedFile {
            path: id.to_owned(),
            name: self.name.clone(),
            library: self.unit.name(),
            size: self.stamp.0,
            state: name.to_owned(),
            progress: hashed as f64 / piece_count(self.stamp.0) as f64,
            served,
            link: token
                .as_deref()
                .map(|token| link_for(LINK_BASE, token, &self.name)),
            token,
            error,
        }
    }
}

impl Libraries {
    /// the libraries `host` has set out, with a peer among `peers` for
    /// each, whose records are published in the background from now on. no
    /// file is shared until the first `rescan`.
    pub fn new(
        api: &VeilidAPI,
        context: &RoutingContext,
        peers: &Arc<Peers>,
        slot_limit: Option<u32>,
        host: &Arc<dyn Host>,
        state: &Arc<dyn StateStore>,
        neighbors: &Arc<Neighbors>,
    ) -> Arc<Libraries> {
        let libraries = Arc::new(Libraries {
            api: api.clone(),
            context: context.clone(),
            peers: peers.clone(),
            slot_limit,
            host: host.clone(),
            state: state.clone(),
            neighbors: neighbors.clone(),
            units: Mutex::default(),
            arranging: Default::default(),
            records: Default::default(),
            entries: Mutex::default(),
            order: Mutex::default(),
            publishing: Arc::new(Semaphore::new(SHARE_CONCURRENCY)),
        });
        // the libraries have their peers from the start: downloads are
        // served over one of them.
        drop(libraries.arrange());
        libraries
    }

    // make `unit` a library to whoever reaches its peer, and publish the
    // record its link points at, in the background.
    fn open(self: &Arc<Self>, unit: &Arc<Unit>) {
        let (libraries, unit) = (self.clone(), unit.clone());
        unit.peer.set_library(Arc::new(Index {
            libraries: libraries.clone(),
            unit: unit.clone(),
        }));
        spawn(async move { libraries.publish(&unit).await });
    }

    // make the unit's own record, or find the one kept, and point it at
    // the unit's peer, so that there is a link to it. tried until it works:
    // a library is of use without a link, so a failure here never stops it.
    async fn publish(&self, unit: &Unit) {
        while !unit.gone.is_raised() {
            let published = async {
                let key = {
                    let _one = self.records.lock().await;
                    let (context, state) = (&self.context, self.state.as_ref());
                    let heir = self.host.heir();
                    open_library_record(context, state, &unit.name(), heir.as_deref()).await?
                };
                unit.peer.publish_home(key.clone()).await.map(|()| key)
            };
            match published.await {
                Ok(key) => {
                    let link = library_link_for(LINK_BASE, &key.to_string());
                    info!("link to {}: {link}", unit.name());
                    return unit.set_token(key.to_string());
                }
                Err(err) => warn!("could not publish the library record: {err}"),
            }
            sleep(RECORD_RETRY).await;
        }
    }

    // take a slot in the lobby for a unit that is listed, and give up the
    // slot of one that is not. a unit whose slot could not be taken is
    // tried again at the next rescan.
    async fn list_or_unlist(&self, unit: &Unit) {
        let mut held = unit.lobby.lock().await;
        let listed = unit.listed() && !unit.gone.is_raised();
        match (listed, held.take()) {
            (true, Some(slot)) => *held = Some(slot),
            (true, None) => match self.neighbors.join_lobby(&unit.peer).await {
                Ok(slot) => *held = Some(slot),
                Err(err) => warn!("could not list {}: {err}", unit.name()),
            },
            (false, Some(slot)) => unit.peer.remove(&slot).await,
            (false, None) => {}
        }
    }

    fn units(&self) -> Vec<Arc<Unit>> {
        self.units.lock().unwrap().clone()
    }

    fn unit_named(&self, name: &str) -> Option<Arc<Unit>> {
        self.units().into_iter().find(|unit| unit.name() == name)
    }

    // bring the units in line with the libraries the host has set out: a
    // unit for each new one, each one's listing as it is set, and none for
    // those that are gone, which are returned for `retire`.
    fn arrange(self: &Arc<Self>) -> Vec<Arc<Unit>> {
        let wanted = self.host.libraries();
        let (mut kept, mut gone) = (Vec::new(), Vec::new());
        for unit in self.units() {
            match wanted.iter().any(|owned| owned.name == unit.name()) {
                true => kept.push(unit),
                false => gone.push(unit),
            }
        }
        for owned in &wanted {
            let held = kept.iter().find(|unit| unit.name() == owned.name);
            let unit = held.cloned().unwrap_or_else(|| {
                let peer = Peer::new(&self.api, &self.context, &self.peers, true, self.slot_limit);
                let unit = Unit::new(peer, &owned.name, owned.listed);
                self.open(&unit);
                kept.push(unit.clone());
                unit
            });
            unit.set_listed(owned.listed);
            let libraries = self.clone();
            spawn(async move { libraries.list_or_unlist(&unit).await });
        }
        *self.units.lock().unwrap() = kept;
        gone
    }

    // a library is no longer shared: its peer gives up every slot and its
    // route, and its record is forgotten, so that a library of the same
    // name made later is a new one, with a new link.
    async fn retire(&self, unit: &Unit) {
        unit.gone.raise();
        unit.peer.retire().await;
        let _one = self.records.lock().await;
        if let Err(err) = move_kept(self.state.as_ref(), &unit.name(), None).await {
            warn!("could not forget the record of {}: {err}", unit.name());
        }
    }

    /// set the libraries out anew, as `change` makes them, for good, and
    /// share what they hold now. `renamed` says which one `change` renames,
    /// and to what: it stays the library it was, under its new name.
    pub async fn rearrange(
        self: &Arc<Self>,
        change: impl FnOnce(&mut Vec<Owned>) -> Result<()>,
        renamed: Option<(&str, &str)>,
    ) -> Result<()> {
        {
            let _arranging = self.arranging.lock().await;
            let mut all = self.host.libraries();
            change(&mut all)?;
            validate(&all)?;
            self.host.set_libraries(all)?;
            if let Some((from, to)) = renamed {
                self.rename(from, to).await;
            }
        }
        self.rescan().await;
        Ok(())
    }

    // the unit called `from` is called `to` from now on: in the record kept
    // for it, and in its slots, which say its name.
    async fn rename(&self, from: &str, to: &str) {
        let Some(unit) = self.unit_named(from) else {
            return;
        };
        unit.set_name(to);
        {
            let _one = self.records.lock().await;
            let (state, heir) = (self.state.as_ref(), self.host.heir());
            let moved = async {
                inherit_kept(state, from, heir.as_deref()).await?;
                move_kept(state, from, Some(to)).await
            };
            if let Err(err) = moved.await {
                warn!("could not keep the record of {to}: {err}");
            }
        }
        let peer = unit.peer.clone();
        spawn(async move { peer.republish().await });
    }

    /// the peer downloads are served over: the one of the library they are
    /// saved in, which is where they are shared once they are whole. None
    /// for a host that keeps downloads in no library.
    pub fn downloads_peer(&self) -> Option<Arc<Peer>> {
        let downloads = self.host.downloads();
        let all = self.host.libraries();
        let keeper = all.iter().find(|held| keeps_downloads(held, &downloads))?;
        Some(self.unit_named(&keeper.name)?.peer.clone())
    }

    /// the catalog a library token of this node's own leads to, and what
    /// that library is called. None for any other token.
    pub fn own_catalog(&self, token: &str) -> Option<(String, Vec<FileEntry>)> {
        let own = |unit: &Arc<Unit>| unit.token().as_deref() == Some(token);
        let unit = self.units().into_iter().find(own)?;
        Some((unit.name(), self.catalog_of(&unit)))
    }

    /// the libraries as their owner set them out, each with its link once
    /// it has one and how many files were found in it.
    pub fn statuses(&self) -> Vec<LibraryStatus> {
        let entries = self.entries.lock().unwrap();
        let status = |owned: Owned| {
            let unit = self.unit_named(&owned.name);
            let token = unit.as_ref().and_then(|unit| unit.token());
            let within = |entry: &&Arc<Entry>| {
                unit.as_ref()
                    .is_some_and(|unit| Arc::ptr_eq(unit, &entry.unit))
            };
            let found: Vec<&Arc<Entry>> = entries.values().filter(within).collect();
            LibraryStatus {
                name: owned.name,
                folders: owned.folders,
                listed: owned.listed,
                link: token
                    .as_deref()
                    .map(|token| library_link_for(LINK_BASE, token)),
                token,
                files: found.len(),
                shared: found.iter().filter(|entry| entry.shared()).count(),
            }
        };
        self.host.libraries().into_iter().map(status).collect()
    }

    /// files found, and how many of them are shared so far.
    pub fn counts(&self) -> (usize, usize) {
        let entries = self.entries.lock().unwrap();
        let shared = |entry: &&Arc<Entry>| entry.shared();
        (entries.len(), entries.values().filter(shared).count())
    }

    /// some of the files, in the order they were listed, and how many there
    /// are.
    pub fn files(&self, offset: usize, limit: usize) -> (usize, Vec<SharedFile>) {
        let order = self.order.lock().unwrap();
        let entries = self.entries.lock().unwrap();
        let held = order.iter().filter_map(|id| Some((id, entries.get(id)?)));
        let page = held.skip(offset).take(limit);
        (
            entries.len(),
            page.map(|(id, entry)| entry.view(id)).collect(),
        )
    }

    /// the files of one library that a fetcher can already reach, as
    /// whoever reaches that library sees them: its own and no other's, each
    /// where it is inside it.
    pub fn catalog_of(&self, unit: &Arc<Unit>) -> Vec<FileEntry> {
        let entries = self.entries.lock().unwrap();
        let own = entries
            .values()
            .filter(|entry| Arc::ptr_eq(unit, &entry.unit));
        catalog_order(own.filter_map(|entry| entry.listed()).collect())
    }

    /// bring what is shared in line with what there is to share now: share
    /// new files, stop sharing ones that are gone or have changed, and try
    /// again those that failed. returns once the files are listed; hashing
    /// and publishing go on in the background.
    pub async fn rescan(self: &Arc<Self>) {
        let _arranging = self.arranging.lock().await;
        let gone = self.arrange();
        let found = self.host.list().await;
        let listed: BTreeMap<&str, &Found> =
            found.iter().map(|file| (file.id.as_str(), file)).collect();
        for (id, entry) in self.outdated(&listed) {
            self.entries.lock().unwrap().remove(&id);
            let state = std::mem::replace(&mut *entry.state.lock().unwrap(), State::Hashing);
            Self::unshare(&entry.unit, state).await;
        }
        for unit in gone {
            self.retire(&unit).await;
        }
        *self.order.lock().unwrap() = found.iter().map(|file| file.id.clone()).collect();
        for file in found {
            self.share_if_new(file);
        }
    }

    // give up the slot and the record of a file that is no longer shared.
    async fn unshare(unit: &Unit, state: State) {
        if let State::Shared(share) = state {
            unit.peer.release(&share).await;
        }
    }

    fn holds(&self, id: &str, entry: &Arc<Entry>) -> bool {
        let entries = self.entries.lock().unwrap();
        entries.get(id).is_some_and(|held| Arc::ptr_eq(held, entry))
    }

    // the entries whose file is gone, has changed, or is somewhere else
    // among the libraries now, and those that failed.
    fn outdated(&self, listed: &BTreeMap<&str, &Found>) -> Vec<(String, Arc<Entry>)> {
        let entries = self.entries.lock().unwrap();
        let stale = |(id, entry): &(&String, &Arc<Entry>)| {
            let failed = matches!(*entry.state.lock().unwrap(), State::Failed(_));
            let (now, library) = (listed.get(id.as_str()).copied(), entry.unit.name());
            failed || !unchanged(now, entry.stamp, &library, &entry.folder)
        };
        let outdated = entries.iter().filter(stale);
        outdated
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect()
    }

    fn share_if_new(self: &Arc<Self>, file: Found) {
        // a file in a library that is gone since it was listed is found
        // again, or not, by the next scan.
        let Some(unit) = self.unit_named(&file.library) else {
            return;
        };
        let entry = Arc::new(Entry {
            stamp: file.stamp,
            name: file.source.name(),
            unit,
            folder: file.folder.clone(),
            hashed: AtomicU64::new(0),
            state: Mutex::new(State::Hashing),
        });
        {
            let mut entries = self.entries.lock().unwrap();
            if entries.contains_key(&file.id) {
                return;
            }
            entries.insert(file.id.clone(), entry.clone());
        }
        let libraries = self.clone();
        spawn(async move {
            let _permit = libraries.publishing.acquire_arc().await;
            let shared = libraries.share(&file, &entry).await;
            let state = shared.unwrap_or_else(|err| {
                warn!("could not share {}: {err}", file.id);
                State::Failed(err.to_string())
            });
            // a scan may have dropped the file while it was being published.
            if libraries.holds(&file.id, &entry) {
                *entry.state.lock().unwrap() = state;
            } else {
                Self::unshare(&entry.unit, state).await;
            }
        });
    }

    // hash the file, unless its leaves were kept, then publish it.
    async fn share(&self, file: &Found, entry: &Arc<Entry>) -> Result<State> {
        let (size, _) = entry.stamp;
        let state = self.state.as_ref();
        let leaves = match kept_leaves(state, &file.id, entry.stamp).await {
            Some(leaves) => leaves,
            None => hash_leaves(file.source.as_ref(), &entry.hashed).await?,
        };
        keep_leaves(state, &file.id, entry.stamp, &leaves).await?;
        entry.hashed.store(piece_count(size), Ordering::Relaxed);
        *entry.state.lock().unwrap() = State::Publishing;
        let manifest = manifest_of(size, &leaves)?;
        let (api, context) = (&self.api, &self.context);
        let share = share_file(
            api,
            context,
            &entry.unit.peer,
            file.source.clone(),
            &manifest,
            leaves,
            JOIN_ATTEMPTS,
        )
        .await?;
        info!("sharing {}", entry.name);
        Ok(State::Shared(share))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::MemoryState;
    use futures_util::FutureExt;

    #[test]
    fn leaves_are_kept_until_the_file_changes() {
        let state = MemoryState::default();
        let kept = |id: &str, stamp| kept_leaves(&state, id, stamp).now_or_never().unwrap();
        let id = "/music/a.mp3";
        let stamp = (40_000, 1_700_000_000_000);
        let leaves = vec![[1; HASH_BYTES], [2; HASH_BYTES]];
        assert_eq!(kept(id, stamp), None);
        let keeping = keep_leaves(&state, id, stamp, &leaves);
        keeping.now_or_never().unwrap().unwrap();
        assert_eq!(kept(id, stamp), Some(leaves));
        assert_eq!(kept(id, (40_001, stamp.1)), None);
        assert_eq!(kept(id, (stamp.0, stamp.1 + 1)), None);
        assert_eq!(kept("/music/b.mp3", stamp), None);
    }

    struct NoBytes;

    impl Source for NoBytes {
        fn name(&self) -> String {
            "a.mp3".into()
        }

        fn size(&self) -> u64 {
            0
        }

        fn read(&self, _start: u64, _end: u64) -> crate::store::BoxFuture<'_, Result<Vec<u8>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[test]
    fn a_file_that_moved_within_the_library_is_shared_anew() {
        let stamp = (40_000, 1_700_000_000_000);
        let listed = |library: &str, folder: &str| Found {
            id: "/music/live/a.mp3".into(),
            stamp,
            library: library.into(),
            folder: folder.into(),
            source: Arc::new(NoBytes),
        };
        let same = listed("tunes", "live");
        assert!(unchanged(Some(&same), stamp, "tunes", "live"));
        // gone, changed, in another library, or elsewhere in its own: a
        // library's peer serves the files of that library alone, so
        // one that moved is no longer its to serve.
        assert!(!unchanged(None, stamp, "tunes", "live"));
        assert!(!unchanged(Some(&same), (40_001, stamp.1), "tunes", "live"));
        assert!(!unchanged(
            Some(&listed("other", "live")),
            stamp,
            "tunes",
            "live"
        ));
        assert!(!unchanged(
            Some(&listed("tunes", "")),
            stamp,
            "tunes",
            "live"
        ));
    }

    #[test]
    fn the_catalog_is_ordered_by_folder_then_name_then_root() {
        let file = |folder: &str, name: &str, root: &str| FileEntry {
            name: name.to_owned(),
            size: 1,
            root: root.to_owned(),
            folder: folder.to_owned(),
        };
        let ordered = catalog_order(vec![
            file("music/live", "a", "1"),
            file("music", "b", "1"),
            file("music", "a", "2"),
            file("music", "a", "1"),
            file("docs", "z", "1"),
        ]);
        let expected = [
            file("docs", "z", "1"),
            file("music", "a", "1"),
            file("music", "a", "2"),
            file("music", "b", "1"),
            file("music/live", "a", "1"),
        ];
        assert_eq!(ordered, expected);
    }
}
