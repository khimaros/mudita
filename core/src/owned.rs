//! the libraries a node's owner has set out (DESIGN.md "a node's
//! libraries"): each a name of the owner's choosing over one or more
//! folders, and whether it is listed where libraries meet. every shared
//! folder is in exactly one. this is the list and the changes to it, all
//! pure; the embedder keeps it (`store::Host`) and the engine shares it.

use crate::protocol::{clean_name, NAME_BYTES};
use crate::Result;
use serde::{Deserialize, Serialize};

/// what the library that downloads go to is called until it is renamed.
pub const DOWNLOADS_NAME: &str = "downloads";
// a catalog names folders the same way whatever the platform's own is.
pub const FOLDER_SEPARATOR: char = '/';
const UNNAMED_FOLDER: &str = "library";
// what a name taken from a folder leaves free of NAME_BYTES, for the " (2)"
// that tells it from another of the same name.
const NUMBER_ROOM: usize = 8;
const NO_SUCH: &str = "no such library";
const KEEPS_DOWNLOADS: &str = "downloads are saved there";

/// one library as its owner set it out.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Owned {
    pub name: String,
    #[serde(default)]
    pub folders: Vec<String>,
    /// whether it holds a slot in the lobby, where a search finds it. one
    /// that is not listed is reached by its link alone, which is how every
    /// library starts out unless its owner says otherwise.
    #[serde(default)]
    pub listed: bool,
}

/// a library's name as it may be kept and sent: cleaned, and not empty, and
/// no path.
pub fn checked_name(name: &str) -> Result<String> {
    let name = clean_name(name)?;
    if name.is_empty() {
        return Err("a library needs a name".into());
    }
    if name.contains(FOLDER_SEPARATOR) {
        return Err("a library's name cannot contain '/'".into());
    }
    Ok(name)
}

/// `text` as a name a library can have, whatever it is: without what a name
/// may not contain, cut to fit with room left to number it, and "library"
/// if nothing is left.
fn fitting_name(text: &str) -> String {
    let usable = |c: &char| !c.is_control() && *c != FOLDER_SEPARATOR;
    let mut fitting: String = text.chars().filter(usable).collect();
    while fitting.len() > NAME_BYTES - NUMBER_ROOM {
        fitting.pop();
    }
    match fitting.trim() {
        "" => UNNAMED_FOLDER.to_owned(),
        name => name.to_owned(),
    }
}

/// what a library made for a folder is called until its owner says
/// otherwise: the last part of the folder's path.
pub fn leaf_name(folder: &str) -> String {
    let mut parts = folder
        .split(FOLDER_SEPARATOR)
        .filter(|part| !part.is_empty());
    fitting_name(parts.next_back().unwrap_or_default())
}

/// `wanted`, or `wanted (2)` and so on: the first that no library has.
pub fn free_name(all: &[Owned], wanted: &str) -> String {
    let taken = |name: &str| all.iter().any(|held| held.name == name);
    let numbered = (2..).map(|n| format!("{wanted} ({n})"));
    let mut candidates = std::iter::once(wanted.to_owned()).chain(numbered);
    candidates.find(|name| !taken(name)).unwrap_or_default()
}

/// the library a folder is in, if any.
pub fn holder_of<'a>(all: &'a [Owned], folder: &str) -> Option<&'a Owned> {
    let holds = |held: &&Owned| held.folders.iter().any(|shared| shared == folder);
    all.iter().find(holds)
}

/// whether the list is one a node can share: every name usable and unlike
/// the others, and no folder in two libraries.
pub fn validate(all: &[Owned]) -> Result<()> {
    for (at, library) in all.iter().enumerate() {
        if checked_name(&library.name)? != library.name {
            return Err(format!("{:?} is no name for a library", library.name).into());
        }
        if all[..at].iter().any(|held| held.name == library.name) {
            return Err(format!("two libraries are called {}", library.name).into());
        }
        for (nth, folder) in library.folders.iter().enumerate() {
            let before = holder_of(&all[..at], folder).is_some();
            if before || library.folders[..nth].contains(folder) {
                return Err(format!("{folder} is shared twice").into());
            }
        }
    }
    Ok(())
}

/// what a config from when a node had one library becomes: one library
/// holding all its folders, called what that library was, or else what its
/// first folder is, and listed as that one was. it is the same library to
/// the network as before (`store::Host::heir`), though its catalog no
/// longer shows each folder's own name. None when it shared no folder but
/// the download folder, which is left for `with_downloads`.
pub fn migrated(folders: &[String], name: &str, downloads: &str) -> Option<Owned> {
    let mut kept: Vec<String> = Vec::new();
    for folder in folders.iter().filter(|folder| *folder != downloads) {
        if !kept.contains(folder) {
            kept.push(folder.clone());
        }
    }
    let called = match name.trim() {
        "" => leaf_name(kept.first()?),
        name => fitting_name(name),
    };
    (!kept.is_empty()).then_some(Owned {
        name: called,
        folders: kept,
        listed: true,
    })
}

/// whether downloads are saved into this library.
pub fn keeps_downloads(library: &Owned, downloads: &str) -> bool {
    library.folders.iter().any(|folder| folder == downloads)
}

/// the list with the download folder in a library: the one it is in, or
/// else one called "downloads", which is made, unlisted, if need be.
pub fn with_downloads(mut all: Vec<Owned>, downloads: &str) -> Vec<Owned> {
    if holder_of(&all, downloads).is_some() {
        return all;
    }
    match all.iter_mut().find(|held| held.name == DOWNLOADS_NAME) {
        Some(held) => held.folders.push(downloads.to_owned()),
        None => all.push(Owned {
            name: DOWNLOADS_NAME.to_owned(),
            folders: vec![downloads.to_owned()],
            listed: false,
        }),
    }
    all
}

fn named<'a>(all: &'a mut [Owned], name: &str) -> Result<&'a mut Owned> {
    let found = all.iter_mut().find(|held| held.name == name);
    found.ok_or_else(|| NO_SUCH.into())
}

/// a new, empty library.
pub fn add(all: &mut Vec<Owned>, name: &str, listed: bool) -> Result<()> {
    let name = checked_name(name)?;
    if all.iter().any(|held| held.name == name) {
        return Err(format!("there is a library called {name} already").into());
    }
    all.push(Owned {
        name,
        folders: Vec::new(),
        listed,
    });
    Ok(())
}

/// stop sharing a library. the one downloads go to stays.
pub fn remove(all: &mut Vec<Owned>, name: &str, downloads: &str) -> Result<()> {
    if keeps_downloads(named(all, name)?, downloads) {
        return Err(KEEPS_DOWNLOADS.into());
    }
    all.retain(|held| held.name != name);
    Ok(())
}

pub fn rename(all: &mut [Owned], name: &str, to: &str) -> Result<()> {
    let to = checked_name(to)?;
    if to != name && all.iter().any(|held| held.name == to) {
        return Err(format!("there is a library called {to} already").into());
    }
    named(all, name)?.name = to;
    Ok(())
}

pub fn set_listed(all: &mut [Owned], name: &str, listed: bool) -> Result<()> {
    named(all, name)?.listed = listed;
    Ok(())
}

/// share a folder in a library. one that is in another is refused; one that
/// is in this one already changes nothing.
pub fn add_folder(all: &mut [Owned], name: &str, folder: &str) -> Result<()> {
    if let Some(holder) = holder_of(all, folder) {
        if holder.name != name {
            return Err(format!("{folder} is shared in {} already", holder.name).into());
        }
        return Ok(());
    }
    named(all, name)?.folders.push(folder.to_owned());
    Ok(())
}

pub fn remove_folder(all: &mut [Owned], name: &str, folder: &str, downloads: &str) -> Result<()> {
    let library = named(all, name)?;
    if folder == downloads {
        return Err(KEEPS_DOWNLOADS.into());
    }
    library.folders.retain(|held| held != folder);
    Ok(())
}

/// share a folder without saying where: in a new library called what the
/// folder is. what a client that knows of folders only asks.
pub fn add_loose_folder(all: &mut Vec<Owned>, folder: &str) -> Result<()> {
    if holder_of(all, folder).is_some() {
        return Ok(());
    }
    let name = free_name(all, &leaf_name(folder));
    add(all, &name, false)?;
    add_folder(all, &name, folder)
}

/// stop sharing a folder without saying where it is: out of the library
/// that has it, which goes too if that leaves it empty.
pub fn remove_loose_folder(all: &mut Vec<Owned>, folder: &str, downloads: &str) -> Result<()> {
    let Some(name) = holder_of(all, folder).map(|holder| holder.name.clone()) else {
        return Ok(());
    };
    remove_folder(all, &name, folder, downloads)?;
    all.retain(|held| held.name != name || !held.folders.is_empty());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWNLOADS: &str = "/home/u/Downloads/mudita";

    fn library(name: &str, folders: &[&str], listed: bool) -> Owned {
        Owned {
            name: name.to_owned(),
            folders: folders.iter().map(|folder| folder.to_string()).collect(),
            listed,
        }
    }

    fn strings(all: &[&str]) -> Vec<String> {
        all.iter().map(|one| one.to_string()).collect()
    }

    #[test]
    fn the_one_library_from_before_stays_one_library() {
        let legacy = strings(&["/home/u/music", "/srv/docs", DOWNLOADS, "/home/u/music"]);
        // called what it was called, with every folder it had but the
        // download folder, and listed as it was.
        let named = migrated(&legacy, "the shelf", DOWNLOADS);
        let folders = ["/home/u/music", "/srv/docs"];
        assert_eq!(named, Some(library("the shelf", &folders, true)));
        // one that had no name is called what its first folder is.
        let unnamed = migrated(&legacy, "", DOWNLOADS);
        assert_eq!(unnamed, Some(library("music", &folders, true)));
        // a name that a library cannot have now is made into one that it can.
        let slashed = migrated(&legacy, "my/shelf", DOWNLOADS).unwrap();
        assert_eq!(slashed.name, "myshelf");
        assert!(validate(&[slashed]).is_ok());
        // one that shared nothing but its downloads leaves nothing here.
        assert_eq!(migrated(&strings(&[DOWNLOADS]), "x", DOWNLOADS), None);
        assert_eq!(migrated(&[], "", DOWNLOADS), None);
    }

    #[test]
    fn downloads_get_a_library_of_their_own_that_is_not_listed() {
        let all = with_downloads(vec![library("music", &["/m"], true)], DOWNLOADS);
        assert_eq!(all[1], library(DOWNLOADS_NAME, &[DOWNLOADS], false));
        // done again, it changes nothing: also not what the owner has since
        // called it, or whether it is listed.
        let mut renamed = all.clone();
        rename(&mut renamed, DOWNLOADS_NAME, "inbox").unwrap();
        set_listed(&mut renamed, "inbox", true).unwrap();
        assert_eq!(with_downloads(renamed.clone(), DOWNLOADS), renamed);
        // a library of that name without the folder is given it.
        let named = with_downloads(vec![library(DOWNLOADS_NAME, &["/d"], true)], DOWNLOADS);
        assert_eq!(named, [library(DOWNLOADS_NAME, &["/d", DOWNLOADS], true)]);
    }

    #[test]
    fn names_are_unique_and_no_folder_is_shared_twice() {
        let fine = [library("a", &["/x"], true), library("b", &["/y"], false)];
        assert!(validate(&fine).is_ok());
        let twins = [library("a", &[], true), library("a", &[], true)];
        assert!(validate(&twins).is_err());
        let shared = [library("a", &["/x"], true), library("b", &["/x"], true)];
        assert!(validate(&shared).is_err());
        assert!(validate(&[library("a", &["/x", "/x"], true)]).is_err());
        for bad in ["", "  ", "a/b", " padded"] {
            assert!(validate(&[library(bad, &[], true)]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn libraries_are_added_renamed_listed_and_removed() {
        let mut all = with_downloads(Vec::new(), DOWNLOADS);
        add(&mut all, " music ", true).unwrap();
        assert_eq!(all[1], library("music", &[], true));
        assert!(add(&mut all, "music", true).is_err());
        assert!(add(&mut all, "", true).is_err());
        assert!(add(&mut all, "a/b", true).is_err());
        rename(&mut all, "music", "tunes").unwrap();
        assert!(rename(&mut all, "music", "x").is_err());
        assert!(rename(&mut all, "tunes", DOWNLOADS_NAME).is_err());
        rename(&mut all, "tunes", "tunes").unwrap();
        set_listed(&mut all, "tunes", false).unwrap();
        assert_eq!(all[1], library("tunes", &[], false));
        assert!(set_listed(&mut all, "nope", true).is_err());
        remove(&mut all, "tunes", DOWNLOADS).unwrap();
        assert!(remove(&mut all, "tunes", DOWNLOADS).is_err());
        // the one downloads go to cannot be taken away, whatever it is called.
        assert!(remove(&mut all, DOWNLOADS_NAME, DOWNLOADS).is_err());
        assert_eq!(all, [library(DOWNLOADS_NAME, &[DOWNLOADS], false)]);
    }

    #[test]
    fn a_folder_is_in_one_library() {
        let mut all = vec![library("a", &[], true), library("b", &[], true)];
        add_folder(&mut all, "a", "/x").unwrap();
        add_folder(&mut all, "a", "/x").unwrap();
        assert_eq!(all[0].folders, ["/x"]);
        let refused = add_folder(&mut all, "b", "/x").unwrap_err();
        assert_eq!(refused.to_string(), "/x is shared in a already");
        assert!(add_folder(&mut all, "c", "/y").is_err());
        remove_folder(&mut all, "a", "/x", DOWNLOADS).unwrap();
        assert!(all[0].folders.is_empty());
        let mut all = with_downloads(all, DOWNLOADS);
        assert!(remove_folder(&mut all, DOWNLOADS_NAME, DOWNLOADS, DOWNLOADS).is_err());
    }

    #[test]
    fn a_folder_shared_without_a_library_gets_one_named_after_it() {
        let mut all = Vec::new();
        add_loose_folder(&mut all, "/home/u/music").unwrap();
        add_loose_folder(&mut all, "/srv/music/").unwrap();
        add_loose_folder(&mut all, "/home/u/music").unwrap();
        let expected = [
            library("music", &["/home/u/music"], false),
            library("music (2)", &["/srv/music/"], false),
        ];
        assert_eq!(all, expected);
        // taken away again, its library goes with it, unless more is in it.
        add_folder(&mut all, "music", "/more").unwrap();
        remove_loose_folder(&mut all, "/home/u/music", DOWNLOADS).unwrap();
        remove_loose_folder(&mut all, "/srv/music/", DOWNLOADS).unwrap();
        remove_loose_folder(&mut all, "/unknown", DOWNLOADS).unwrap();
        assert_eq!(all, [library("music", &["/more"], false)]);
    }

    #[test]
    fn any_folder_gives_a_name_a_library_can_have() {
        assert_eq!(leaf_name("/home/u/music"), "music");
        assert_eq!(leaf_name("/srv/music/"), "music");
        assert_eq!(leaf_name("/"), "library");
        assert_eq!(leaf_name(""), "library");
        // a long name is cut, at a character, with room left to number it.
        let long = format!("/srv/{}", "\u{e9}".repeat(NAME_BYTES));
        let mut all = Vec::new();
        add_loose_folder(&mut all, &long).unwrap();
        add_loose_folder(&mut all, &format!("/other{long}")).unwrap();
        assert!(validate(&all).is_ok());
        assert_eq!(all[0].name.len(), NAME_BYTES - NUMBER_ROOM);
        assert_eq!(all[1].name, format!("{} (2)", all[0].name));
        let rooted = migrated(&[long, "/".into()], "", "/d").unwrap();
        assert!(validate(&[rooted]).is_ok());
    }

    #[test]
    fn a_library_is_reached_by_its_link_alone_unless_its_config_lists_it() {
        let parsed: Owned = serde_json::from_str(r#"{"name": "a"}"#).unwrap();
        assert_eq!(parsed, library("a", &[], false));
        let listed = r#"{"name": "a", "folders": ["/x"], "listed": true}"#;
        let parsed: Owned = serde_json::from_str(listed).unwrap();
        assert_eq!(parsed, library("a", &["/x"], true));
    }

    #[test]
    fn only_the_library_from_before_is_listed_without_being_asked() {
        // it was in the lobby before: who could find it still can.
        let was = migrated(&strings(&["/music"]), "", DOWNLOADS).unwrap();
        assert!(was.listed);
        // every library made since starts out reached by its link alone.
        let mut all = with_downloads(vec![was], DOWNLOADS);
        add_loose_folder(&mut all, "/srv/docs").unwrap();
        let listed: Vec<(&str, bool)> = all
            .iter()
            .map(|held| (held.name.as_str(), held.listed))
            .collect();
        assert_eq!(
            listed,
            [("music", true), ("downloads", false), ("docs", false)]
        );
    }
}
