//! an engine's downloads: any number at once, each into a sink the embedder
//! supplies, and served to others while they run (DESIGN.md "serving while
//! downloading") over one peer: that of the library downloads are saved in,
//! or on a node without libraries its only one. what becomes of a finished
//! one depends on the node: one with libraries hands it to their folders,
//! to be shared like every other file there; on one without, the download
//! itself goes on serving the file, until it is dropped from the list.

use crate::control;
use crate::fetch::{file_key, key_of, name_of, open_record, read_manifest, Fetch, Swarm};
use crate::library::Libraries;
use crate::merkle::blake3_hash;
use crate::neighbors::Neighbors;
use crate::node::owner_keypair;
use crate::peer::{Peer, Share};
use crate::protocol::{chosen_name, hash_from_hex, piece_count, save_name, share_keys, FileEntry};
use crate::rt::{race, spawn, Either, Flag};
use crate::store::Sinks;
use crate::Result;
use futures_util::future::{select, Either as Selected};
use futures_util::pin_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::warn;
use veilid_core::{RecordKey, RoutingContext, VeilidAPI};

// a download serves as a courtesy, so its slot is not fought for.
const SERVE_ATTEMPTS: u32 = 3;
const CANCELLED: &str = "cancelled";
const NOWHERE: &str = "say where to save it: this node has no download folder";

/// what to download: a file found by browsing or searching, or whatever a
/// share link or token leads to.
pub enum Target {
    File(FileEntry),
    Token(String),
}

// a target, and the name its asker wants the file saved under, if any.
type Asked = (Target, Option<String>);

#[derive(Default)]
struct Entry {
    id: u64,
    // what is known so far, as clients see it.
    view: Mutex<control::Download>,
    fetch: Mutex<Option<Arc<Fetch>>>,
    // the record and the slot held while the download runs, and after it
    // for as long as the download itself serves the file.
    key: Mutex<Option<RecordKey>>,
    share: Mutex<Option<Arc<Share>>>,
    cancelled: Flag,
}

pub struct Downloads {
    api: VeilidAPI,
    context: RoutingContext,
    peer: Arc<Peer>,
    neighbors: Arc<Neighbors>,
    // the library a finished download is handed to, where there is one.
    library: Option<Arc<Libraries>>,
    // where a download goes that does not say.
    sinks: Option<Arc<dyn Sinks>>,
    concurrency: Option<usize>,
    entries: Mutex<Vec<Arc<Entry>>>,
    next_id: AtomicU64,
}

impl Entry {
    fn set(&self, change: impl FnOnce(&mut control::Download)) {
        change(&mut self.view.lock().unwrap());
    }

    fn view(&self) -> control::Download {
        let mut view = self.view.lock().unwrap().clone();
        if let Some(fetch) = &*self.fetch.lock().unwrap() {
            view.done = fetch.progress().0;
        }
        view
    }
}

impl Downloads {
    pub fn new(
        api: &VeilidAPI,
        context: &RoutingContext,
        peer: &Arc<Peer>,
        neighbors: &Arc<Neighbors>,
        library: Option<Arc<Libraries>>,
        sinks: Option<Arc<dyn Sinks>>,
        concurrency: Option<usize>,
    ) -> Arc<Downloads> {
        Arc::new(Downloads {
            api: api.clone(),
            context: context.clone(),
            peer: peer.clone(),
            neighbors: neighbors.clone(),
            library,
            sinks,
            concurrency,
            entries: Mutex::default(),
            next_id: AtomicU64::new(1),
        })
    }

    /// every download of this run, newest first.
    pub fn views(&self) -> Vec<control::Download> {
        let entries = self.entries.lock().unwrap();
        entries.iter().rev().map(|entry| entry.view()).collect()
    }

    /// start a download in the background, into a sink of `sinks` or, when
    /// none is given, to where this engine's downloads go. `name` is what
    /// to call the file, over the name its link or its listing gives it.
    /// returns its id.
    pub fn start(
        self: &Arc<Self>,
        target: Target,
        name: Option<String>,
        sinks: Option<Arc<dyn Sinks>>,
    ) -> Result<u64> {
        let sinks = sinks.or_else(|| self.sinks.clone()).ok_or(NOWHERE)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Entry {
            id,
            ..Default::default()
        });
        entry.set(|view| (view.id, view.state) = (id, "resolving".into()));
        if let Target::File(file) = &target {
            let name = chosen_name(name.as_deref(), &file.name, &[]);
            entry.set(|view| (view.name, view.size) = (Some(name), Some(file.size)));
        }
        self.entries.lock().unwrap().push(entry.clone());
        spawn(self.clone().run(entry, (target, name), sinks));
        Ok(id)
    }

    /// stop a running download and drop it from the list, or just drop a
    /// finished one, which then no longer serves its file either.
    pub fn cancel(&self, id: u64) -> Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let at = entries.iter().position(|entry| entry.id == id);
        let entry = entries.remove(at.ok_or("no such download")?);
        entry.cancelled.raise();
        Ok(())
    }

    // give up the slot and the record a download held.
    async fn release(&self, entry: &Entry) {
        let share = entry.share.lock().unwrap().take();
        if let Some(share) = share {
            self.peer.remove(&share).await;
        }
        let key = entry.key.lock().unwrap().take();
        if let Some(key) = key {
            self.peer.close(&key).await;
        }
    }

    async fn run(self: Arc<Self>, entry: Arc<Entry>, asked: Asked, sinks: Arc<dyn Sinks>) {
        let downloading = self.download(&entry, asked, sinks.as_ref());
        let outcome = match race(downloading, entry.cancelled.wait()).await {
            Either::Left(outcome) => outcome,
            Either::Right(()) => Err(CANCELLED.into()),
        };
        let saved = match outcome {
            Ok(saved) => saved,
            Err(err) => {
                self.release(&entry).await;
                return self.failed(&entry, &err.to_string()).await;
            }
        };
        entry.set(|view| (view.state, view.saved) = ("done".into(), Some(saved)));
        match &self.library {
            // the slot was held as a download. as a whole file it is the
            // library's to share.
            Some(library) => {
                self.release(&entry).await;
                library.rescan().await;
            }
            None => self.serve_on(&entry).await,
        }
    }

    // nobody else will share a file this engine has downloaded, so the
    // download does, as a peer holding every piece, until it is dropped.
    async fn serve_on(&self, entry: &Entry) {
        let share = entry.share.lock().unwrap().clone();
        if let Some(share) = share {
            self.peer.complete(&share).await;
        }
        entry.cancelled.wait().await;
        self.release(entry).await;
    }

    // drop what a failed or cancelled download had written, and say why it
    // ended. a cancelled download has left the list and needs no word.
    async fn failed(&self, entry: &Entry, error: &str) {
        let fetch = entry.fetch.lock().unwrap().clone();
        if let Some(fetch) = fetch {
            fetch.discard().await;
        }
        if error != CANCELLED {
            warn!("download failed: {error}");
        }
        entry.set(|view| (view.state, view.error) = ("error".into(), Some(error.to_owned())));
    }

    // the record of what is to be downloaded, and the name its link or its
    // listing gives it.
    async fn key_of(&self, target: &Target) -> Result<(RecordKey, String)> {
        match target {
            Target::Token(token) => key_of(token),
            Target::File(file) => Ok((file_key(&self.api, file).await?, file.name.clone())),
        }
    }

    // returns where the file was saved.
    async fn download(
        &self,
        entry: &Entry,
        (target, asked): Asked,
        sinks: &dyn Sinks,
    ) -> Result<String> {
        let (api, context) = (&self.api, &self.context);
        let (key, given) = self.key_of(&target).await?;
        open_record(context, &key).await?;
        *entry.key.lock().unwrap() = Some(key.clone());
        let manifest = read_manifest(api, context, &key).await?;
        let root = hash_from_hex(&manifest.root)?;
        // reopened as a writer, which a peer holding a slot must be.
        let keys = share_keys(blake3_hash, &root, manifest.size);
        let writer = Some(owner_keypair(&keys));
        drop(context.open_dht_record(key.clone(), writer).await?);
        let name = name_of(context, &key, asked.as_deref(), &given).await;
        let size = manifest.size;
        let sink = sinks.create(&save_name(&name), size).await?;
        let neighbors = self.neighbors.clone();
        let swarm = Some(Swarm {
            peer: self.peer.clone(),
            // a fresh slot in the file's record: that library is alive.
            on_library: Box::new(move |route| neighbors.saw(&route)),
        });
        let (concurrency, shown) = (self.concurrency, name.clone());
        let fetch = Fetch::new(api, context, &key, name, manifest, sink, swarm, concurrency)?;
        *entry.fetch.lock().unwrap() = Some(fetch.clone());
        entry.set(|view| {
            (view.name, view.size) = (Some(shown), Some(size));
            (view.state, view.total) = ("downloading".into(), Some(piece_count(size)));
        });
        self.fetch_and_serve(entry, &fetch, key).await
    }

    // run the download and the claim of a slot for it side by side, so that
    // a full swarm or a slow write never holds up the pieces.
    async fn fetch_and_serve(
        &self,
        entry: &Entry,
        fetch: &Arc<Fetch>,
        key: RecordKey,
    ) -> Result<String> {
        let root = hash_from_hex(&fetch.manifest.root)?;
        let serving = self
            .peer
            .add(key, root, &fetch.name, fetch.clone(), false, SERVE_ATTEMPTS);
        let running = fetch.run();
        pin_mut!(serving, running);
        match select(running, serving).await {
            Selected::Left((saved, serving)) => {
                // a small file is whole before its slot is claimed. where
                // the download is to go on serving it, the claim is seen
                // through, with the file already shown as done.
                if let (Ok(saved), None) = (&saved, &self.library) {
                    let saved = Some(saved.clone());
                    entry.set(|view| (view.state, view.saved) = ("done".into(), saved));
                    *entry.share.lock().unwrap() = serving.await.ok();
                }
                saved
            }
            Selected::Right((share, running)) => {
                *entry.share.lock().unwrap() = share.ok();
                running.await
            }
        }
    }
}
