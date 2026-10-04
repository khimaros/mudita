//! files shared one at a time: what `send` shares, what a page in a browser
//! shares when its user picks a file, and what a client of the control
//! protocol shares by path. each is hashed and published in the background
//! and served over one peer, which is no library, until it is stopped or
//! the engine is. a library's folders are not these: see library.rs.

use crate::control;
use crate::peer::{Peer, Share};
use crate::protocol::{link_for, piece_count, LINK_BASE};
use crate::rt::{race, spawn, Either, Flag};
use crate::seed::{hash_leaves, manifest_of, share_file, JOIN_ATTEMPTS};
use crate::store::Source;
use crate::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};
use veilid_core::{RoutingContext, VeilidAPI};

const STOPPED: &str = "stopped";

enum State {
    Hashing,
    Publishing,
    Shared(Arc<Share>),
    Failed(String),
}

struct Entry {
    id: u64,
    name: String,
    size: u64,
    // pieces hashed so far.
    hashed: AtomicU64,
    state: Mutex<State>,
    // raised when the share reaches a state it stays in: shared or failed.
    settled: Flag,
    stopped: Flag,
}

pub struct Shares {
    api: VeilidAPI,
    context: RoutingContext,
    peer: Arc<Peer>,
    entries: Mutex<Vec<Arc<Entry>>>,
    next_id: AtomicU64,
}

impl Entry {
    // the share as clients see it, with whoever `peer` has heard is
    // downloading it.
    fn view(&self, peer: &Peer) -> control::Share {
        let held = self.state.lock().unwrap();
        let (state, served, token, error) = match &*held {
            State::Shared(share) => ("shared", share.served(), Some(share.key.to_string()), None),
            State::Failed(error) => ("error", 0, None, Some(error.clone())),
            State::Hashing => ("hashing", 0, None, None),
            State::Publishing => ("publishing", 0, None, None),
        };
        let downloaders = match &*held {
            State::Shared(share) => peer.downloaders(&share.root),
            _ => Vec::new(),
        };
        let hashed = self.hashed.load(Ordering::Relaxed);
        control::Share {
            id: self.id,
            name: self.name.clone(),
            size: self.size,
            state: state.to_owned(),
            progress: hashed as f64 / piece_count(self.size) as f64,
            served,
            link: token
                .as_deref()
                .map(|token| link_for(LINK_BASE, token, &self.name)),
            token,
            error,
            downloads: downloaders.into_iter().map(Into::into).collect(),
        }
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }
}

impl Shares {
    pub fn new(api: &VeilidAPI, context: &RoutingContext, peer: &Arc<Peer>) -> Arc<Shares> {
        Arc::new(Shares {
            api: api.clone(),
            context: context.clone(),
            peer: peer.clone(),
            entries: Mutex::default(),
            next_id: AtomicU64::new(1),
        })
    }

    /// every share of this run, newest first.
    pub fn views(&self) -> Vec<control::Share> {
        let entries = self.entries.lock().unwrap();
        let views = entries.iter().rev().map(|entry| entry.view(&self.peer));
        views.collect()
    }

    fn entry(&self, id: u64) -> Result<Arc<Entry>> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.iter().find(|entry| entry.id == id);
        entry.cloned().ok_or_else(|| "no such share".into())
    }

    /// start sharing a file in the background. returns the share's id.
    pub fn add(self: &Arc<Self>, source: Arc<dyn Source>) -> u64 {
        let entry = Arc::new(Entry {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            name: source.name(),
            size: source.size(),
            hashed: AtomicU64::new(0),
            state: Mutex::new(State::Hashing),
            settled: Flag::default(),
            stopped: Flag::default(),
        });
        self.entries.lock().unwrap().push(entry.clone());
        spawn(self.clone().run(entry.clone(), source));
        entry.id
    }

    /// the share token of a share, once it is being served.
    pub async fn token(&self, id: u64) -> Result<String> {
        let entry = self.entry(id)?;
        entry.settled.wait().await;
        let view = entry.view(&self.peer);
        view.token
            .ok_or_else(|| view.error.unwrap_or_default().into())
    }

    /// stop sharing a file and drop it from the list. its slot is given up
    /// in the background.
    pub fn stop(&self, id: u64) -> Result<()> {
        let entry = self.entry(id)?;
        self.entries.lock().unwrap().retain(|held| held.id != id);
        entry.stopped.raise();
        Ok(())
    }

    // share the file until it is stopped, then give up its slot and record.
    async fn run(self: Arc<Self>, entry: Arc<Entry>, source: Arc<dyn Source>) {
        let shared = match race(self.share(&entry, source), entry.stopped.wait()).await {
            Either::Left(shared) => shared,
            Either::Right(()) => Err(STOPPED.into()),
        };
        let share = match shared {
            Ok(share) => share,
            Err(err) => {
                if !entry.stopped.is_raised() {
                    warn!("could not share {}: {err}", entry.name);
                }
                entry.set(State::Failed(err.to_string()));
                return entry.settled.raise();
            }
        };
        info!("sharing {}", entry.name);
        entry.set(State::Shared(share.clone()));
        entry.settled.raise();
        entry.stopped.wait().await;
        self.peer.release(&share).await;
    }

    async fn share(&self, entry: &Entry, source: Arc<dyn Source>) -> Result<Arc<Share>> {
        info!("hashing {}", entry.name);
        let leaves = hash_leaves(source.as_ref(), &entry.hashed).await?;
        let manifest = manifest_of(entry.size, &leaves)?;
        entry.set(State::Publishing);
        let (api, context) = (&self.api, &self.context);
        share_file(
            api,
            context,
            &self.peer,
            source,
            &manifest,
            leaves,
            JOIN_ATTEMPTS,
        )
        .await
    }
}
