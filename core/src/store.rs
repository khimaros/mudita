//! what the core asks of whoever embeds it. the core has no filesystem: the
//! command line hands it files on disk, a browser hands it the files a user
//! picked and somewhere in the page's storage to put downloads.
//!
//! every future here is `Send`, because the core runs them on tasks. a
//! browser embedder, whose values are not, wraps them as veilid does its
//! own (`send_wrapper`): there is one thread there, so nothing is ever sent.

use crate::library::Found;
use crate::owned::Owned;
use crate::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// a file to share: what it is called, how big it is, and its bytes.
pub trait Source: Send + Sync {
    /// the file's name, without any folder.
    fn name(&self) -> String;

    fn size(&self) -> u64;

    /// the bytes from `start` up to, not including, `end`.
    fn read(&self, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>>;
}

/// where a download goes, a verified piece at a time.
pub trait Sink: Send + Sync {
    fn write(&self, index: u64, piece: Vec<u8>) -> BoxFuture<'_, Result<()>>;

    /// a piece written earlier, read back to serve it on. this works after
    /// `finish` too.
    fn read(&self, index: u64) -> BoxFuture<'_, Result<Vec<u8>>>;

    /// every piece is written: put the file in its place. says where that
    /// is, for the user.
    fn finish(&self) -> BoxFuture<'_, Result<String>>;

    /// the download failed or was given up: drop what was written.
    fn discard(&self) -> BoxFuture<'_, ()>;
}

/// where downloads go. asked once per download, as soon as its size and the
/// name to save it under are known and before any piece is: refusing here
/// (a file in the way, a full disk) costs nothing. the name has been cut to
/// its last path component: it may have come from a stranger.
pub trait Sinks: Send + Sync {
    fn create(&self, name: &str, size: u64) -> BoxFuture<'_, Result<Arc<dyn Sink>>>;
}

/// the files of an embedder that names them by path, as the command line
/// does and a browser does not. it is how a client of the control protocol
/// (see control.rs) shares a file of its own choosing, or says exactly
/// where a download is to go.
pub trait Files: Send + Sync {
    /// the file at `path`, to share.
    fn open(&self, path: &str) -> BoxFuture<'_, Result<Arc<dyn Source>>>;

    /// where one download goes when it is to be saved exactly at `path`,
    /// and never over a file that is there.
    fn sink_at(&self, path: &str) -> Arc<dyn Sinks>;
}

/// storage that takes a download a chunk at a time: a run of pieces,
/// written once and whole. a browser's file system is of this kind, where
/// every write to a file costs a copy of the file. `chunked::ChunkedSink`
/// makes a `Sink` of it.
pub trait Chunks: Send + Sync {
    fn write(&self, chunk: u64, bytes: Vec<u8>) -> BoxFuture<'_, Result<()>>;

    /// the bytes of a written chunk from `start` up to, not including,
    /// `end`, counted from the start of the chunk.
    fn read(&self, chunk: u64, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>>;

    /// every chunk is written. says where the download is, for whoever
    /// asks after it.
    fn finish(&self) -> BoxFuture<'_, Result<String>>;

    fn discard(&self) -> BoxFuture<'_, ()>;
}

/// small values kept between runs, by name: a library's record, and the
/// leaf hashes of files already hashed. a name may contain '/'.
pub trait StateStore: Send + Sync {
    fn get(&self, name: &str) -> BoxFuture<'_, Option<Vec<u8>>>;

    fn put(&self, name: &str, value: Vec<u8>) -> BoxFuture<'_, Result<()>>;
}

/// what a node that shares libraries is told by its owner, and where that
/// is kept: the libraries with the folders in each, and the files in them.
pub trait Host: Send + Sync {
    /// the libraries as the owner set them out, the one downloads go to
    /// among them.
    fn libraries(&self) -> Vec<Owned>;

    /// set the libraries out anew, for good. the list has been checked.
    fn set_libraries(&self, all: Vec<Owned>) -> Result<()>;

    /// the library that is what the node's one library was before a node
    /// had several, by name, where the owner's settings are from then: it
    /// takes over the record kept for that one, and so its link. None
    /// once the settings have been kept in their present shape.
    fn heir(&self) -> Option<String>;

    /// where downloads go: a folder of one of the libraries.
    fn downloads(&self) -> String;

    /// every file to share, as the folders hold them now.
    fn list(&self) -> BoxFuture<'_, Vec<Found>>;
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// a state store that forgets everything when the test ends.
    #[derive(Default)]
    pub struct MemoryState(Mutex<HashMap<String, Vec<u8>>>);

    impl StateStore for MemoryState {
        fn get(&self, name: &str) -> BoxFuture<'_, Option<Vec<u8>>> {
            let value = self.0.lock().unwrap().get(name).cloned();
            Box::pin(async move { value })
        }

        fn put(&self, name: &str, value: Vec<u8>) -> BoxFuture<'_, Result<()>> {
            self.0.lock().unwrap().insert(name.to_owned(), value);
            Box::pin(async { Ok(()) })
        }
    }
}
