//! the core's files, downloads and kept state as they are on a disk: what
//! the command line hands the core, which has no filesystem of its own (see
//! the core's store.rs).

use mudita::library::{Found, Stamp};
use mudita::owned::Owned;
use mudita::protocol::{piece_range, save_name};
use mudita::store::{BoxFuture, Files, Sink, Sinks, Source, StateStore};
use mudita::Result;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tokio::task::spawn_blocking;
use tracing::warn;
use veilid_core::{VeilidConfig, VeilidConfigProtectedStore, VeilidConfigTableStore};

const PROGRAM_NAME: &str = "mudita";
// pieces land in a sibling file that is renamed once every piece verifies.
pub const PART_SUFFIX: &str = ".part";
const PART_EXTENSION: &str = "part";
const HIDDEN_PREFIX: char = '.';
// a catalog names folders the same way whatever the platform's own is.
const FOLDER_SEPARATOR: &str = "/";
// kept state includes the secret a library speaks with: the user's alone.
const STATE_MODE: u32 = 0o600;

/// the config of a veilid node that keeps its storage under `storage`. two
/// nodes (a `send` and a `fetch` on one machine) cannot share a table
/// store, so each gets storage of its own.
pub fn node_config(storage: &Path) -> VeilidConfig {
    let directory = |name: &str| storage.join(name).to_string_lossy().into_owned();
    VeilidConfig {
        program_name: PROGRAM_NAME.into(),
        protected_store: VeilidConfigProtectedStore {
            // nothing worth protecting is stored: a node's identity says
            // nothing about its user, and most are thrown away on exit.
            always_use_insecure_storage: true,
            directory: directory("protected_store"),
            ..Default::default()
        },
        table_store: VeilidConfigTableStore {
            directory: directory("table_store"),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// where a node that is thrown away at exit keeps its storage meanwhile.
pub fn throwaway_storage() -> PathBuf {
    std::env::temp_dir().join(format!("{PROGRAM_NAME}-{}", std::process::id()))
}

fn read_range(path: &Path, start: u64, end: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (end - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

// disk reads and writes block, so they run off the runtime's own threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    spawn_blocking(work).await?
}

/// a file on disk, to share.
pub struct FileSource {
    path: PathBuf,
    size: u64,
}

impl FileSource {
    pub fn new(path: &Path, size: u64) -> FileSource {
        FileSource {
            path: path.to_owned(),
            size,
        }
    }

    pub fn open(path: &Path) -> Result<FileSource> {
        Ok(FileSource::new(path, std::fs::metadata(path)?.len()))
    }
}

impl Source for FileSource {
    fn name(&self) -> String {
        let name = self.path.file_name().unwrap_or_default();
        name.to_string_lossy().into_owned()
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn read(&self, start: u64, end: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        let path = self.path.clone();
        Box::pin(blocking(move || read_range(&path, start, end)))
    }
}

/// a download on its way to `dest`, in a `.part` file beside it.
struct FileSink {
    dest: PathBuf,
    part: PathBuf,
    size: u64,
    saved: AtomicBool,
}

fn part_of(dest: &Path) -> PathBuf {
    let mut part = dest.to_owned().into_os_string();
    part.push(PART_SUFFIX);
    part.into()
}

impl FileSink {
    fn create(dest: PathBuf, size: u64) -> Result<Arc<dyn Sink>> {
        let part = part_of(&dest);
        File::create(&part)?.set_len(size)?;
        Ok(Arc::new(FileSink {
            dest,
            part,
            size,
            saved: AtomicBool::new(false),
        }))
    }
}

impl Sink for FileSink {
    fn write(&self, index: u64, piece: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        let (part, (start, _)) = (self.part.clone(), piece_range(index, self.size));
        Box::pin(blocking(move || {
            let mut file = OpenOptions::new().write(true).open(part)?;
            file.seek(SeekFrom::Start(start))?;
            Ok(file.write_all(&piece)?)
        }))
    }

    fn read(&self, index: u64) -> BoxFuture<'_, Result<Vec<u8>>> {
        let saved = self.saved.load(Ordering::Relaxed);
        let path = if saved { &self.dest } else { &self.part }.clone();
        let (start, end) = piece_range(index, self.size);
        Box::pin(blocking(move || read_range(&path, start, end)))
    }

    fn finish(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            std::fs::rename(&self.part, &self.dest)?;
            self.saved.store(true, Ordering::Relaxed);
            Ok(self.dest.display().to_string())
        })
    }

    // only the unfinished file is dropped: a saved one is the user's.
    fn discard(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if !self.saved.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&self.part);
            }
        })
    }
}

// the name comes from a stranger, so only its final component is used.
fn destination(name: &str, output: Option<PathBuf>) -> Result<PathBuf> {
    let dest = output.unwrap_or_else(|| PathBuf::from(save_name(name)));
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()).into());
    }
    Ok(dest)
}

/// where `fetch` puts its one download: the path asked for, or the file's
/// name in the current directory, and never over a file that is there.
pub struct OutputSink(pub Option<PathBuf>);

impl Sinks for OutputSink {
    fn create(&self, name: &str, size: u64) -> BoxFuture<'_, Result<Arc<dyn Sink>>> {
        let dest = destination(name, self.0.clone());
        Box::pin(async move { FileSink::create(dest?, size) })
    }
}

/// the disk, to a client that names files by path.
pub struct DiskFiles;

impl Files for DiskFiles {
    fn open(&self, path: &str) -> BoxFuture<'_, Result<Arc<dyn Source>>> {
        let path = PathBuf::from(path);
        Box::pin(async move {
            if !path.is_file() {
                return Err(format!("{} is not a file", path.display()).into());
            }
            Ok(Arc::new(FileSource::open(&path)?) as Arc<dyn Source>)
        })
    }

    fn sink_at(&self, path: &str) -> Arc<dyn Sinks> {
        Arc::new(OutputSink(Some(path.into())))
    }
}

/// a free path for a file called `name` in `folder`: its own name, or the
/// name with a number when that is taken. the name comes from a stranger,
/// so only its final component is used.
fn free_path(folder: &Path, name: &str) -> PathBuf {
    let name = save_name(name);
    let name = Path::new(&name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let extension = name
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()));
    let numbered = |n: u32| format!("{stem} ({n}){}", extension.as_deref().unwrap_or_default());
    let candidates = std::iter::once(name.to_owned()).chain((2..).map(|n| numbered(n).into()));
    let taken = |path: &PathBuf| path.exists() || part_of(path).exists();
    let mut paths = candidates.map(|candidate| folder.join(candidate));
    paths.find(|path| !taken(path)).unwrap_or_default()
}

/// where a server's downloads go: its download folder, each under the
/// first name that is free there.
pub struct FolderSinks(pub PathBuf);

impl Sinks for FolderSinks {
    fn create(&self, name: &str, size: u64) -> BoxFuture<'_, Result<Arc<dyn Sink>>> {
        let name = name.to_owned();
        Box::pin(async move {
            std::fs::create_dir_all(&self.0)?;
            FileSink::create(free_path(&self.0, &name), size)
        })
    }
}

/// what a server keeps between runs, each value a file under one directory.
pub struct DirState(pub PathBuf);

impl StateStore for DirState {
    fn get(&self, name: &str) -> BoxFuture<'_, Option<Vec<u8>>> {
        let path = self.0.join(name);
        Box::pin(async move { std::fs::read(path).ok() })
    }

    fn put(&self, name: &str, value: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        let path = self.0.join(name);
        Box::pin(async move {
            std::fs::create_dir_all(path.parent().ok_or("no state directory")?)?;
            std::fs::write(&path, value)?;
            let mode = std::fs::Permissions::from_mode(STATE_MODE);
            Ok(std::fs::set_permissions(path, mode)?)
        })
    }
}

/// whether a file under a shared folder is shared: hidden files, files in
/// hidden folders, and unfinished downloads are not.
fn shareable(path: &Path, folder: &Path) -> bool {
    let Ok(inside) = path.strip_prefix(folder) else {
        return false;
    };
    let hidden = |part: std::path::Component| {
        let name = part.as_os_str().to_string_lossy();
        name.starts_with(HIDDEN_PREFIX)
    };
    !inside.components().any(hidden) && path.extension().is_none_or(|ext| ext != PART_EXTENSION)
}

fn stamp_of(metadata: &std::fs::Metadata) -> Stamp {
    let modified = metadata.modified().ok();
    let since = modified.and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    (
        metadata.len(),
        since.map_or(0, |since| since.as_millis() as u64),
    )
}

// every regular file below `folder`. one that cannot be read is skipped, so
// a bad mount does not unshare the rest.
fn list_folder(folder: &Path, at: &Path, found: &mut BTreeMap<PathBuf, Stamp>) {
    let Ok(listing) = std::fs::read_dir(at) else {
        warn!("could not read {}", at.display());
        return;
    };
    for entry in listing.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            list_folder(folder, &path, found);
        } else if metadata.is_file() && shareable(&path, folder) {
            found.insert(path, stamp_of(&metadata));
        }
    }
}

/// every file to share under the folders, with how it looks now.
fn list_files(folders: &[PathBuf]) -> BTreeMap<PathBuf, Stamp> {
    let mut found = BTreeMap::new();
    for folder in folders {
        list_folder(folder, folder, &mut found);
    }
    found
}

/// where the file at `path` is in the library: the name of the library
/// whose folder it is under, and its directory inside that folder,
/// '/'-separated and empty at the top. neither the folder's own name nor
/// anything above it is in there: a library is called what its owner
/// chose. when shared folders nest, the innermost counts. None for a file
/// under none of them.
fn place_of(path: &Path, all: &[Owned]) -> Option<(String, String)> {
    let shared = all.iter().flat_map(|library| {
        let folders = library.folders.iter().map(Path::new);
        folders.map(move |folder| (library, folder))
    });
    let (library, folder) = shared
        .filter(|(_, folder)| path.starts_with(folder))
        .max_by_key(|(_, folder)| folder.components().count())?;
    let inside = path.parent()?.strip_prefix(folder).ok()?;
    let names = inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy());
    let inside = names.collect::<Vec<_>>().join(FOLDER_SEPARATOR);
    Some((library.name.clone(), inside))
}

/// every file under the folders of the libraries as the core is to share
/// it, in path order.
pub async fn found_in(all: Vec<Owned>) -> Vec<Found> {
    let listed = spawn_blocking(move || {
        let folders = all.iter().flat_map(|library| &library.folders);
        let files = list_files(&folders.map(PathBuf::from).collect::<Vec<_>>());
        let found = |(path, stamp): (PathBuf, Stamp)| {
            let (library, folder) = place_of(&path, &all)?;
            Some(Found {
                id: path.to_string_lossy().into_owned(),
                stamp,
                library,
                folder,
                source: Arc::new(FileSource::new(&path, stamp.0)),
            })
        };
        files.into_iter().filter_map(found).collect()
    });
    listed.await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mudita::protocol::FALLBACK_NAME;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mudita-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hidden_files_and_unfinished_downloads_are_not_shared() {
        let folder = Path::new("/music");
        assert!(shareable(Path::new("/music/a.mp3"), folder));
        assert!(shareable(Path::new("/music/album/b.mp3"), folder));
        assert!(shareable(Path::new("/music/part"), folder));
        assert!(!shareable(Path::new("/music/.secret"), folder));
        assert!(!shareable(Path::new("/music/.git/config"), folder));
        assert!(!shareable(Path::new("/music/half.iso.part"), folder));
        assert!(!shareable(Path::new("/elsewhere/a.mp3"), folder));
        // a folder that is itself hidden is shared when asked for by name.
        assert!(shareable(
            Path::new("/home/.cache/a"),
            Path::new("/home/.cache")
        ));
    }

    #[test]
    fn folders_are_listed_all_the_way_down() {
        let folder = temp_dir("list");
        for file in ["a.txt", "sub/deep/b.txt", ".hidden", "sub/c.part"] {
            let path = folder.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, file).unwrap();
        }
        let missing = folder.join("no-such-folder");
        let found = list_files(&[folder.clone(), missing]);
        let paths: Vec<&PathBuf> = found.keys().collect();
        assert_eq!(
            paths,
            [&folder.join("a.txt"), &folder.join("sub/deep/b.txt")]
        );
        assert_eq!(found[&folder.join("a.txt")].0, 5);
        std::fs::remove_dir_all(folder).unwrap();
    }

    fn library(name: &str, folders: &[&str]) -> Owned {
        Owned {
            name: name.to_owned(),
            folders: folders.iter().map(|folder| folder.to_string()).collect(),
            listed: true,
        }
    }

    fn placed(library: &str, inside: &str) -> Option<(String, String)> {
        Some((library.to_owned(), inside.to_owned()))
    }

    #[test]
    fn a_file_is_placed_by_its_library_and_its_path_inside_the_folder() {
        let all = [
            library("tunes", &["/home/u/music", "/srv/more music"]),
            library("papers", &["/srv/docs"]),
        ];
        let place = |path: &str| place_of(Path::new(path), &all);
        // the folder's own name is no part of it: the library's is used.
        assert_eq!(place("/home/u/music/a.mp3"), placed("tunes", ""));
        assert_eq!(
            place("/home/u/music/live/1999/b.mp3"),
            placed("tunes", "live/1999")
        );
        // two folders of one library share its top.
        assert_eq!(place("/srv/more music/c.mp3"), placed("tunes", ""));
        assert_eq!(place("/srv/docs/tax/d.txt"), placed("papers", "tax"));
        assert_eq!(place("/elsewhere/e.txt"), None);
    }

    #[test]
    fn nested_shared_folders_place_a_file_by_the_innermost() {
        let all = [
            library("home", &["/home/u"]),
            library("tunes", &["/home/u/music"]),
        ];
        let place = |path: &str| place_of(Path::new(path), &all);
        assert_eq!(place("/home/u/music/live/a.mp3"), placed("tunes", "live"));
        assert_eq!(place("/home/u/notes/b.txt"), placed("home", "notes"));
        let root = [library("all", &["/"])];
        assert_eq!(
            place_of(Path::new("/etc/hosts"), &root),
            placed("all", "etc")
        );
        assert_eq!(place_of(Path::new("/hosts"), &root), placed("all", ""));
    }

    #[tokio::test]
    async fn the_files_found_say_which_library_each_is_in() {
        let folder = temp_dir("found");
        for file in ["music/a.mp3", "music/live/b.mp3", "docs/c.txt"] {
            let path = folder.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, file).unwrap();
        }
        let shared = |name: &str| folder.join(name).to_string_lossy().into_owned();
        let all = vec![
            library("papers", &[&shared("docs")]),
            library("tunes", &[&shared("music")]),
        ];
        let found = found_in(all).await;
        let places: Vec<(&str, &str, String)> = found
            .iter()
            .map(|file| (&*file.library, &*file.folder, file.source.name()))
            .collect();
        let expected = [
            ("papers", "", "c.txt".to_owned()),
            ("tunes", "", "a.mp3".to_owned()),
            ("tunes", "live", "b.mp3".to_owned()),
        ];
        assert_eq!(places, expected);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_download_takes_the_first_free_name() {
        let folder = temp_dir("free");
        assert_eq!(free_path(&folder, "a.tar.gz"), folder.join("a.tar.gz"));
        std::fs::write(folder.join("a.tar.gz"), "").unwrap();
        assert_eq!(free_path(&folder, "a.tar.gz"), folder.join("a.tar (2).gz"));
        std::fs::write(folder.join("a.tar (2).gz.part"), "").unwrap();
        assert_eq!(free_path(&folder, "a.tar.gz"), folder.join("a.tar (3).gz"));
        std::fs::write(folder.join("plain"), "").unwrap();
        assert_eq!(free_path(&folder, "plain"), folder.join("plain (2)"));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_name_never_escapes_the_download_folder() {
        let folder = Path::new("/downloads");
        assert_eq!(free_path(folder, "../../etc/passwd"), folder.join("passwd"));
        assert_eq!(free_path(folder, "/abs/evil.sh"), folder.join("evil.sh"));
        assert_eq!(free_path(folder, ".."), folder.join(FALLBACK_NAME));
        assert_eq!(free_path(folder, ""), folder.join(FALLBACK_NAME));
    }

    #[test]
    fn a_fetch_goes_to_the_path_asked_for_or_else_under_the_files_name() {
        let asked = Some(PathBuf::from("/no/such/out.bin"));
        let dest = destination("a.bin", asked.clone()).unwrap();
        assert_eq!(Some(dest), asked);
        let dest = destination("../no-such-mudita-file.bin", None).unwrap();
        assert_eq!(dest, PathBuf::from("no-such-mudita-file.bin"));
    }

    #[tokio::test]
    async fn a_download_is_a_part_file_until_it_is_finished() {
        let folder = temp_dir("sink");
        let sinks = FolderSinks(folder.clone());
        let sink = sinks.create("a.bin", 4).await.unwrap();
        sink.write(0, b"abcd".to_vec()).await.unwrap();
        assert!(folder.join("a.bin.part").exists() && !folder.join("a.bin").exists());
        assert_eq!(sink.read(0).await.unwrap(), b"abcd");
        assert_eq!(
            sink.finish().await.unwrap(),
            folder.join("a.bin").display().to_string()
        );
        // it is read back from where it was saved, and never dropped again.
        assert_eq!(sink.read(0).await.unwrap(), b"abcd");
        sink.discard().await;
        assert_eq!(std::fs::read(folder.join("a.bin")).unwrap(), b"abcd");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn a_discarded_download_leaves_nothing_and_none_overwrites() {
        let folder = temp_dir("discard");
        let dest = folder.join("out.bin");
        let sink = OutputSink(Some(dest.clone()));
        let sink = sink.create("a.bin", 4).await.unwrap();
        sink.discard().await;
        assert!(!part_of(&dest).exists());
        std::fs::write(&dest, "keep me").unwrap();
        let refused = OutputSink(Some(dest.clone())).create("a.bin", 4).await;
        assert!(refused.is_err_and(|err| err.to_string().contains("already exists")));
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn kept_state_is_read_back_and_is_the_users_alone() {
        let dir = temp_dir("state");
        let state = DirState(dir.clone());
        assert_eq!(state.get("leaves/abc").await, None);
        state.put("leaves/abc", vec![1, 2, 3]).await.unwrap();
        assert_eq!(state.get("leaves/abc").await, Some(vec![1, 2, 3]));
        let mode = std::fs::metadata(dir.join("leaves/abc")).unwrap();
        assert_eq!(mode.permissions().mode() & 0o777, STATE_MODE);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
