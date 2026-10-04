//! downloads in the origin private file system, a folder each: where they
//! belong, and where a page that is a secure context keeps them. the
//! browser's side of it is in opfs.js.

use crate::storage::{bytes_of, failure, on_page, Thrown};
use js_sys::Uint8Array;
use mudita::store::{BoxFuture, Chunks};
use mudita::Result;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/opfs.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = writeChunk)]
    async fn write_chunk(folder: &str, chunk: u32, bytes: Uint8Array) -> Thrown<()>;

    #[wasm_bindgen(catch, js_name = readChunk)]
    async fn read_chunk(folder: &str, chunk: u32, start: u32, end: u32) -> Thrown<JsValue>;

    #[wasm_bindgen(catch, js_name = joinChunks)]
    pub async fn join(folder: &str, count: u32, name: &str) -> Thrown<JsValue>;

    #[wasm_bindgen(js_name = removeFolder)]
    pub async fn remove(folder: &str);

    /// claim `session` for this page, and drop what pages that are gone
    /// left behind.
    #[wasm_bindgen(catch, js_name = claimSession)]
    pub async fn claim(session: &str) -> Thrown<()>;
}

/// one download's folder.
pub struct Folder(pub String);

impl Chunks for Folder {
    fn write(&self, chunk: u64, bytes: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        on_page(async move {
            // copied out of the wasm memory: the write outlives this call.
            let bytes = Uint8Array::from(&bytes[..]);
            let written = write_chunk(&self.0, chunk as u32, bytes).await;
            written.map_err(failure)
        })
    }

    fn read(&self, chunk: u64, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        on_page(async move {
            let read = read_chunk(&self.0, chunk as u32, start as u32, end as u32).await;
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
