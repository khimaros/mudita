//! the core's files and downloads as a web page has them (see the core's
//! store.rs): a file the user picked is read through the browser's own
//! slicing, and a download is kept in the browser's storage, a chunk at a
//! time, so that neither is ever held in memory whole. where a download is
//! kept depends on the page (see `Kind`); the browser's side of the rest is
//! in glue.js.

use crate::{indexed, opfs};
use js_sys::Uint8Array;
use mudita::chunked::{ChunkedSink, CHUNK_PIECES};
use mudita::protocol::{piece_count, save_name};
use mudita::store::{BoxFuture, Sink, Sinks, Source};
use mudita::Result;
use send_wrapper::SendWrapper;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

const SESSION_SEPARATOR: char = '.';
const LOG_TARGET: &str = "mudita";

// what a call into javascript gives: its value, or what it threw.
pub type Thrown<T> = std::result::Result<T, JsValue>;

#[wasm_bindgen(module = "/src/glue.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = readFile)]
    async fn read_file(file: &web_sys::File, start: f64, end: f64) -> Thrown<JsValue>;

    #[wasm_bindgen(js_name = opfsLack)]
    async fn opfs_lack() -> JsValue;
}

/// what javascript threw, as the core speaks of failures: an error's
/// message, or the string that was thrown.
pub fn failure(thrown: JsValue) -> mudita::Error {
    let said = match thrown.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => thrown.as_string().unwrap_or_else(|| format!("{thrown:?}")),
    };
    said.into()
}

// a page has one thread, so a future over its values is never sent, but
// the core's tasks ask that it could be.
pub fn on_page<'a, T>(work: impl Future<Output = T> + 'a) -> BoxFuture<'a, T> {
    Box::pin(SendWrapper::new(work))
}

pub fn bytes_of(array: JsValue) -> Vec<u8> {
    Uint8Array::from(array).to_vec()
}

/// a file the user picked in the page, to share.
pub struct PickedFile(SendWrapper<web_sys::File>);

impl PickedFile {
    pub fn new(file: web_sys::File) -> PickedFile {
        PickedFile(SendWrapper::new(file))
    }
}

impl Source for PickedFile {
    fn name(&self) -> String {
        self.0.name()
    }

    fn size(&self) -> u64 {
        self.0.size() as u64
    }

    fn read(&self, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        on_page(async move {
            let read = read_file(&self.0, start as f64, end as f64).await;
            read.map(bytes_of).map_err(failure)
        })
    }
}

/// where a page keeps its downloads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// the origin private file system (opfs.rs).
    Opfs,
    /// indexeddb (indexed.rs), where there is no such file system.
    Indexed,
}

impl Kind {
    /// the kind for a page, given why it cannot have the origin private
    /// file system, if it cannot.
    fn chosen(lack: Option<&str>) -> Kind {
        lack.map_or(Kind::Opfs, |_| Kind::Indexed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Opfs => "opfs",
            Kind::Indexed => "indexeddb",
        }
    }

    // what download `number` of `session` is kept under: a folder's name,
    // or a database's.
    fn place(self, session: &str, number: u64) -> String {
        let own = format!("{session}{SESSION_SEPARATOR}{number}");
        match self {
            Kind::Opfs => own,
            Kind::Indexed => format!("{}{own}", indexed::PREFIX),
        }
    }
}

/// the line that tells the user where downloads are kept, and why.
fn announcement(lack: Option<&str>) -> String {
    match lack {
        None => "keeping downloads in the origin private file system".to_owned(),
        Some(lack) => format!("keeping downloads in indexeddb: {lack}"),
    }
}

/// where a page's downloads go: a folder or a database each, numbered
/// within the page's session.
pub struct PageSinks {
    kind: Kind,
    session: String,
    next: AtomicU64,
}

impl PageSinks {
    /// claim `session` for this page, and drop what pages that are gone
    /// left behind.
    pub async fn open(session: String) -> Result<PageSinks> {
        // THE CHOICE OF STORAGE. the origin private file system is where
        // downloads belong, and it and the web locks its cleanup uses
        // exist only in a secure context. veilid cannot be reached from an
        // https page today, so the app is served over plain http, where
        // there is neither: such a page falls back on indexeddb. once
        // veilid works over https, remove the fallback (indexed.rs,
        // indexed.js, `Kind`) and keep everything in the file system.
        let lack = opfs_lack().await.as_string();
        let kind = Kind::chosen(lack.as_deref());
        tracing::info!(target: LOG_TARGET, "{}", announcement(lack.as_deref()));
        match kind {
            Kind::Opfs => opfs::claim(&session).await.map_err(failure)?,
            Kind::Indexed => indexed::sweep().await,
        }
        Ok(PageSinks {
            kind,
            session,
            next: AtomicU64::new(1),
        })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }
}

impl Sinks for PageSinks {
    // a download is kept under a number: its name is only put on the file
    // the page is handed once it is whole.
    fn create(&self, _name: &str, size: u64) -> BoxFuture<'_, Result<Arc<dyn Sink>>> {
        let number = self.next.fetch_add(1, Ordering::Relaxed);
        let place = self.kind.place(&self.session, number);
        on_page(async move {
            let sink: Arc<dyn Sink> = match self.kind {
                Kind::Opfs => Arc::new(ChunkedSink::new(opfs::Folder(place), size)),
                Kind::Indexed => {
                    let database = indexed::Database::open(place).await?;
                    Arc::new(ChunkedSink::new(database, size))
                }
            };
            Ok(sink)
        })
    }
}

/// how many chunks a download of `size` bytes is kept in.
fn chunk_count(size: u64) -> u32 {
    piece_count(size).div_ceil(CHUNK_PIECES) as u32
}

/// the finished download kept at `place`, as one file called `name`: what
/// the download is listed as, which may be a stranger's word for it, so
/// only its last part is used.
pub async fn saved_file(kind: Kind, place: &str, name: &str, size: u64) -> Result<web_sys::File> {
    let (count, name) = (chunk_count(size), save_name(name));
    let joined = match kind {
        Kind::Opfs => opfs::join(place, count, &name).await,
        Kind::Indexed => indexed::join(place, count, &name).await,
    };
    Ok(joined.map_err(failure)?.unchecked_into())
}

/// drop what a download kept, finished or not.
pub async fn remove(kind: Kind, place: &str) {
    match kind {
        Kind::Opfs => opfs::remove(place).await,
        Kind::Indexed => indexed::remove(place).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mudita::protocol::PIECE_BYTES;

    #[test]
    fn a_download_is_kept_in_as_many_chunks_as_its_pieces_fill() {
        let chunk = CHUNK_PIECES * PIECE_BYTES;
        assert_eq!(chunk_count(0), 1);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(chunk), 1);
        assert_eq!(chunk_count(chunk + 1), 2);
        assert_eq!(chunk_count(10 * chunk), 10);
    }

    #[test]
    fn a_page_without_the_file_system_falls_back_on_indexeddb_and_says_why() {
        assert_eq!(Kind::chosen(None), Kind::Opfs);
        assert_eq!(
            announcement(None),
            "keeping downloads in the origin private file system"
        );
        let lack = Some("the page is not a secure context");
        assert_eq!(Kind::chosen(lack), Kind::Indexed);
        assert_eq!(
            announcement(lack),
            "keeping downloads in indexeddb: the page is not a secure context"
        );
        assert_eq!(
            (Kind::Opfs.as_str(), Kind::Indexed.as_str()),
            ("opfs", "indexeddb")
        );
    }

    #[test]
    fn a_download_is_kept_under_its_session_and_number() {
        assert_eq!(Kind::Opfs.place("ab12", 3), "ab12.3");
        let database = Kind::Indexed.place("ab12", 3);
        assert_eq!(database, "mudita-download-ab12.3");
        // what the sweep of leftovers goes by.
        assert!(database.starts_with(indexed::PREFIX));
    }
}
