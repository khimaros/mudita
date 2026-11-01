//! the commands that drive a running `danaserve` over its control
//! socket: `status`, `search`, `download` and `rescan`. they start no node of
//! their own, which is what makes them quick.

use crate::config::{process_env, socket_path};
use mudita::control::{
    decode_response, encode_request, nothing, Download, Downloads, Hit, LibraryStatus, Listed,
    Page, SearchResults, Status, Upload, Uploads,
};
use mudita::fetch::LIBRARY_LINK;
use mudita::protocol::{parse_link, FileEntry, Link};
use mudita::Result;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

// how often a running search is asked how far it has got.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

const PERCENT: u64 = 100;
const FILE_NOT_A_LIBRARY: &str = "that is not a link to a library";

/// a change to how the node's libraries are set out.
#[derive(Debug, PartialEq)]
pub enum Arrange {
    Add { name: String, listed: bool },
    Remove(String),
    Rename { name: String, to: String },
    Listed { name: String, listed: bool },
    Share { name: String, folder: String },
    Unshare { name: String, folder: String },
}

pub enum Command {
    Status,
    Rescan,
    /// who is downloading from this server.
    Uploads,
    Library(Arrange),
    Search(String),
    /// a library link, to list what that library shares.
    Browse(String),
    /// the number of a result of the last search, or a share link or token.
    Download(String),
}

struct Client {
    stream: BufReader<UnixStream>,
    next_id: u64,
}

impl Client {
    fn connect(socket: Option<PathBuf>) -> Result<Client> {
        let path = socket_path(&process_env, socket);
        let stream = UnixStream::connect(&path).map_err(|_| {
            let path = path.display();
            format!("no server is running (nothing answers at {path}); start one with `danaserve`")
        })?;
        Ok(Client {
            stream: BufReader::new(stream),
            next_id: 1,
        })
    }

    fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        let id = self.next_id;
        self.next_id += 1;
        let request = encode_request(id, method, params) + "\n";
        self.stream.get_mut().write_all(request.as_bytes())?;
        let mut line = String::new();
        if self.stream.read_line(&mut line)? == 0 {
            return Err("the server closed the connection".into());
        }
        let (_, outcome) = decode_response(&line)?;
        Ok(serde_json::from_value(outcome?)?)
    }
}

fn format_download(download: &Download) -> String {
    let name = download.name.as_deref().unwrap_or("(looking up)");
    let total = download
        .total
        .map_or("?".to_owned(), |total| total.to_string());
    let error = download.error.as_ref().map(|error| format!(": {error}"));
    let Download {
        id, done, state, ..
    } = download;
    format!(
        "  {id}  {name}  {done}/{total} pieces  {state}{}",
        error.unwrap_or_default()
    )
}

/// one of the node's libraries as `danastatus` shows it: what it is
/// called and how far it is shared, then its folders, then its link once it
/// has one.
fn format_library(library: &LibraryStatus) -> Vec<String> {
    let LibraryStatus {
        name,
        files,
        shared,
        ..
    } = library;
    let reached = if library.listed {
        "discoverable"
    } else {
        "link only"
    };
    let mut lines = vec![format!(
        "  {name} ({reached}, {files} files, {shared} shared)"
    )];
    lines.extend(library.folders.iter().map(|folder| format!("    {folder}")));
    lines.extend(library.link.iter().map(|link| format!("    link: {link}")));
    lines
}

/// the node's libraries, as `danastatus` and `danalibrary` show them.
fn format_libraries(libraries: &[LibraryStatus]) -> Vec<String> {
    let shown = libraries.iter().flat_map(format_library);
    std::iter::once("libraries:".to_owned())
        .chain(shown)
        .collect()
}

/// what `danastatus` prints.
fn format_status(status: &Status, downloads: &[Download]) -> String {
    let mut lines = vec![format!(
        "ready: {}",
        if status.ready { "yes" } else { "not yet" }
    )];
    lines.extend(format_libraries(&status.libraries));
    lines.push(format!("downloads to: {}", status.downloads));
    lines.push(format!(
        "files: {} ({} shared)",
        status.files, status.shared
    ));
    lines.push(format!("libraries discovered: {}", status.discovered));
    lines.push(format!("uploading: {}", status.uploading));
    if !downloads.is_empty() {
        lines.push("downloads:".to_owned());
        lines.extend(downloads.iter().map(format_download));
    }
    lines.join("\n")
}

/// what `danasearch` prints: the results numbered from 1, for
/// `danadownload <number>`.
fn format_results(results: &SearchResults) -> String {
    let asked = format!("asked {} libraries", results.asked);
    if results.results.is_empty() {
        return format!("no files found ({asked})");
    }
    let line = |(index, hit): (usize, &Hit)| {
        let Hit {
            file,
            names,
            sources,
            ..
        } = hit;
        // the name shown is the most common one: the others are counted.
        let others = match names.len().saturating_sub(1) {
            0 => String::new(),
            1 => " (and 1 more name)".to_owned(),
            others => format!(" (and {others} more names)"),
        };
        format!(
            "  {}  {}{others}  {} bytes  in {sources} libraries",
            index + 1,
            file.name,
            file.size
        )
    };
    let mut lines: Vec<String> = results.results.iter().enumerate().map(line).collect();
    lines.push(asked);
    lines.join("\n")
}

/// the params of the download a user asked for: a result of the last
/// search by its number, or else a share link or token.
fn download_params(wanted: &str, last: impl FnOnce() -> Result<SearchResults>) -> Result<Value> {
    if let Link::Library(_) = parse_link(wanted) {
        return Err(LIBRARY_LINK.into());
    }
    let Ok(number) = wanted.parse::<usize>() else {
        return Ok(json!({"token": wanted}));
    };
    let results = last()?.results;
    let hit = number.checked_sub(1).and_then(|index| results.get(index));
    let hit = hit.ok_or_else(|| format!("the last search has no result {number}"))?;
    Ok(json!({"file": hit.file}))
}

/// what `danauploads` prints: a line for each download someone is making
/// from this server, or has made: the file, the library it is in if it is
/// in one, and how far the download has got or how it ended.
fn format_uploads(uploads: &[Upload]) -> String {
    if uploads.is_empty() {
        return "nobody is downloading from here".to_owned();
    }
    let line = |upload: &Upload| {
        let Upload {
            id, name, state, ..
        } = upload;
        let library = upload.library.as_ref();
        let library = library.map_or_else(String::new, |library| format!("  in {library}"));
        let got = match upload.running() {
            true => format!("{}%", upload.done * PERCENT / upload.total.max(1)),
            false => state.clone(),
        };
        format!("  {id}  {name}{library}  {got}")
    };
    uploads.iter().map(line).collect::<Vec<_>>().join("\n")
}

/// one line of `danabrowse`: where the file is and how big.
fn format_file(file: &FileEntry) -> String {
    let FileEntry {
        folder, name, size, ..
    } = file;
    let separator = if folder.is_empty() { "" } else { "/" };
    format!("  {folder}{separator}{name}  {size} bytes")
}

// the whole catalog of the library a link leads to, a page at a time.
fn browse(client: &mut Client, link: &str) -> Result<String> {
    if let Link::File(..) = parse_link(link) {
        return Err(FILE_NOT_A_LIBRARY.into());
    }
    let mut files = Vec::<Listed>::new();
    let name = loop {
        let params = json!({"link": link, "offset": files.len()});
        let page: Page = client.call("browse", params)?;
        let done = page.files.is_empty() || files.len() + page.files.len() >= page.total;
        files.extend(page.files);
        if done {
            break page.library;
        }
    };
    let mut lines: Vec<String> = files
        .iter()
        .map(|listed| format_file(&listed.file))
        .collect();
    let of = if name.is_empty() {
        String::new()
    } else {
        format!(" in {name}")
    };
    lines.push(format!("{} files{of}", files.len()));
    Ok(lines.join("\n"))
}

fn search(client: &mut Client, query: &str) -> Result<String> {
    let started: Value = client.call("search.start", json!({"query": query}))?;
    loop {
        let results: SearchResults =
            client.call("search.poll", json!({"search": started["search"]}))?;
        if results.done {
            return Ok(format_results(&results));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// the control method a change to the libraries is asked by, and its
/// params. a folder is named as the server will find it: in full.
fn arrange_call(change: &Arrange) -> Result<(&'static str, Value)> {
    let whole = |folder: &str| -> Result<String> {
        Ok(std::path::absolute(folder)?.to_string_lossy().into_owned())
    };
    Ok(match change {
        Arrange::Add { name, listed } => ("library.add", json!({"name": name, "listed": listed})),
        Arrange::Remove(name) => ("library.remove", json!({"name": name})),
        Arrange::Rename { name, to } => ("library.rename", json!({"name": name, "to": to})),
        Arrange::Listed { name, listed } => {
            ("library.set", json!({"name": name, "listed": listed}))
        }
        Arrange::Share { name, folder } => (
            "folders.add",
            json!({"library": name, "path": whole(folder)?}),
        ),
        Arrange::Unshare { name, folder } => (
            "folders.remove",
            json!({"library": name, "path": whole(folder)?}),
        ),
    })
}

/// run one command against the server and print what it answers.
pub fn run(command: Command, socket: Option<PathBuf>) -> Result<()> {
    let mut client = Client::connect(socket)?;
    let output = match command {
        Command::Status => {
            let status: Status = client.call("status", nothing())?;
            // a server still starting up has no downloads to list.
            let downloads = client.call::<Downloads>("downloads", nothing());
            format_status(&status, &downloads.map_or(Vec::new(), |all| all.downloads))
        }
        Command::Uploads => {
            let listed: Uploads = client.call("uploads", nothing())?;
            format_uploads(&listed.uploads)
        }
        Command::Rescan => {
            client.call::<Value>("rescan", nothing())?;
            "looking for changes in the shared folders".to_owned()
        }
        Command::Library(change) => {
            let (method, params) = arrange_call(&change)?;
            let status: Status = client.call(method, params)?;
            format_libraries(&status.libraries).join("\n")
        }
        Command::Search(query) => search(&mut client, &query)?,
        Command::Browse(link) => browse(&mut client, &link)?,
        Command::Download(wanted) => {
            let params = download_params(&wanted, || client.call("search.last", nothing()))?;
            let started: Value = client.call("download", params)?;
            format!(
                "download {} started; `danastatus` shows its progress",
                started["download"]
            )
        }
    };
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, sources: usize) -> Hit {
        let file = FileEntry {
            name: name.to_owned(),
            size: 100,
            root: "ab".repeat(32),
            folder: "music".into(),
        };
        Hit {
            file,
            token: "VLD0:a:b".into(),
            names: vec![name.to_owned()],
            sources,
            libraries: Vec::new(),
        }
    }

    fn results(names: &[&str]) -> SearchResults {
        SearchResults {
            asked: 3,
            done: true,
            results: names.iter().map(|name| hit(name, 2)).collect(),
        }
    }

    #[test]
    fn results_are_numbered_from_one() {
        let printed = format_results(&results(&["a.iso", "b.iso"]));
        let expected = "  1  a.iso  100 bytes  in 2 libraries\n  2  b.iso  100 bytes  in 2 libraries\nasked 3 libraries";
        assert_eq!(printed, expected);
        assert_eq!(
            format_results(&results(&[])),
            "no files found (asked 3 libraries)"
        );
    }

    #[test]
    fn a_result_with_other_names_says_how_many_more() {
        let mut found = results(&["a.iso", "b.iso"]);
        found.results[0].names.push("copy.iso".into());
        found.results[1]
            .names
            .extend(["c.iso".into(), "d.iso".into()]);
        let printed = format_results(&found);
        let expected = "  1  a.iso (and 1 more name)  100 bytes  in 2 libraries\n  2  b.iso (and 2 more names)  100 bytes  in 2 libraries\nasked 3 libraries";
        assert_eq!(printed, expected);
    }

    #[test]
    fn a_number_downloads_that_result_of_the_last_search() {
        let params = download_params("2", || Ok(results(&["a.iso", "b.iso"]))).unwrap();
        assert_eq!(params["file"]["name"], "b.iso");
        for missing in ["0", "3"] {
            let refused = download_params(missing, || Ok(results(&["a.iso", "b.iso"])));
            assert!(refused.unwrap_err().to_string().contains("no result"));
        }
        assert!(download_params("1", || Err("no search yet".into())).is_err());
    }

    #[test]
    fn anything_else_downloads_as_a_link_or_token() {
        let unused = || -> Result<SearchResults> { panic!("no search is needed for a token") };
        let params = download_params("VLD0:abc:def", unused).unwrap();
        assert_eq!(params, json!({"token": "VLD0:abc:def"}));
    }

    #[test]
    fn status_lists_folders_counts_and_downloads() {
        let status = Status {
            ready: true,
            folders: vec!["/music".into()],
            downloads: "/d".into(),
            files: 5,
            shared: 4,
            discovered: 2,
            uploading: 1,
            libraries: vec![
                LibraryStatus {
                    name: "tunes".into(),
                    folders: vec!["/music".into(), "/more".into()],
                    listed: true,
                    token: Some("VLD0:t:s".into()),
                    link: Some("http://x/#library/VLD0%3At%3As".into()),
                    files: 5,
                    shared: 4,
                },
                LibraryStatus {
                    name: "downloads".into(),
                    folders: vec!["/d".into()],
                    listed: false,
                    token: None,
                    link: None,
                    files: 0,
                    shared: 0,
                },
            ],
        };
        let running = Download {
            id: 1,
            name: Some("a.iso".into()),
            size: Some(100),
            state: "downloading".into(),
            done: 3,
            total: Some(4),
            error: None,
            saved: None,
        };
        let failed = Download {
            id: 2,
            state: "error".into(),
            error: Some("no peers".into()),
            ..Default::default()
        };
        let printed = format_status(&status, &[running, failed]);
        let expected = "ready: yes\nlibraries:\n  tunes (discoverable, 5 files, 4 shared)\n    /music\n    /more\n    link: http://x/#library/VLD0%3At%3As\n  downloads (link only, 0 files, 0 shared)\n    /d\ndownloads to: /d\nfiles: 5 (4 shared)\nlibraries discovered: 2\nuploading: 1\ndownloads:\n  1  a.iso  3/4 pieces  downloading\n  2  (looking up)  0/? pieces  error: no peers";
        assert_eq!(printed, expected);
        let waiting = Status {
            ready: false,
            ..status
        };
        assert!(format_status(&waiting, &[]).starts_with("ready: not yet\n"));
    }

    #[test]
    fn uploads_are_printed_one_to_a_line() {
        let upload = |id: &str, library: Option<&str>, state: &str, done| Upload {
            id: id.into(),
            name: "a.bin".into(),
            library: library.map(str::to_owned),
            done,
            total: 4,
            size: 100_000,
            state: state.into(),
            started: 10,
            seen: 20,
        };
        let printed = format_uploads(&[
            upload("0102030405060708", Some("tunes"), "downloading", 1),
            upload("a1b2c3d4e5f60718", None, "finished", 4),
            upload("ffffffffffffffff", None, "gone", 2),
        ]);
        let expected = "  0102030405060708  a.bin  in tunes  25%\n  a1b2c3d4e5f60718  a.bin  finished\n  ffffffffffffffff  a.bin  gone";
        assert_eq!(printed, expected);
        assert_eq!(format_uploads(&[]), "nobody is downloading from here");
    }

    #[test]
    fn a_change_to_the_libraries_is_one_control_call() {
        let call = |change: Arrange| arrange_call(&change).unwrap();
        let (name, to) = ("tunes".to_owned(), "music".to_owned());
        assert_eq!(
            call(Arrange::Add {
                name: name.clone(),
                listed: false
            }),
            ("library.add", json!({"name": "tunes", "listed": false}))
        );
        assert_eq!(
            call(Arrange::Remove(name.clone())),
            ("library.remove", json!({"name": "tunes"}))
        );
        assert_eq!(
            call(Arrange::Rename {
                name: name.clone(),
                to
            }),
            ("library.rename", json!({"name": "tunes", "to": "music"}))
        );
        assert_eq!(
            call(Arrange::Listed {
                name: name.clone(),
                listed: true
            }),
            ("library.set", json!({"name": "tunes", "listed": true}))
        );
        // a folder is sent in full: the server runs somewhere else.
        let folder = "/srv/music".to_owned();
        let shared = json!({"library": "tunes", "path": "/srv/music"});
        assert_eq!(
            call(Arrange::Share {
                name: name.clone(),
                folder: folder.clone()
            }),
            ("folders.add", shared.clone())
        );
        assert_eq!(
            call(Arrange::Unshare { name, folder }),
            ("folders.remove", shared)
        );
        let relative = Arrange::Share {
            name: "tunes".into(),
            folder: "music".into(),
        };
        let path = call(relative).1["path"].as_str().unwrap().to_owned();
        assert!(path.starts_with('/') && path.ends_with("/music"), "{path}");
    }

    #[test]
    fn a_library_link_is_browsed_and_not_downloaded() {
        let unused = || -> Result<SearchResults> { panic!("no search is needed for a link") };
        let link = "http://veilid.tech/mudita/#library/VLD0%3Aabc%3Adef";
        let refused = download_params(link, unused).unwrap_err();
        assert!(refused.to_string().contains("dana browse"));
        let unused = || -> Result<SearchResults> { panic!("no search is needed for a link") };
        assert!(download_params("library/VLD0:abc:def", unused).is_err());
    }

    #[test]
    fn a_browsed_file_is_shown_under_its_folder() {
        let file = |folder: &str| FileEntry {
            name: "a.mp3".into(),
            size: 9,
            root: String::new(),
            folder: folder.into(),
        };
        assert_eq!(
            format_file(&file("music/live")),
            "  music/live/a.mp3  9 bytes"
        );
        assert_eq!(format_file(&file("")), "  a.mp3  9 bytes");
    }
}
