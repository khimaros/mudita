//! the remote control of an engine: what is said over the unix socket of
//! `dana serve`, and to the engine a web page runs (the wasm crate).
//! newline-delimited json, one connection carrying any number of requests.
//! `{"id":1,"method":"status","params":{}}` is answered by
//! `{"id":1,"result":{...}}` or `{"id":1,"error":"..."}`. the cli's own
//! commands and the apps are all clients of it. everything here is pure.

use crate::download_log::{Seen, SeenFile, Standing};
use crate::protocol::FileEntry;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const UNKNOWN_METHOD: &str = "unknown method";
/// what a method that needs the network answers until the node is usable.
pub const NOT_READY: &str = "not ready yet";

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// everything a client can ask.
#[derive(Debug, PartialEq)]
pub enum Call {
    Status,
    Files {
        offset: usize,
        limit: usize,
    },
    /// the node's own libraries, as `status` lists them.
    Libraries,
    /// share a folder: in the library named, or with none named, in a new
    /// one called what the folder is.
    AddFolder {
        path: String,
        library: Option<String>,
    },
    /// stop sharing a folder: of the library named, or with none named, of
    /// whichever has it, which goes too if that leaves it empty.
    RemoveFolder {
        path: String,
        library: Option<String>,
    },
    /// a new library, with no folders yet.
    AddLibrary {
        name: String,
        listed: bool,
    },
    RemoveLibrary(String),
    /// call a library something else. its link stays.
    RenameLibrary {
        name: String,
        to: String,
    },
    /// have a library found by searches, or reached by its link alone.
    SetLibrary {
        name: String,
        listed: bool,
    },
    Rescan,
    /// other people's libraries, as far as this node has discovered them.
    Discover,
    /// read the lobby again for libraries that have joined since.
    RefreshDiscover,
    Browse {
        library: String,
        offset: u32,
    },
    /// a catalog page of the library a library link leads to, which need
    /// not be one this server knows.
    BrowseLink {
        link: String,
        offset: u32,
    },
    StartSearch(String),
    PollSearch(u64),
    /// the most recent search, for a client that did not start it.
    LastSearch,
    /// download a file found by browsing or searching, or what a share link
    /// or token leads to.
    DownloadFile(FileEntry, Saving),
    DownloadToken(String, Saving),
    Downloads,
    CancelDownload(u64),
    /// the files shared one at a time, as opposed to a library's folders.
    Shares,
    /// every download others are making from this node, of a file shared
    /// on its own or of a library's.
    Uploads,
    /// share the file at a path, where the embedder has paths.
    AddShare(String),
    StopShare(u64),
    /// what a share link or token leads to, without downloading it.
    Lookup(String),
    /// test-only (see `serve::Tuning`): ask the library a link leads to for
    /// the first piece of the file with this root, and say what it answers.
    Probe {
        link: String,
        root: String,
    },
}

/// how a download is to be saved, where its asker says.
#[derive(Deserialize, Debug, PartialEq, Clone, Default)]
pub struct Saving {
    /// what to call the file, over any name in a link.
    pub name: Option<String>,
    /// exactly where to put it, where the embedder has paths; else it goes
    /// to where the embedder puts downloads.
    pub path: Option<String>,
}

#[derive(Deserialize)]
struct FilesParams {
    #[serde(default)]
    offset: usize,
    limit: usize,
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct FolderParams {
    path: String,
    library: Option<String>,
}

#[derive(Deserialize)]
struct NameParams {
    name: String,
}

#[derive(Deserialize)]
struct LibraryParams {
    name: String,
    // a new library is reached by its link alone unless it is asked for
    // listed.
    #[serde(default)]
    listed: bool,
}

#[derive(Deserialize)]
struct ListedParams {
    name: String,
    listed: bool,
}

#[derive(Deserialize)]
struct RenameParams {
    name: String,
    to: String,
}

#[derive(Deserialize)]
struct BrowseParams {
    library: Option<String>,
    link: Option<String>,
    #[serde(default)]
    offset: u32,
}

#[derive(Deserialize)]
struct QueryParams {
    query: String,
}

#[derive(Deserialize)]
struct SearchParams {
    search: u64,
}

#[derive(Deserialize)]
struct DownloadParams {
    file: Option<FileEntry>,
    token: Option<String>,
    #[serde(flatten)]
    saving: Saving,
}

#[derive(Deserialize)]
struct LinkParams {
    link: String,
}

#[derive(Deserialize)]
struct IdParams {
    id: u64,
}

#[derive(Deserialize)]
struct ProbeParams {
    link: String,
    root: String,
}

fn params<T: serde::de::DeserializeOwned>(request: Request) -> Result<T> {
    let parsed = serde_json::from_value(request.params);
    parsed.map_err(|err| format!("bad params: {err}").into())
}

impl Call {
    /// what a request asks for.
    pub fn of(request: Request) -> Result<Call> {
        Ok(match request.method.as_str() {
            "status" => Call::Status,
            "files" => {
                let FilesParams { offset, limit } = params(request)?;
                Call::Files { offset, limit }
            }
            "folders.add" => {
                let FolderParams { path, library } = params(request)?;
                Call::AddFolder { path, library }
            }
            "folders.remove" => {
                let FolderParams { path, library } = params(request)?;
                Call::RemoveFolder { path, library }
            }
            "library.add" => {
                let LibraryParams { name, listed } = params(request)?;
                Call::AddLibrary { name, listed }
            }
            "library.remove" => Call::RemoveLibrary(params::<NameParams>(request)?.name),
            "library.rename" => {
                let RenameParams { name, to } = params(request)?;
                Call::RenameLibrary { name, to }
            }
            "library.set" => {
                let ListedParams { name, listed } = params(request)?;
                Call::SetLibrary { name, listed }
            }
            "libraries" => Call::Libraries,
            "rescan" => Call::Rescan,
            "discover" => Call::Discover,
            "discover.refresh" => Call::RefreshDiscover,
            "browse" => match params::<BrowseParams>(request)? {
                BrowseParams {
                    library: Some(library),
                    offset,
                    ..
                } => Call::Browse { library, offset },
                BrowseParams {
                    link: Some(link),
                    offset,
                    ..
                } => Call::BrowseLink { link, offset },
                _ => return Err("bad params: a library or a link is needed".into()),
            },
            "search.start" => Call::StartSearch(params::<QueryParams>(request)?.query),
            "search.poll" => Call::PollSearch(params::<SearchParams>(request)?.search),
            "search.last" => Call::LastSearch,
            "download" => match params::<DownloadParams>(request)? {
                DownloadParams {
                    file: Some(file),
                    saving,
                    ..
                } => Call::DownloadFile(file, saving),
                DownloadParams {
                    token: Some(token),
                    saving,
                    ..
                } => Call::DownloadToken(token, saving),
                _ => return Err("bad params: a file or a token is needed".into()),
            },
            "downloads" => Call::Downloads,
            "download.cancel" => Call::CancelDownload(params::<IdParams>(request)?.id),
            "shares" => Call::Shares,
            "uploads" => Call::Uploads,
            "share.add" => Call::AddShare(params::<PathParams>(request)?.path),
            "share.stop" => Call::StopShare(params::<IdParams>(request)?.id),
            "lookup" => Call::Lookup(params::<LinkParams>(request)?.link),
            "probe" => {
                let ProbeParams { link, root } = params(request)?;
                Call::Probe { link, root }
            }
            _ => return Err(UNKNOWN_METHOD.into()),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Status {
    /// whether the node can reach the network yet.
    pub ready: bool,
    /// every shared folder but the one downloads go to, whichever library
    /// it is in.
    pub folders: Vec<String>,
    pub downloads: String,
    /// files found in the folders, and how many of them are shared so far.
    pub files: usize,
    pub shared: usize,
    /// how many of other people's libraries have been discovered.
    pub discovered: usize,
    /// how many downloads others are making from this node just now.
    #[serde(default)]
    pub uploading: usize,
    /// the node's own libraries.
    pub libraries: Vec<LibraryStatus>,
}

/// the node's own libraries, as `libraries` answers.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Libraries {
    pub libraries: Vec<LibraryStatus>,
}

/// one of the node's own libraries, as `status` lists it.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct LibraryStatus {
    pub name: String,
    pub folders: Vec<String>,
    /// whether a search finds it; if not, it is reached by its link alone.
    pub listed: bool,
    /// its own token, which says nothing of the node's other libraries.
    /// None until its record is published.
    pub token: Option<String>,
    /// the link with that token in it.
    pub link: Option<String>,
    /// files found in its folders, and how many of them are shared so far.
    pub files: usize,
    pub shared: usize,
}

/// one file of the node's libraries, as `files` lists it. `state` is
/// "hashing", "publishing", "shared" or "error".
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct SharedFile {
    pub path: String,
    pub name: String,
    /// the library it is in.
    #[serde(default)]
    pub library: String,
    pub size: u64,
    pub state: String,
    /// 0 to 1, moving while the file is hashed.
    pub progress: f64,
    pub served: u64,
    pub token: Option<String>,
    /// the share link, with the file's name in it. null until it is shared.
    pub link: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SharedFiles {
    pub total: usize,
    pub files: Vec<SharedFile>,
}

/// someone else's library, as `discover` lists it.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Discovered {
    pub id: String,
    /// what its owner calls it. null until it has answered once.
    pub name: Option<String>,
    /// unix seconds at which it was last known to be alive. null for one
    /// that only another library has spoken of.
    pub seen: Option<u64>,
    /// how many files it shares, once a page of its catalog has been seen;
    /// left out until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
    /// the link to it, where it was reached by one: the lobby gives a
    /// library's route and not its record. left out otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// other people's libraries, as `discover` answers.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Discoveries {
    pub libraries: Vec<Discovered>,
}

/// a file of another library, as the engine hands it out: with its share
/// token, so that a client can download it without deriving any keys.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Listed {
    #[serde(flatten)]
    pub file: FileEntry,
    pub token: String,
}

/// a page of another library's catalog.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Page {
    pub total: usize,
    /// what that library's owner calls it; empty for none.
    #[serde(default)]
    pub library: String,
    pub files: Vec<Listed>,
}

/// a file a search found. libraries may call one file different things:
/// `file.name` is the most common of its names and `names` is all of them,
/// the most common first. `sources` is how many of the libraries asked
/// have it, under whatever name.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Hit {
    #[serde(flatten)]
    pub file: FileEntry,
    pub token: String,
    pub names: Vec<String>,
    pub sources: usize,
    /// what the libraries that have it are called, those that have a name,
    /// in the order they answered.
    #[serde(default)]
    pub libraries: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SearchResults {
    /// libraries that have answered or failed so far.
    pub asked: usize,
    pub done: bool,
    pub results: Vec<Hit>,
}

/// one file shared on its own, as `shares` lists it. `state` is "hashing",
/// "publishing", "shared" or "error".
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Share {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub state: String,
    /// 0 to 1, moving while the file is hashed.
    pub progress: f64,
    pub served: u64,
    pub token: Option<String>,
    /// the share link, with the file's name in it. null until it is shared.
    pub link: Option<String>,
    pub error: Option<String>,
    /// who is downloading the file from here, or has been, oldest first.
    #[serde(default)]
    pub downloads: Vec<Downloader>,
}

/// one download of a shared file, as its downloader's own notices tell of
/// it (DESIGN.md "download notices"): downloaders are anonymous, so this
/// is all there is to know of one. `state` is "downloading", "finished",
/// or "gone" for one not heard from for a minute.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Downloader {
    /// the 16 hex characters the log calls it by.
    pub id: String,
    /// pieces it has reported, and how many the file has.
    pub done: u64,
    pub total: u64,
    pub state: String,
    /// unix seconds of its first and of its latest notice.
    pub started: u64,
    pub seen: u64,
}

const DOWNLOADING: &str = "downloading";

fn state_of(standing: Standing) -> &'static str {
    match standing {
        Standing::Downloading => DOWNLOADING,
        Standing::Finished => "finished",
        Standing::Gone => "gone",
    }
}

impl From<Seen> for Downloader {
    fn from(seen: Seen) -> Downloader {
        Downloader {
            id: seen.id,
            done: seen.done,
            total: seen.total,
            state: state_of(seen.standing).to_owned(),
            started: seen.started,
            seen: seen.seen,
        }
    }
}

/// one download someone else is making from this node, or has made, as
/// `uploads` lists it: a `Downloader`, and the file it is of.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Upload {
    pub id: String,
    /// what this node shares the file as.
    pub name: String,
    /// the library the file is in; null for a file shared on its own.
    pub library: Option<String>,
    pub done: u64,
    pub total: u64,
    /// the file's size in bytes.
    pub size: u64,
    pub state: String,
    pub started: u64,
    pub seen: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Uploads {
    pub uploads: Vec<Upload>,
}

impl Upload {
    pub fn running(&self) -> bool {
        self.state == DOWNLOADING
    }
}

/// the downloads others make from this node as `uploads` lists them: those
/// under way first, then those that have ended, the newest first in each.
pub fn uploads_of(held: Vec<(Option<String>, SeenFile)>) -> Vec<Upload> {
    let listed = |(library, file): (Option<String>, SeenFile)| Upload {
        id: file.seen.id,
        name: file.name,
        library,
        done: file.seen.done,
        total: file.seen.total,
        size: file.size,
        state: state_of(file.seen.standing).to_owned(),
        started: file.seen.started,
        seen: file.seen.seen,
    };
    let mut uploads: Vec<Upload> = held.into_iter().map(listed).collect();
    uploads.sort_by_key(|upload| (!upload.running(), std::cmp::Reverse(upload.started)));
    uploads
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Shares {
    pub shares: Vec<Share>,
}

/// what a share link or token leads to, as `lookup` answers: the bare
/// token to download it by, the file, and what it is called.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Found {
    pub token: String,
    pub size: u64,
    pub root: String,
    /// the name in the link, else the most common of `names`, else empty.
    pub name: String,
    /// every distinct name the peers in the file's slots give it, the most
    /// common first.
    pub names: Vec<String>,
}

/// one download. `state` is "resolving", "downloading", "done" or "error".
/// the size and piece count are unknown until the manifest is read, and the
/// name until it is settled: the one asked for, the one in the link or the
/// listing, or else the most common one in the file's slots.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
pub struct Download {
    pub id: u64,
    pub name: Option<String>,
    pub size: Option<u64>,
    pub state: String,
    pub done: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// where a finished download is, as its embedder names places: a path
    /// on disk, or what a browser asks its storage for.
    #[serde(default)]
    pub saved: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Downloads {
    pub downloads: Vec<Download>,
}

/// the result of a method that only acts.
pub fn nothing() -> Value {
    json!({})
}

pub fn encode_request(id: u64, method: &str, params: Value) -> String {
    json!({"id": id, "method": method, "params": params}).to_string()
}

/// one line answering request `id`.
pub fn encode_response(id: u64, outcome: Result<Value>) -> String {
    match outcome {
        Ok(result) => json!({"id": id, "result": result}).to_string(),
        Err(err) => json!({"id": id, "error": err.to_string()}).to_string(),
    }
}

/// the id a response answers, and its result or the server's error.
pub fn decode_response(line: &str) -> Result<(u64, Result<Value>)> {
    #[derive(Deserialize)]
    struct Response {
        id: u64,
        result: Option<Value>,
        error: Option<String>,
    }
    let response: Response = serde_json::from_str(line)?;
    let outcome = match (response.result, response.error) {
        (_, Some(error)) => Err(error.into()),
        (Some(result), None) => Ok(result),
        (None, None) => return Err("response has neither a result nor an error".into()),
    };
    Ok((response.id, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> Result<Call> {
        Call::of(serde_json::from_str(line)?)
    }

    fn file() -> FileEntry {
        FileEntry {
            name: "a.bin".into(),
            size: 7,
            root: "ab".repeat(32),
            folder: "music".into(),
        }
    }

    fn saving(name: Option<&str>, path: Option<&str>) -> Saving {
        Saving {
            name: name.map(str::to_owned),
            path: path.map(str::to_owned),
        }
    }

    #[test]
    fn methods_without_params_parse_with_or_without_them() {
        assert_eq!(call(r#"{"id":1,"method":"status"}"#).unwrap(), Call::Status);
        let with = r#"{"id":1,"method":"rescan","params":{}}"#;
        assert_eq!(call(with).unwrap(), Call::Rescan);
        for (method, expected) in [
            ("libraries", Call::Libraries),
            ("discover", Call::Discover),
            ("discover.refresh", Call::RefreshDiscover),
            ("search.last", Call::LastSearch),
            ("downloads", Call::Downloads),
        ] {
            let line = encode_request(3, method, nothing());
            assert_eq!(call(&line).unwrap(), expected);
        }
    }

    #[test]
    fn methods_with_params_parse() {
        let parsed = |method: &str, params: Value| call(&encode_request(1, method, params));
        assert_eq!(
            parsed("files", json!({"offset": 5, "limit": 10})).unwrap(),
            Call::Files {
                offset: 5,
                limit: 10
            }
        );
        let folder = json!({"path": "/music"});
        assert_eq!(
            parsed("folders.add", folder.clone()).unwrap(),
            Call::AddFolder {
                path: "/music".into(),
                library: None
            }
        );
        assert_eq!(
            parsed("folders.remove", folder).unwrap(),
            Call::RemoveFolder {
                path: "/music".into(),
                library: None
            }
        );
        assert_eq!(
            parsed(
                "browse",
                json!({"library": "0011223344556677", "offset": 3})
            )
            .unwrap(),
            Call::Browse {
                library: "0011223344556677".into(),
                offset: 3
            }
        );
        assert_eq!(
            parsed("search.start", json!({"query": "linux iso"})).unwrap(),
            Call::StartSearch("linux iso".into())
        );
        assert_eq!(
            parsed("search.poll", json!({"search": 4})).unwrap(),
            Call::PollSearch(4)
        );
        assert_eq!(
            parsed("download.cancel", json!({"id": 2})).unwrap(),
            Call::CancelDownload(2)
        );
    }

    #[test]
    fn libraries_are_added_renamed_listed_and_removed_by_name() {
        let parsed = |method: &str, params: Value| call(&encode_request(1, method, params));
        let named = |name: &str| json!({"name": name});
        assert_eq!(
            parsed("library.add", named("tunes")).unwrap(),
            Call::AddLibrary {
                name: "tunes".into(),
                listed: false
            }
        );
        assert_eq!(
            parsed("library.add", json!({"name": "tunes", "listed": true})).unwrap(),
            Call::AddLibrary {
                name: "tunes".into(),
                listed: true
            }
        );
        assert_eq!(
            parsed("library.remove", named("tunes")).unwrap(),
            Call::RemoveLibrary("tunes".into())
        );
        assert_eq!(
            parsed("library.rename", json!({"name": "tunes", "to": "music"})).unwrap(),
            Call::RenameLibrary {
                name: "tunes".into(),
                to: "music".into()
            }
        );
        assert_eq!(
            parsed("library.set", json!({"name": "tunes", "listed": false})).unwrap(),
            Call::SetLibrary {
                name: "tunes".into(),
                listed: false
            }
        );
        // a folder is shared in the library named, when one is.
        let folder = json!({"path": "/music", "library": "tunes"});
        assert_eq!(
            parsed("folders.add", folder.clone()).unwrap(),
            Call::AddFolder {
                path: "/music".into(),
                library: Some("tunes".into())
            }
        );
        assert_eq!(
            parsed("folders.remove", folder).unwrap(),
            Call::RemoveFolder {
                path: "/music".into(),
                library: Some("tunes".into())
            }
        );
        for method in ["library.add", "library.remove", "library.rename"] {
            assert!(parsed(method, nothing()).is_err(), "{method}");
        }
        assert!(parsed("library.set", named("tunes")).is_err());
    }

    #[test]
    fn a_download_names_a_file_or_a_token() {
        let by_file = json!({"file": file()});
        let parsed = call(&encode_request(1, "download", by_file)).unwrap();
        assert_eq!(parsed, Call::DownloadFile(file(), Saving::default()));
        let by_token = encode_request(1, "download", json!({"token": "VLD0:a:b"}));
        assert_eq!(
            call(&by_token).unwrap(),
            Call::DownloadToken("VLD0:a:b".into(), Saving::default())
        );
        assert!(call(&encode_request(1, "download", nothing())).is_err());
    }

    #[test]
    fn a_download_may_say_exactly_where_it_goes() {
        let placed = json!({"token": "VLD0:a:b", "path": "/tmp/out.bin"});
        assert_eq!(
            call(&encode_request(1, "download", placed)).unwrap(),
            Call::DownloadToken("VLD0:a:b".into(), saving(None, Some("/tmp/out.bin")))
        );
        let placed = json!({"file": file(), "path": "/tmp/out.bin"});
        assert_eq!(
            call(&encode_request(1, "download", placed)).unwrap(),
            Call::DownloadFile(file(), saving(None, Some("/tmp/out.bin")))
        );
    }

    #[test]
    fn a_download_may_say_what_the_file_is_to_be_called() {
        let named = json!({"token": "VLD0:a:b", "name": "mine.bin"});
        assert_eq!(
            call(&encode_request(1, "download", named)).unwrap(),
            Call::DownloadToken("VLD0:a:b".into(), saving(Some("mine.bin"), None))
        );
        // a file as `browse` or a search hands it out is one to download.
        let mut listed = serde_json::to_value(Listed {
            file: file(),
            token: "VLD0:a:b".into(),
        });
        listed.as_mut().unwrap()["names"] = json!(["a.bin", "b.bin"]);
        let named = json!({"file": listed.unwrap(), "name": "mine.bin"});
        assert_eq!(
            call(&encode_request(1, "download", named)).unwrap(),
            Call::DownloadFile(file(), saving(Some("mine.bin"), None))
        );
    }

    #[test]
    fn uploads_are_listed_running_first_and_newest_first() {
        let held = |id: &str, library: Option<&str>, standing, started| {
            let seen = Seen {
                id: id.into(),
                done: 1,
                total: 4,
                standing,
                started,
                seen: started + 3,
            };
            let file = SeenFile {
                seen,
                name: format!("{id}.bin"),
                size: 100_000,
            };
            (library.map(str::to_owned), file)
        };
        let uploads = uploads_of(vec![
            held("old-done", None, Standing::Finished, 10),
            held("old-live", Some("tunes"), Standing::Downloading, 20),
            held("new-gone", Some("tunes"), Standing::Gone, 30),
            held("new-live", None, Standing::Downloading, 40),
        ]);
        let ids: Vec<&str> = uploads.iter().map(|upload| upload.id.as_str()).collect();
        assert_eq!(ids, ["new-live", "old-live", "new-gone", "old-done"]);
        assert_eq!(uploads.iter().filter(|upload| upload.running()).count(), 2);
        // a library's file says which library; one shared on its own, null.
        let expected = json!({"id": "old-live", "name": "old-live.bin", "library": "tunes", "done": 1, "total": 4, "size": 100_000, "state": "downloading", "started": 20, "seen": 23});
        assert_eq!(serde_json::to_value(&uploads[1]).unwrap(), expected);
        let plain = serde_json::to_value(&uploads[3]).unwrap();
        assert_eq!(
            (&plain["library"], &plain["state"]),
            (&Value::Null, &json!("finished"))
        );
        assert_eq!(serde_json::to_value(&uploads[2]).unwrap()["state"], "gone");
        let asked = call(&encode_request(1, "uploads", nothing())).unwrap();
        assert_eq!(asked, Call::Uploads);
    }

    #[test]
    fn single_files_are_shared_listed_stopped_and_looked_up() {
        let parsed = |method: &str, params: Value| call(&encode_request(1, method, params));
        assert_eq!(parsed("shares", nothing()).unwrap(), Call::Shares);
        assert_eq!(
            parsed("share.add", json!({"path": "/tmp/a.bin"})).unwrap(),
            Call::AddShare("/tmp/a.bin".into())
        );
        assert_eq!(
            parsed("share.stop", json!({"id": 3})).unwrap(),
            Call::StopShare(3)
        );
        assert_eq!(
            parsed("lookup", json!({"link": "https://x/#VLD0:a:b"})).unwrap(),
            Call::Lookup("https://x/#VLD0:a:b".into())
        );
        for method in ["share.add", "share.stop", "lookup"] {
            assert!(parsed(method, nothing()).is_err(), "{method}");
        }
    }

    #[test]
    fn a_share_and_a_looked_up_file_are_written_as_documented() {
        let share = Share {
            id: 2,
            name: "a.bin".into(),
            size: 7,
            state: "hashing".into(),
            progress: 0.5,
            served: 0,
            token: None,
            link: None,
            error: None,
            downloads: vec![Downloader::from(Seen {
                id: "0102030405060708".into(),
                done: 3,
                total: 10,
                standing: Standing::Gone,
                started: 100,
                seen: 114,
            })],
        };
        let downloader = json!({"id": "0102030405060708", "done": 3, "total": 10, "state": "gone", "started": 100, "seen": 114});
        let expected = json!({"id": 2, "name": "a.bin", "size": 7, "state": "hashing", "progress": 0.5, "served": 0, "token": null, "link": null, "error": null, "downloads": [downloader]});
        assert_eq!(serde_json::to_value(&share).unwrap(), expected);
        let states = [Standing::Downloading, Standing::Finished].map(|standing| {
            let seen = Seen {
                id: String::new(),
                done: 0,
                total: 0,
                standing,
                started: 0,
                seen: 0,
            };
            Downloader::from(seen).state
        });
        assert_eq!(states, ["downloading", "finished"]);
        // a share from a core that does not list them has none.
        let old = json!({"id": 2, "name": "a.bin", "size": 7, "state": "hashing", "progress": 0.5, "served": 0, "token": null, "link": null, "error": null});
        assert!(serde_json::from_value::<Share>(old)
            .unwrap()
            .downloads
            .is_empty());
        let found = Found {
            token: "VLD0:a:b".into(),
            size: 7,
            root: "ab".repeat(32),
            name: "a.bin".into(),
            names: vec!["a.bin".into(), "b.bin".into()],
        };
        let expected = json!({"token": "VLD0:a:b", "size": 7, "root": "ab".repeat(32), "name": "a.bin", "names": ["a.bin", "b.bin"]});
        assert_eq!(serde_json::to_value(&found).unwrap(), expected);
    }

    #[test]
    fn unknown_methods_and_bad_params_are_errors() {
        let unknown = call(r#"{"id":1,"method":"dance"}"#).unwrap_err();
        assert_eq!(unknown.to_string(), UNKNOWN_METHOD);
        let bad = call(r#"{"id":1,"method":"files","params":{"limit":"many"}}"#);
        assert!(bad.unwrap_err().to_string().starts_with("bad params"));
        assert!(call("not json").is_err());
    }

    #[test]
    fn responses_round_trip() {
        let line = encode_response(7, Ok(json!({"search": 2})));
        assert_eq!(line, r#"{"id":7,"result":{"search":2}}"#);
        let (id, outcome) = decode_response(&line).unwrap();
        assert_eq!((id, outcome.unwrap()), (7, json!({"search": 2})));

        let line = encode_response(8, Err(NOT_READY.into()));
        assert_eq!(line, r#"{"error":"not ready yet","id":8}"#);
        let (id, outcome) = decode_response(&line).unwrap();
        assert_eq!(
            (id, outcome.unwrap_err().to_string()),
            (8, NOT_READY.to_owned())
        );
        assert!(decode_response(r#"{"id":1}"#).is_err());
    }

    #[test]
    fn a_hit_is_a_file_with_its_token_names_and_sources() {
        let hit = Hit {
            file: file(),
            token: "VLD0:a:b".into(),
            names: vec!["a.bin".into(), "b.bin".into()],
            sources: 2,
            libraries: vec!["the shelf".into()],
        };
        let expected = json!({"name": "a.bin", "size": 7, "root": "ab".repeat(32), "folder": "music", "token": "VLD0:a:b", "names": ["a.bin", "b.bin"], "sources": 2, "libraries": ["the shelf"]});
        assert_eq!(serde_json::to_value(&hit).unwrap(), expected);
        assert_eq!(serde_json::from_value::<Hit>(expected).unwrap(), hit);
    }

    #[test]
    fn a_listed_file_carries_the_token_to_download_it_by() {
        let page = Page {
            total: 1,
            library: "the shelf".into(),
            files: vec![Listed {
                file: file(),
                token: "VLD0:a:b".into(),
            }],
        };
        let expected = json!({"total": 1, "library": "the shelf", "files": [{"name": "a.bin", "size": 7, "root": "ab".repeat(32), "folder": "music", "token": "VLD0:a:b"}]});
        assert_eq!(serde_json::to_value(&page).unwrap(), expected);
    }

    #[test]
    fn a_file_to_download_needs_no_folder() {
        let named = json!({"file": {"name": "a.bin", "size": 7, "root": "ab".repeat(32)}});
        let Call::DownloadFile(file, _) = call(&encode_request(1, "download", named)).unwrap()
        else {
            panic!("a file did not parse as a file download");
        };
        assert_eq!(file.folder, "");
    }

    #[test]
    fn a_discovered_library_is_listed_with_its_name_and_when_it_was_seen() {
        let answered = Discovered {
            id: "0011223344556677".into(),
            name: Some("the shelf".into()),
            seen: Some(1_791_072_000),
            files: Some(12),
            link: Some("http://x/#library/VLD0:a:b".into()),
        };
        // how many files it has and its link are said where they are known.
        let expected = json!({"id": "0011223344556677", "name": "the shelf", "seen": 1_791_072_000u64, "files": 12, "link": "http://x/#library/VLD0:a:b"});
        assert_eq!(serde_json::to_value(&answered).unwrap(), expected);
        let spoken_of = Discovered {
            id: "8899aabbccddeeff".into(),
            name: None,
            seen: None,
            files: None,
            link: None,
        };
        let expected = json!({"id": "8899aabbccddeeff", "name": null, "seen": null});
        assert_eq!(serde_json::to_value(&spoken_of).unwrap(), expected);
        // a node has no library that stands for all of its libraries, so
        // there is none to name.
        let named = encode_request(1, "name.set", json!({"name": "the shelf"}));
        assert_eq!(call(&named).unwrap_err().to_string(), UNKNOWN_METHOD);
    }

    #[test]
    fn a_library_is_browsed_by_its_id_or_its_link() {
        let by_link = json!({"link": "library/VLD0:a:b", "offset": 2});
        assert_eq!(
            call(&encode_request(1, "browse", by_link)).unwrap(),
            Call::BrowseLink {
                link: "library/VLD0:a:b".into(),
                offset: 2
            }
        );
        assert!(call(&encode_request(1, "browse", json!({"offset": 2}))).is_err());
    }

    #[test]
    fn status_lists_each_library_with_its_own_link_once_it_has_a_record() {
        let library = |token: Option<&str>| LibraryStatus {
            name: "tunes".into(),
            folders: vec!["/music".into()],
            listed: true,
            token: token.map(str::to_owned),
            link: token.map(|token| format!("http://x/#library/{token}")),
            files: 2,
            shared: 1,
        };
        let status = Status {
            ready: true,
            folders: vec!["/music".into()],
            downloads: "/d".into(),
            files: 2,
            shared: 2,
            discovered: 1,
            uploading: 2,
            libraries: vec![library(Some("VLD0:k:s")), library(None)],
        };
        let linked = json!({"name": "tunes", "folders": ["/music"], "listed": true, "token": "VLD0:k:s", "link": "http://x/#library/VLD0:k:s", "files": 2, "shared": 1});
        let unpublished = json!({"name": "tunes", "folders": ["/music"], "listed": true, "token": null, "link": null, "files": 2, "shared": 1});
        // nothing in it stands for the libraries together: no link, no name.
        let expected = json!({"ready": true, "folders": ["/music"], "downloads": "/d", "files": 2, "shared": 2, "discovered": 1, "uploading": 2, "libraries": [linked, unpublished]});
        assert_eq!(serde_json::to_value(&status).unwrap(), expected);
    }

    #[test]
    fn unknown_values_are_written_as_null() {
        let waiting = Download {
            id: 1,
            name: None,
            size: None,
            state: "resolving".into(),
            done: 0,
            total: None,
            error: None,
            saved: None,
        };
        let expected = json!({"id": 1, "name": null, "size": null, "state": "resolving", "done": 0, "total": null, "error": null, "saved": null});
        assert_eq!(serde_json::to_value(&waiting).unwrap(), expected);
    }
}
