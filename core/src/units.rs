//! a library as the network sees one (DESIGN.md "a node's libraries"): a
//! peer with a route of its own, a record of its own that a link points
//! at, and a name. a node has one for each of its libraries, which are
//! strangers to each other as far as anyone outside can tell.

use crate::peer::{Peer, Share};
use crate::rt::Flag;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Unit {
    pub peer: Arc<Peer>,
    name: Mutex<String>,
    listed: AtomicBool,
    // the key of its record, once that points at its peer.
    token: Mutex<Option<String>>,
    /// the slot held in the lobby while the unit is listed. behind an
    /// async lock so that one change of listing runs at a time.
    pub lobby: async_lock::Mutex<Option<Arc<Share>>>,
    /// raised when the library is no longer shared.
    pub gone: Flag,
}

impl Unit {
    pub fn new(peer: Arc<Peer>, name: &str, listed: bool) -> Arc<Unit> {
        Arc::new(Unit {
            peer,
            name: Mutex::new(name.to_owned()),
            listed: AtomicBool::new(listed),
            token: Mutex::default(),
            lobby: Default::default(),
            gone: Flag::default(),
        })
    }

    pub fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }

    pub fn set_name(&self, name: &str) {
        name.clone_into(&mut self.name.lock().unwrap());
    }

    pub fn listed(&self) -> bool {
        self.listed.load(Ordering::Relaxed)
    }

    pub fn set_listed(&self, listed: bool) {
        self.listed.store(listed, Ordering::Relaxed);
    }

    /// the token a link to this library carries. None until its record is
    /// published.
    pub fn token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    pub fn set_token(&self, token: String) {
        *self.token.lock().unwrap() = Some(token);
    }
}
