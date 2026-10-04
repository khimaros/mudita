//! mudita command line: send and fetch files directly between peers over
//! veilid. see DESIGN.md for the protocol. everything that speaks it is in
//! the core (`core/`); this is the shell around it: arguments, logging,
//! files on disk, and the server's socket.

mod client;
mod config;
mod files;
mod serve;

use files::{node_config, throwaway_storage, FileSource, OutputSink};
use mudita::protocol::{link_for, LINK_BASE};
use mudita::rt::Flag;
use mudita::serve::{Embedder, Engine, Tuning};
use mudita::store::Source;
use mudita::Result;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use veilid_core::FmtStripVeilidFields;

const USAGE: &str = "usage:
  mudita send <file>                       share a file until interrupted
  mudita fetch <link-or-token> [-o <path>] download a shared file
  mudita serve [--config <path>]           share the folders in the config
                                           file until interrupted
commands for a running `mudita serve`:
  mudita status                            what it shares and downloads, and
                                           its libraries with a link each
  mudita library add <name> [--listed]     a new library, reached by its
                                           link alone unless it is listed.
                                           nothing tells others which
                                           libraries are one server's
  mudita library share <name> <folder>     share a folder in a library
  mudita library unshare <name> <folder>   stop sharing a folder of one
  mudita library rename <name> <new-name>  call one something else
  mudita library list <name>               have searches find one
  mudita library unlist <name>             have one reached by its link only
  mudita library remove <name>             stop sharing one
  mudita search <words...>                 find files in other libraries
  mudita browse <library-link>             list what a library shares
  mudita download <number|link-or-token>   download a search result, by its
                                           number, or a shared file
  mudita rescan                            look for changes in its folders
  mudita uploads                           who is downloading from it, and
                                           how far each has got
`serve` and the commands for it take --socket <path> to use a control
socket other than the default. `serve --attached` stops when the program
that started it exits. `serve --no-library` shares no folders and keeps
nothing: it is a node for a program to share and download single files
through.";

const CONFIG_OPTION: &str = "--config";
const SOCKET_OPTION: &str = "--socket";
// for a program that starts `serve` as its own backend: the server stops
// when that program exits, by watching its stdin close.
const ATTACHED_FLAG: &str = "--attached";
// for a program that wants a node to share and download single files
// through, by the control methods, and no library: no config, no folders,
// nothing kept between runs.
const NO_LIBRARY_FLAG: &str = "--no-library";
// for `library add`: a library that searches find from the start.
const LISTED_FLAG: &str = "--listed";

// for timing downloads at other settings than the default.
const CONCURRENCY_ENV: &str = "MUDITA_CONCURRENCY";
// test-only: the highest slot a peer will take (see the core's peer.rs).
const TEST_SLOTS_ENV: &str = "MUDITA_TEST_SLOTS";
// test-only: answer the `probe` control method (see the core's serve.rs).
const TEST_PROBE_ENV: &str = "MUDITA_TEST_PROBE";

const LOG_ENV: &str = "RUST_LOG";
// our own events show at info unless RUST_LOG says otherwise. veilid's targets
// are its facility names (net, rpc, ...), which no default directive matches,
// so veilid stays quiet until RUST_LOG asks for it.
const OWN_LOG_DIRECTIVE: &str = "mudita=info";

enum Command {
    Send(PathBuf),
    Fetch(String, Option<PathBuf>),
    Serve(serve::Options),
    /// a command for a running server, and the socket to reach it on.
    Remote(client::Command, Option<PathBuf>),
}

/// take `--name <value>` out of the arguments, wherever it stands.
fn take_option(args: &mut Vec<&str>, name: &str) -> Result<Option<PathBuf>> {
    let Some(at) = args.iter().position(|arg| *arg == name) else {
        return Ok(None);
    };
    if at + 1 >= args.len() {
        return Err(USAGE.into());
    }
    args.remove(at);
    Ok(Some(args.remove(at).into()))
}

/// what `mudita library ...` asks: a change to the node's libraries.
fn parse_arrange(args: &[&str]) -> Result<client::Arrange> {
    use client::Arrange;
    let owned = |text: &str| text.to_owned();
    Ok(match *args {
        ["add", name] => Arrange::Add {
            name: owned(name),
            listed: false,
        },
        ["add", name, LISTED_FLAG] | ["add", LISTED_FLAG, name] => Arrange::Add {
            name: owned(name),
            listed: true,
        },
        ["remove", name] => Arrange::Remove(owned(name)),
        ["rename", name, to] => Arrange::Rename {
            name: owned(name),
            to: owned(to),
        },
        [listing @ ("list" | "unlist"), name] => Arrange::Listed {
            name: owned(name),
            listed: listing == "list",
        },
        ["share", name, folder] => Arrange::Share {
            name: owned(name),
            folder: owned(folder),
        },
        ["unshare", name, folder] => Arrange::Unshare {
            name: owned(name),
            folder: owned(folder),
        },
        _ => return Err(USAGE.into()),
    })
}

fn parse_args(args: &[&str]) -> Result<Command> {
    let mut args = args.to_vec();
    let serving = matches!(args.first(), Some(&"serve"));
    let remote = matches!(
        args.first(),
        Some(&("status" | "uploads" | "library" | "search" | "browse" | "download" | "rescan"))
    );
    let socket = if serving || remote {
        take_option(&mut args, SOCKET_OPTION)?
    } else {
        None
    };
    let config = if serving {
        take_option(&mut args, CONFIG_OPTION)?
    } else {
        None
    };
    let mut flag = |name: &str| {
        let given = serving && args.contains(&name);
        args.retain(|arg| !(serving && *arg == name));
        given
    };
    let (attached, plain) = (flag(ATTACHED_FLAG), flag(NO_LIBRARY_FLAG));
    let remote = |command| Ok(Command::Remote(command, socket.clone()));
    match args[..] {
        ["send", file] => Ok(Command::Send(file.into())),
        ["fetch", link] => Ok(Command::Fetch(link.to_string(), None)),
        ["fetch", link, "-o", path] => Ok(Command::Fetch(link.to_string(), Some(path.into()))),
        ["serve"] => Ok(Command::Serve(serve::Options {
            config,
            socket,
            attached,
            plain,
        })),
        ["status"] => remote(client::Command::Status),
        ["rescan"] => remote(client::Command::Rescan),
        ["uploads"] => remote(client::Command::Uploads),
        ["search", _, ..] => remote(client::Command::Search(args[1..].join(" "))),
        ["library", ..] => remote(client::Command::Library(parse_arrange(&args[1..])?)),
        ["browse", link] => remote(client::Command::Browse(link.to_string())),
        ["download", wanted] => remote(client::Command::Download(wanted.to_string())),
        _ => Err(USAGE.into()),
    }
}

/// which events reach stderr: ours at info, then whatever `spec` adds or
/// overrides in RUST_LOG syntax (`debug`, `net=debug,mudita=warn`).
fn log_filter(spec: &str) -> Result<Targets> {
    let directives: Vec<&str> = std::iter::once(OWN_LOG_DIRECTIVE)
        .chain(spec.split(','))
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .collect();
    Ok(directives.join(",").parse()?)
}

/// install the process-wide subscriber. veilid logs through tracing as well,
/// so this one subscriber prints its events too; VeilidTracing would install
/// a second global subscriber, which fails.
fn init_logging() -> Result<()> {
    let filter = log_filter(&std::env::var(LOG_ENV).unwrap_or_default())?;
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .map_fmt_fields(FmtStripVeilidFields::mapper())
        .with_filter(filter);
    Ok(tracing_subscriber::registry().with(layer).try_init()?)
}

/// what experiments and tests ask of the core through the environment.
/// neither is for users.
pub fn tuning() -> Tuning {
    let asked = |name: &str| std::env::var(name).ok();
    let concurrency = asked(CONCURRENCY_ENV).and_then(|asked| asked.parse().ok());
    Tuning {
        concurrency: concurrency.filter(|asked| *asked > 0),
        slot_limit: asked(TEST_SLOTS_ENV).and_then(|limit| limit.parse().ok()),
        probe: asked(TEST_PROBE_ENV).is_some(),
    }
}

// `send` and `fetch` each run on an engine of their own, whose node is
// started for the command and thrown away after it. `work` is the command;
// the engine runs until `stop` is raised.
async fn on_engine<T>(
    engine: &Engine,
    stop: &Flag,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let storage = throwaway_storage();
    let _ = std::fs::remove_dir_all(&storage);
    let running = engine.run(node_config(&storage), stop.wait());
    let (ran, worked) = tokio::join!(running, work);
    let _ = std::fs::remove_dir_all(&storage);
    // a node that did not start is why the work failed, not the other way.
    ran?;
    worked
}

/// hash the file, join its rendezvous record, print the share token on
/// stdout, and serve pieces until ctrl-c.
async fn send(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()).into());
    }
    let engine = Engine::new(Embedder::default(), tuning());
    let stop = Flag::default();
    let sending = async {
        let sent = async {
            engine.ready().await?;
            let source = Arc::new(FileSource::open(path)?);
            let name = source.name();
            let share = engine.add_share(source)?;
            let token = engine.share_token(share).await?;
            // the bare token goes to stdout for scripts; everything else is
            // logged to stderr. the link says what the file is called here,
            // which the token does not.
            println!("{token}");
            info!("link: {}", link_for(LINK_BASE, &token, &name));
            info!("keep this running; press ctrl-c to stop");
            Ok(())
        };
        let sent = sent.await;
        if sent.is_ok() {
            let _ = tokio::signal::ctrl_c().await;
        }
        stop.raise();
        sent
    };
    on_engine(&engine, &stop, sending).await
}

/// download the file behind a share link or token, to `output` or under
/// the name in the link or, failing that, the one most of its peers give
/// it.
async fn fetch(link: &str, output: Option<PathBuf>) -> Result<()> {
    let engine = Engine::new(Embedder::default(), tuning());
    let stop = Flag::default();
    // a file that is told where to go is called what it is called there.
    let asked = output.as_deref().and_then(Path::file_name);
    let asked = asked.map(|name| name.to_string_lossy().into_owned());
    let fetching = async {
        let fetched = async {
            engine.ready().await?;
            let sink = OutputSink(output);
            engine.download(link, asked.as_deref(), &sink).await
        };
        // said before the node is taken down, which takes a while.
        let fetched = fetched.await.map(|dest| info!("saved {dest}"));
        stop.raise();
        Ok(fetched)
    };
    let fetched = on_engine(&engine, &stop, fetching).await?;
    fetched.map_err(|err| format!("download failed: {err}").into())
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::Serve(options) => serve::serve(options).await,
        // blocking, and quick: it only talks to the server's socket.
        Command::Remote(command, socket) => client::run(command, socket),
        Command::Send(path) => send(&path).await,
        Command::Fetch(link, output) => fetch(&link, output).await,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // before logging exists, a problem can only be reported plainly.
    let command = match parse_args(&args).and_then(|command| init_logging().map(|()| command)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    match run(command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    #[test]
    fn the_subcommands_are_send_and_fetch() {
        assert!(matches!(parse_args(&["send", "a"]), Ok(Command::Send(_))));
        assert!(matches!(
            parse_args(&["fetch", "link"]),
            Ok(Command::Fetch(link, None)) if link == "link"
        ));
        assert!(matches!(
            parse_args(&["fetch", "link", "-o", "out"]),
            Ok(Command::Fetch(_, Some(path))) if path.as_os_str() == "out"
        ));
        for bad in [&["get", "link"][..], &["fetch"], &["fetch", "a", "b"], &[]] {
            assert!(parse_args(bad).is_err());
        }
    }

    #[test]
    fn serve_takes_a_config_and_a_socket() {
        assert!(matches!(
            parse_args(&["serve"]),
            Ok(Command::Serve(serve::Options {
                config: None,
                socket: None,
                attached: false,
                plain: false,
            }))
        ));
        assert!(matches!(
            parse_args(&["serve", "--attached"]),
            Ok(Command::Serve(serve::Options {
                attached: true,
                plain: false,
                ..
            }))
        ));
        assert!(matches!(
            parse_args(&["serve", "--no-library", "--attached"]),
            Ok(Command::Serve(serve::Options {
                attached: true,
                plain: true,
                ..
            }))
        ));
        assert!(parse_args(&["status", "--attached"]).is_err());
        assert!(parse_args(&["status", "--no-library"]).is_err());
        let Ok(Command::Serve(options)) =
            parse_args(&["serve", "--socket", "/s", "--config", "/c.json"])
        else {
            panic!("serve with options did not parse");
        };
        assert_eq!(options.config, Some("/c.json".into()));
        assert_eq!(options.socket, Some("/s".into()));
        assert!(parse_args(&["serve", "--config"]).is_err());
        assert!(parse_args(&["serve", "folder"]).is_err());
    }

    #[test]
    fn remote_commands_parse_with_or_without_a_socket() {
        use client::Command as Remote;
        assert!(matches!(
            parse_args(&["status"]),
            Ok(Command::Remote(Remote::Status, None))
        ));
        assert!(parse_args(&["--socket", "/s", "rescan"]).is_err());
        assert!(matches!(
            parse_args(&["uploads", "--socket", "/s"]),
            Ok(Command::Remote(Remote::Uploads, Some(_)))
        ));
        assert!(parse_args(&["uploads", "now"]).is_err());
        assert!(matches!(
            parse_args(&["rescan", "--socket", "/s"]),
            Ok(Command::Remote(Remote::Rescan, Some(socket))) if socket.as_os_str() == "/s"
        ));
        assert!(matches!(
            parse_args(&["search", "linux", "--socket", "/s", "iso"]),
            Ok(Command::Remote(Remote::Search(query), Some(_))) if query == "linux iso"
        ));
        assert!(matches!(
            parse_args(&["download", "2"]),
            Ok(Command::Remote(Remote::Download(wanted), None)) if wanted == "2"
        ));
        assert!(matches!(
            parse_args(&["browse", "library/VLD0:a:b"]),
            Ok(Command::Remote(Remote::Browse(link), None)) if link == "library/VLD0:a:b"
        ));
        for bad in [
            &["search"][..],
            &["browse"],
            &["download"],
            &["status", "now"],
            &["send", "a", "--socket", "/s"],
            // a server has no library that stands for all of its libraries.
            &["name", "the", "shelf"],
        ] {
            assert!(parse_args(bad).is_err());
        }
    }

    #[test]
    fn the_library_commands_set_out_the_libraries() {
        use client::Arrange;
        let parsed = |args: &[&str]| match parse_args(args) {
            Ok(Command::Remote(client::Command::Library(change), _)) => Some(change),
            _ => None,
        };
        let (name, listed) = ("tunes".to_owned(), false);
        // a new library is reached by its link alone unless it is listed.
        let added = Arrange::Add {
            name: name.clone(),
            listed,
        };
        assert_eq!(parsed(&["library", "add", "tunes"]), Some(added));
        let listed = Arrange::Add {
            name: name.clone(),
            listed: true,
        };
        let asked = ["library", "add", "tunes", "--listed", "--socket", "/s"];
        assert_eq!(parsed(&asked), Some(listed));
        assert_eq!(parsed(&["library", "add", "tunes", "--unlisted"]), None);
        assert_eq!(
            parsed(&["library", "remove", "tunes"]),
            Some(Arrange::Remove(name.clone()))
        );
        let renamed = Arrange::Rename {
            name: name.clone(),
            to: "my music".into(),
        };
        assert_eq!(
            parsed(&["library", "rename", "tunes", "my music"]),
            Some(renamed)
        );
        for (word, listed) in [("list", true), ("unlist", false)] {
            let name = name.clone();
            let expected = Arrange::Listed { name, listed };
            assert_eq!(parsed(&["library", word, "tunes"]), Some(expected));
        }
        let folder = "/music".to_owned();
        let shared = Arrange::Share {
            name: name.clone(),
            folder: folder.clone(),
        };
        assert_eq!(
            parsed(&["library", "share", "tunes", "/music"]),
            Some(shared)
        );
        let unshared = Arrange::Unshare { name, folder };
        assert_eq!(
            parsed(&["library", "unshare", "tunes", "/music"]),
            Some(unshared)
        );
        for bad in [
            &["library"][..],
            &["library", "add"],
            &["library", "rename", "tunes"],
            &["library", "share", "tunes"],
            &["library", "dance", "tunes"],
        ] {
            assert!(parse_args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn own_events_show_at_info_by_default() {
        let filter = log_filter("").unwrap();
        assert!(filter.would_enable("mudita::seed", &Level::INFO));
        assert!(filter.would_enable("mudita", &Level::ERROR));
        assert!(!filter.would_enable("mudita::seed", &Level::DEBUG));
    }

    #[test]
    fn veilid_events_are_quiet_until_asked_for() {
        assert!(!log_filter("").unwrap().would_enable("net", &Level::ERROR));
        let filter = log_filter("net=debug").unwrap();
        assert!(filter.would_enable("net", &Level::DEBUG));
        assert!(!filter.would_enable("rpc", &Level::DEBUG));
        assert!(filter.would_enable("mudita::fetch", &Level::INFO));
    }

    #[test]
    fn a_bare_level_applies_to_everything_else() {
        let filter = log_filter("debug").unwrap();
        assert!(filter.would_enable("rpc", &Level::DEBUG));
        assert!(!filter.would_enable("mudita::seed", &Level::DEBUG));
    }

    #[test]
    fn own_level_can_be_overridden() {
        let filter = log_filter(" mudita=debug, ").unwrap();
        assert!(filter.would_enable("mudita::seed", &Level::DEBUG));
        let filter = log_filter("mudita=warn").unwrap();
        assert!(!filter.would_enable("mudita::seed", &Level::INFO));
    }

    #[test]
    fn a_malformed_spec_is_an_error() {
        assert!(log_filter("net=loud").is_err());
    }
}
