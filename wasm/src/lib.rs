//! the mudita core in a web page: one engine, the same one `dana serve`
//! runs, driven from javascript. a page shares the files its user picks,
//! downloads into the browser's own storage, and for everything else speaks
//! the control protocol (the core's control.rs) through `call`, as a client
//! of the server's socket would. every promise here rejects with the
//! failure's message, a plain string.

mod indexed;
mod log;
mod opfs;
mod storage;

use mudita::control::{decode_response, encode_request, Download};
use mudita::rt::Flag;
use mudita::serve::{Embedder, Engine, Tuning};
use mudita::Result;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::Arc;
use storage::{Kind, PageSinks, PickedFile};
use veilid_core::{VeilidConfig, VeilidConfigProtectedStore};
use wasm_bindgen::prelude::*;

const PROGRAM_NAME: &str = "mudita";
const NOT_STARTED: &str = "the node has not been started";
const ALREADY_STARTED: &str = "the node is already started";
const NOT_DONE: &str = "that download is not finished";
// tells one page's downloads from another's (see storage.rs).
const SESSION_BYTES: usize = 8;

/// the engine a page runs, where its downloads are kept, and how to end it.
#[derive(Clone)]
struct Running {
    engine: Arc<Engine>,
    storage: Kind,
    stop: Arc<Flag>,
    ended: Arc<Flag>,
}

thread_local! {
    static RUNNING: RefCell<Option<Running>> = const { RefCell::new(None) };
}

fn thrown(err: mudita::Error) -> JsValue {
    JsValue::from_str(&err.to_string())
}

fn running() -> Result<Running> {
    RUNNING.with(|held| held.borrow().clone().ok_or_else(|| NOT_STARTED.into()))
}

// the engine, once its node can reach the network.
async fn ready_engine() -> Result<Arc<Engine>> {
    let engine = running()?.engine;
    engine.ready().await?;
    Ok(engine)
}

// one control request of our own, answered as a client would have it.
async fn ask(engine: &Engine, method: &str, params: Value) -> Result<Value> {
    let reply = engine.call(&encode_request(0, method, params)).await;
    decode_response(&reply)?.1
}

// a node kept in the browser's own storage, under `namespace`. a browser
// has no keyring, so what veilid would protect there is stored plainly: a
// node's identity says nothing about its user.
fn node_config(namespace: String) -> VeilidConfig {
    VeilidConfig {
        program_name: PROGRAM_NAME.into(),
        namespace,
        protected_store: VeilidConfigProtectedStore {
            allow_insecure_fallback: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
    log::install();
}

/// start the page's node, keeping its storage under `namespace`. resolves
/// once the engine is running, which is before its node has reached the
/// network: `call` answers at once, with `"ready": false` in `status`
/// until it has.
#[wasm_bindgen]
pub async fn start(namespace: String) -> std::result::Result<(), JsValue> {
    if running().is_ok() {
        return Err(JsValue::from_str(ALREADY_STARTED));
    }
    let session = to_hex(&mudita::rt::unique_bytes::<SESSION_BYTES>());
    let sinks = PageSinks::open(session).await.map_err(thrown)?;
    let storage = sinks.kind();
    let embedder = Embedder {
        sinks: Some(Arc::new(sinks)),
        ..Default::default()
    };
    let run = Running {
        engine: Engine::new(embedder, Tuning::default()),
        storage,
        stop: Arc::default(),
        ended: Arc::default(),
    };
    RUNNING.with(|held| *held.borrow_mut() = Some(run.clone()));
    wasm_bindgen_futures::spawn_local(async move {
        let config = node_config(namespace);
        if let Err(err) = run.engine.run(config, run.stop.wait()).await {
            tracing::error!(target: "mudita", "the node could not start: {err}");
        }
        run.ended.raise();
    });
    Ok(())
}

/// one control request, as a line of json, and its response.
#[wasm_bindgen]
pub async fn call(line: String) -> std::result::Result<String, JsValue> {
    let engine = running().map_err(thrown)?.engine;
    Ok(engine.call(&line).await)
}

/// share a file the user picked. resolves to the share's id, under which
/// `shares` lists its progress and, once it is shared, its token.
#[wasm_bindgen(js_name = shareFile)]
pub async fn share_file(file: web_sys::File) -> std::result::Result<f64, JsValue> {
    let engine = ready_engine().await.map_err(thrown)?;
    let share = engine.add_share(Arc::new(PickedFile::new(file)));
    share.map(|id| id as f64).map_err(thrown)
}

/// download what a share link or token leads to into the browser's own
/// storage, to be called `name` when one is given, and else what the link
/// or the file's peers call it. resolves to the download's id, under which
/// `downloads` lists its progress.
#[wasm_bindgen(js_name = downloadFile)]
pub async fn download_file(
    link: String,
    name: Option<String>,
) -> std::result::Result<f64, JsValue> {
    let started = async {
        let engine = ready_engine().await?;
        let params = json!({"token": link, "name": name});
        let started = ask(&engine, "download", params).await?;
        started["download"].as_f64().ok_or_else(|| "no id".into())
    };
    started.await.map_err(thrown)
}

// what `downloads` lists under `id`.
fn download(engine: &Engine, id: f64) -> Result<Download> {
    let downloads = engine.downloads()?;
    let wanted = downloads.into_iter().find(|held| held.id as f64 == id);
    wanted.ok_or_else(|| "no such download".into())
}

/// where this page keeps its downloads: `"opfs"`, the origin private file
/// system, or `"indexeddb"` on a page that cannot have it.
#[wasm_bindgen(js_name = storageKind)]
pub fn storage_kind() -> std::result::Result<String, JsValue> {
    let run = running().map_err(thrown)?;
    Ok(run.storage.as_str().to_owned())
}

/// a finished download as a file for the page to save. it lies on disk,
/// and is not read into memory to be handed over.
#[wasm_bindgen(js_name = downloadedFile)]
pub async fn downloaded_file(id: f64) -> std::result::Result<web_sys::File, JsValue> {
    let saved = async {
        let run = running()?;
        let download = download(&run.engine, id)?;
        let (Some(place), Some(name), Some(size)) = (download.saved, download.name, download.size)
        else {
            return Err(NOT_DONE.into());
        };
        storage::saved_file(run.storage, &place, &name, size).await
    };
    saved.await.map_err(thrown)
}

/// drop a download, finished or not: it stops, stops being served to
/// others, and what it wrote is removed.
#[wasm_bindgen(js_name = discardDownload)]
pub async fn discard_download(id: f64) -> std::result::Result<(), JsValue> {
    let discarded = async {
        let run = running()?;
        let saved = download(&run.engine, id)?.saved;
        ask(&run.engine, "download.cancel", json!({"id": id as u64})).await?;
        // an unfinished download drops what it kept as it stops.
        if let Some(place) = saved {
            storage::remove(run.storage, &place).await;
        }
        Ok(())
    };
    discarded.await.map_err(thrown)
}

/// have every line the core logs at info and above handed to
/// `handler(level, target, message)`: the same lines, in the same words,
/// that the command line prints.
#[wasm_bindgen(js_name = setLogHandler)]
pub fn set_log_handler(handler: js_sys::Function) {
    log::set_handler(handler);
}

/// leave every swarm and shut the node down.
#[wasm_bindgen]
pub async fn stop() -> std::result::Result<(), JsValue> {
    let run = running().map_err(thrown)?;
    run.stop.raise();
    run.ended.wait().await;
    RUNNING.with(|held| *held.borrow_mut() = None);
    Ok(())
}
