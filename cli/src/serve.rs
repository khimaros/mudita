//! `danaserve`: a long-standing library. the library itself is the
//! core's engine; this is what makes it a server on this machine: its
//! config file and the folders in it, its state directory, and the unix
//! socket it is driven over by the other `dana` commands and by the
//! desktop app (see the core's control.rs for what is said there).

use crate::config::{default_config_path, process_env, shown, socket_path, state_dir, Config};
use crate::files::{found_in, node_config, throwaway_storage, DirState, DiskFiles, FolderSinks};
use crate::tuning;
use mudita::library::Found;
use mudita::owned::Owned;
use mudita::serve::{Embedder, Engine, LibraryParts};
use mudita::store::{BoxFuture, Files, Host};
use mudita::Result;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;
use tracing::info;

const NODE_FOLDER: &str = "node";
const LOCK_FILE: &str = "lock";
// how long to wait for a server that is leaving the swarm to let go of the
// state directory: about a second per shared file was measured.
const LOCK_WAIT: Duration = Duration::from_secs(30);
const LOCK_RETRY: Duration = Duration::from_millis(250);
// only the user may drive the server.
const SOCKET_MODE: u32 = 0o600;

pub struct Options {
    pub config: Option<PathBuf>,
    pub socket: Option<PathBuf>,
    /// stop when the process that started this one exits.
    pub attached: bool,
    /// be no library: a bare node for single files, driven by its clients.
    pub plain: bool,
}

/// the libraries as the config file sets them out, which is where a change
/// to them is kept.
struct ConfigHost {
    path: PathBuf,
    config: Mutex<Config>,
}

impl ConfigHost {
    // change the config, and rewrite its file.
    fn change(&self, change: impl FnOnce(&mut Config)) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        change(&mut config);
        config.save(&self.path)
    }
}

impl Host for ConfigHost {
    fn libraries(&self) -> Vec<Owned> {
        self.config.lock().unwrap().libraries.clone()
    }

    fn set_libraries(&self, all: Vec<Owned>) -> Result<()> {
        self.change(|config| config.libraries = all)
    }

    // kept for as long as the server runs, also once the file is rewritten:
    // the record may not have been taken over yet.
    fn heir(&self) -> Option<String> {
        self.config.lock().unwrap().heir.clone()
    }

    fn downloads(&self) -> String {
        shown(&self.config.lock().unwrap().downloads)
    }

    fn list(&self) -> BoxFuture<'_, Vec<Found>> {
        Box::pin(found_in(self.libraries()))
    }
}

// one line in, one line out, for as long as the client stays.
async fn converse(engine: Arc<Engine>, stream: UnixStream) {
    let (reading, mut writing) = stream.into_split();
    let mut lines = BufReader::new(reading).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let reply = engine.call(&line).await + "\n";
        if writing.write_all(reply.as_bytes()).await.is_err() {
            break;
        }
    }
}

async fn accept(listener: UnixListener, engine: Arc<Engine>) {
    while let Ok((stream, _)) = listener.accept().await {
        drop(tokio::spawn(converse(engine.clone(), stream)));
    }
}

/// take the control socket. a file left by a server that crashed is
/// replaced; one a live server answers on is not.
fn bind(path: &Path) -> Result<UnixListener> {
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err(format!("a server is already listening at {}", path.display()).into());
    }
    let _ = std::fs::remove_file(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE))?;
    Ok(listener)
}

/// claim the state directory for this process, waiting a while for a server
/// that is leaving to let go of it. the lock goes when the file is dropped,
/// or the process dies.
async fn lock_state(state: &Path) -> Result<File> {
    std::fs::create_dir_all(state)?;
    let lock = File::create(state.join(LOCK_FILE))?;
    let deadline = Instant::now() + LOCK_WAIT;
    while lock.try_lock().is_err() {
        if Instant::now() >= deadline {
            let state = state.display();
            return Err(format!("another `danaserve` is already using {state}").into());
        }
        tokio::time::sleep(LOCK_RETRY).await;
    }
    Ok(lock)
}

/// notified when whatever started this server lets go of its stdin, which a
/// parent does by exiting, however it exits. on a plain thread: a blocking
/// task would keep the runtime from shutting down on a signal.
fn watch_parent() -> Arc<Notify> {
    let gone = Arc::new(Notify::new());
    let notify = gone.clone();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        notify.notify_one();
    });
    gone
}

async fn shutdown_signal(parent_gone: Arc<Notify>) {
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
        () = parent_gone.notified() => info!("the app that started this server is gone"),
    }
}

/// what makes the engine a library on this machine: the folders and name
/// in the config file, the state kept under `state`, and the download
/// folder.
fn library_embedder(config_path: PathBuf, state: &Path) -> Result<Embedder> {
    let config = Config::load(&config_path, &process_env)?;
    std::fs::create_dir_all(&config.downloads)?;
    info!("config: {}", config_path.display());
    Ok(Embedder {
        sinks: Some(Arc::new(FolderSinks(config.downloads.clone()))),
        files: Some(Arc::new(DiskFiles)),
        library: Some(LibraryParts {
            state: Arc::new(DirState(state.to_owned())),
            host: Arc::new(ConfigHost {
                path: config_path,
                config: Mutex::new(config),
            }),
        }),
    })
}

// said once the node is up, which is when the server is of any use.
async fn announce(engine: Arc<Engine>) {
    if engine.ready().await.is_ok() {
        info!("serving; press ctrl-c to stop");
    }
}

/// answer the remote control until interrupted: as a library sharing the
/// configured folders, or with `plain`, as a bare node that shares and
/// downloads only what its clients ask it to, by path, and keeps nothing.
pub async fn serve(options: Options) -> Result<()> {
    let env = &process_env;
    let state = state_dir(env);
    // a library claims its state directory until it ends. a plain node has
    // no state to claim, and throws its node's storage away.
    let (embedder, storage, _lock) = if options.plain {
        let files: Arc<dyn Files> = Arc::new(DiskFiles);
        let embedder = Embedder {
            files: Some(files),
            ..Default::default()
        };
        (embedder, throwaway_storage(), None)
    } else {
        let lock = lock_state(&state).await?;
        let config_path = options.config.unwrap_or_else(|| default_config_path(env));
        let embedder = library_embedder(config_path, &state)?;
        (embedder, state.join(NODE_FOLDER), Some(lock))
    };
    let socket = socket_path(env, options.socket);
    let listener = bind(&socket)?;
    info!("control socket: {}", socket.display());
    let engine = Engine::new(embedder, tuning());
    let parent_gone = if options.attached {
        watch_parent()
    } else {
        Arc::default()
    };
    // before the node, which takes a while: a client can connect at once
    // and is told when the server is ready.
    let accepting = tokio::spawn(accept(listener, engine.clone()));
    let announcing = tokio::spawn(announce(engine.clone()));
    let node = node_config(&storage);
    // leaving the swarm takes seconds, and a server that is leaving must not
    // be taken for a running one: the socket goes before the slots do.
    let stop = async {
        shutdown_signal(parent_gone).await;
        accepting.abort();
        let _ = std::fs::remove_file(&socket);
    };
    let outcome = engine.run(node, stop).await;
    accepting.abort();
    announcing.abort();
    let _ = std::fs::remove_file(&socket);
    if options.plain {
        let _ = std::fs::remove_dir_all(&storage);
    }
    outcome
}
