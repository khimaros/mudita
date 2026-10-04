//! where `mudita serve` keeps things, and its config file: the folders it
//! shares and where it saves downloads. paths follow the xdg directories;
//! the environment is passed in so that all of it is pure.

use mudita::owned::{migrated, validate, with_downloads, Owned};
use mudita::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APP_FOLDER: &str = "mudita";
const CONFIG_FILE: &str = "serve.json";
const STATE_FOLDER: &str = "serve";
const SOCKET_FILE: &str = "mudita.sock";
const SOCKET_ENV: &str = "MUDITA_SOCKET";

/// reads an environment variable; a parameter so that tests need no real one.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn home(env: Env) -> PathBuf {
    PathBuf::from(env("HOME").unwrap_or_default())
}

fn xdg(env: Env, variable: &str, fallback: &str) -> PathBuf {
    env(variable).map_or_else(|| home(env).join(fallback), PathBuf::from)
}

pub fn default_config_path(env: Env) -> PathBuf {
    let base = xdg(env, "XDG_CONFIG_HOME", ".config");
    base.join(APP_FOLDER).join(CONFIG_FILE)
}

/// leaf hashes, the server's veilid node and its lock live here.
pub fn state_dir(env: Env) -> PathBuf {
    let base = xdg(env, "XDG_STATE_HOME", ".local/state");
    base.join(APP_FOLDER).join(STATE_FOLDER)
}

/// the control socket: the one asked for, else the one named in the
/// environment, else in the runtime directory, else beside the state.
pub fn socket_path(env: Env, asked: Option<PathBuf>) -> PathBuf {
    let named = || env(SOCKET_ENV).map(PathBuf::from);
    let runtime = || env("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(SOCKET_FILE));
    asked
        .or_else(named)
        .or_else(runtime)
        .unwrap_or_else(|| state_dir(env).join(SOCKET_FILE))
}

fn default_downloads(env: Env) -> PathBuf {
    home(env).join("Downloads").join(APP_FOLDER)
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Config {
    /// the libraries: every file under the folders of one is shared in it.
    /// the one that has the download folder is where downloads go.
    pub libraries: Vec<Owned>,
    /// where downloads are saved. a folder of one of the libraries, so
    /// what is downloaded is offered on.
    pub downloads: PathBuf,
    /// the library that is what the node's one library was, by name, when
    /// this was read from a file written while a node had one (see the
    /// core's `store::Host::heir`). never written.
    #[serde(skip)]
    pub heir: Option<String>,
}

pub fn shown(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// what a config file from when a node had one library becomes: that
/// library with all its folders, and one for the download folder; and the
/// name of the one that is the old library to the network. that is the
/// first, or the downloads' own where the old one shared nothing else.
fn from_before(folders: &[String], name: &str, downloads: &str) -> (Vec<Owned>, Option<String>) {
    let was = migrated(folders, name, downloads);
    let libraries = with_downloads(was.into_iter().collect(), downloads);
    let heir = libraries.first().map(|library| library.name.clone());
    (libraries, heir)
}

impl Config {
    /// parse a config file. `downloads` may be left out. a file written
    /// when a node had one library lists `folders` and maybe a `name`.
    pub fn parse(json: &str, env: Env) -> Result<Config> {
        #[derive(Deserialize)]
        struct Written {
            #[serde(default)]
            folders: Vec<String>,
            libraries: Option<Vec<Owned>>,
            downloads: Option<PathBuf>,
            #[serde(default)]
            name: String,
        }
        let written: Written = serde_json::from_str(json)?;
        let downloads = written.downloads.unwrap_or_else(|| default_downloads(env));
        let (libraries, heir) = match written.libraries {
            Some(set_out) => (with_downloads(set_out, &shown(&downloads)), None),
            None => from_before(&written.folders, &written.name, &shown(&downloads)),
        };
        validate(&libraries)?;
        Ok(Config {
            libraries,
            downloads,
            heir,
        })
    }

    /// read the config at `path`. a missing one is created, sharing only the
    /// download folder.
    pub fn load(path: &Path, env: Env) -> Result<Config> {
        if let Ok(json) = std::fs::read_to_string(path) {
            let parsed = Config::parse(&json, env);
            return parsed.map_err(|err| format!("{}: {err}", path.display()).into());
        }
        let downloads = default_downloads(env);
        let config = Config {
            libraries: with_downloads(Vec::new(), &shown(&downloads)),
            downloads,
            heir: None,
        };
        config.save(path)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, serde_json::to_string_pretty(self)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            let found = pairs.iter().find(|(key, _)| *key == name);
            found.map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn paths_follow_the_xdg_directories() {
        let env = env_of(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/c"),
            ("XDG_STATE_HOME", "/s"),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
        ]);
        assert_eq!(default_config_path(&env), Path::new("/c/mudita/serve.json"));
        assert_eq!(state_dir(&env), Path::new("/s/mudita/serve"));
        assert_eq!(
            socket_path(&env, None),
            Path::new("/run/user/1/mudita.sock")
        );
    }

    #[test]
    fn paths_fall_back_to_the_home_directory() {
        let env = env_of(&[("HOME", "/home/u")]);
        let config = Path::new("/home/u/.config/mudita/serve.json");
        assert_eq!(default_config_path(&env), config);
        let state = Path::new("/home/u/.local/state/mudita/serve");
        assert_eq!(state_dir(&env), state);
        assert_eq!(socket_path(&env, None), state.join("mudita.sock"));
    }

    #[test]
    fn a_socket_asked_for_wins_over_one_in_the_environment() {
        let env = env_of(&[("MUDITA_SOCKET", "/e.sock"), ("XDG_RUNTIME_DIR", "/run")]);
        assert_eq!(socket_path(&env, None), Path::new("/e.sock"));
        assert_eq!(
            socket_path(&env, Some("/a.sock".into())),
            Path::new("/a.sock")
        );
    }

    fn library(name: &str, folders: &[&str], listed: bool) -> Owned {
        Owned {
            name: name.to_owned(),
            folders: folders.iter().map(|folder| folder.to_string()).collect(),
            listed,
        }
    }

    #[test]
    fn a_config_from_when_a_node_had_one_library_keeps_it_as_one() {
        let env = env_of(&[("HOME", "/home/u")]);
        let legacy = r#"{"folders": ["/home/u/music", "/srv/docs"], "name": "the shelf"}"#;
        let config = Config::parse(legacy, &env).unwrap();
        assert_eq!(config.downloads, Path::new("/home/u/Downloads/mudita"));
        // every folder in one library, called what the library was called,
        // and listed as it was; downloads apart, and not listed.
        let expected = [
            library("the shelf", &["/home/u/music", "/srv/docs"], true),
            library("downloads", &["/home/u/Downloads/mudita"], false),
        ];
        assert_eq!(config.libraries, expected);
        // it is the same library to the network: it takes over the record.
        assert_eq!(config.heir.as_deref(), Some("the shelf"));
        // written again, it is in the new shape, and no longer from before.
        let written = serde_json::to_string(&config).unwrap();
        assert!(!written.contains("heir") && !written.contains("the shelf\",\"downloads"));
        let reread = Config::parse(&written, &env).unwrap();
        assert_eq!((reread.libraries, reread.heir), (config.libraries, None));
        assert!(Config::parse("{", &env).is_err());
    }

    #[test]
    fn a_library_from_before_without_a_name_is_called_what_its_first_folder_is() {
        let env = env_of(&[]);
        let json = r#"{"folders": ["/d", "/srv/music", "/m"], "downloads": "/d"}"#;
        let config = Config::parse(json, &env).unwrap();
        let expected = [
            library("music", &["/srv/music", "/m"], true),
            library("downloads", &["/d"], false),
        ];
        assert_eq!(config.libraries, expected);
        assert_eq!(config.heir.as_deref(), Some("music"));
        // one that shared nothing but its downloads: those are what its
        // link showed, so their library takes the record over.
        let bare = Config::parse(r#"{"folders": [], "downloads": "/d"}"#, &env).unwrap();
        assert_eq!(bare.libraries, [library("downloads", &["/d"], false)]);
        assert_eq!(bare.heir.as_deref(), Some("downloads"));
    }

    #[test]
    fn libraries_are_read_as_written_and_checked() {
        let env = env_of(&[("HOME", "/home/u")]);
        let json = r#"{"libraries": [
            {"name": "tunes", "folders": ["/music", "/more"]},
            {"name": "inbox", "folders": ["/d"], "listed": true}
        ], "downloads": "/d", "folders": ["/ignored"]}"#;
        let config = Config::parse(json, &env).unwrap();
        // one that does not say it is listed is reached by its link alone.
        let expected = [
            library("tunes", &["/music", "/more"], false),
            library("inbox", &["/d"], true),
        ];
        assert_eq!((config.libraries, config.heir), (expected.to_vec(), None));
        for bad in [
            r#"{"libraries": [{"name": "a"}, {"name": "a"}]}"#,
            r#"{"libraries": [{"name": ""}]}"#,
            r#"{"libraries": [{"name": "a/b"}]}"#,
            r#"{"libraries": [{"name": "a", "folders": ["/x"]}, {"name": "b", "folders": ["/x"]}]}"#,
        ] {
            assert!(Config::parse(bad, &env).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_missing_config_is_created_and_read_back() {
        let env = env_of(&[("HOME", "/home/u")]);
        let dir = std::env::temp_dir().join(format!("mudita-config-{}", std::process::id()));
        let path = dir.join("nested/serve.json");
        let created = Config::load(&path, &env).unwrap();
        let downloads = library("downloads", &["/home/u/Downloads/mudita"], false);
        assert_eq!(created.libraries, [downloads]);
        assert_eq!(Config::load(&path, &env).unwrap(), created);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
