//! THE PLAIN-HTTP WORKAROUND: downloads in indexeddb, a database each, for
//! a page that is not a secure context and so cannot keep them as opfs.rs
//! does. veilid cannot be reached from an https page today, so the app is
//! served over plain http, and this is what runs there. once it can be,
//! remove this module and indexed.js and keep every download in the origin
//! private file system. the browser's side of it is in indexed.js.

use crate::storage::{bytes_of, failure, on_page, Thrown};
use js_sys::Uint8Array;
use mudita::store::{BoxFuture, Chunks};
use mudita::Result;
use wasm_bindgen::prelude::*;

/// what every download's database is named with, and nothing else in the
/// page's storage is.
pub const PREFIX: &str = "mudita-download-";

#[wasm_bindgen(module = "/src/indexed.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = openDownload)]
    async fn open_download(name: &str) -> Thrown<()>;

    #[wasm_bindgen(catch, js_name = storeChunk)]
    async fn store_chunk(name: &str, chunk: u32, bytes: Uint8Array) -> Thrown<JsValue>;

    #[wasm_bindgen(catch, js_name = loadChunk)]
    async fn load_chunk(name: &str, chunk: u32, start: u32, end: u32) -> Thrown<JsValue>;

    #[wasm_bindgen(catch, js_name = joinStored)]
    pub async fn join(name: &str, count: u32, file_name: &str) -> Thrown<JsValue>;

    #[wasm_bindgen(js_name = dropDownload)]
    pub async fn remove(name: &str);

    #[wasm_bindgen(js_name = sweepDownloads)]
    async fn sweep_downloads(prefix: &str);
}

/// drop what pages that are gone left behind.
pub async fn sweep() {
    sweep_downloads(PREFIX).await;
}

/// one download's database.
pub struct Database(String);

impl Database {
    /// a database called `name`, made and held open: a full disk or a
    /// browser that refuses storage says so here, before any piece is
    /// asked for.
    pub async fn open(name: String) -> Result<Database> {
        open_download(&name).await.map_err(failure)?;
        Ok(Database(name))
    }
}

impl Chunks for Database {
    fn write(&self, chunk: u64, bytes: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        on_page(async move {
            // copied out of the wasm memory: the write outlives this call.
            let bytes = Uint8Array::from(&bytes[..]);
            let stored = store_chunk(&self.0, chunk as u32, bytes).await;
            stored.map(drop).map_err(failure)
        })
    }

    fn read(&self, chunk: u64, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        on_page(async move {
            let read = load_chunk(&self.0, chunk as u32, start as u32, end as u32).await;
            read.map(bytes_of).map_err(failure)
        })
    }

    fn finish(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }

    fn discard(&self) -> BoxFuture<'_, ()> {
        on_page(remove(&self.0))
    }
}
