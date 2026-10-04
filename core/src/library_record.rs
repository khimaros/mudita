//! the record a link to a library points at (DESIGN.md "library record and
//! link"). its one subkey names the library's current route, so the link
//! outlives every route. unlike a file's record nothing about it is
//! derived: its key and owner are made once and kept between runs. a node
//! keeps one for each of its libraries, by the library's name.

use crate::protocol::LIBRARY_SUBKEYS;
use crate::store::StateStore;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;
use veilid_core::{DHTSchema, KeyPair, RecordKey, RoutingContext, CRYPTO_KIND_VLD0};

// what the records are kept as. they hold the secrets that let their holder
// speak as these libraries, so the store must keep them to the user.
const RECORDS_NAME: &str = "libraries.json";
// the one record a node kept when it had one library, which the library
// that one became takes over (`store::Host::heir`).
const LEGACY_NAME: &str = "library.json";

/// what is kept of a record between runs: its key, which is the library
/// token, and the keypair that may write it.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Kept {
    pub key: String,
    pub owner: String,
}

/// the records kept, by what each library is called. every one has a key
/// and an owner of its own, made apart from the others', so that nothing
/// about one record says which others are the same node's. empty when
/// there are none, or they cannot be read: new records are made then, and
/// the libraries get new links.
pub async fn load_kept(state: &dyn StateStore) -> BTreeMap<String, Kept> {
    let kept = state.get(RECORDS_NAME).await;
    let parsed = kept.and_then(|kept| serde_json::from_slice(&kept).ok());
    parsed.unwrap_or_default()
}

async fn save_kept(state: &dyn StateStore, kept: &BTreeMap<String, Kept>) -> Result<()> {
    let json = serde_json::to_string_pretty(kept)?;
    state.put(RECORDS_NAME, json.into_bytes()).await
}

fn schema() -> Result<DHTSchema> {
    Ok(DHTSchema::dflt(LIBRARY_SUBKEYS)?)
}

// open a kept record for writing. a node whose storage was lost no longer
// holds it; it is then made again under the same owner, which gives the
// same record, and reopened under the kept key, as a share's record is.
async fn reopen(context: &RoutingContext, kept: &Kept) -> Result<RecordKey> {
    let key = RecordKey::from_str(&kept.key)?;
    let owner = KeyPair::from_str(&kept.owner)?;
    let opened = context.open_dht_record(key.clone(), Some(owner.clone()));
    if opened.await.is_ok() {
        return Ok(key);
    }
    let created = context
        .create_dht_record(CRYPTO_KIND_VLD0, schema()?, Some(owner.clone()))
        .await?;
    context.close_dht_record(created.key()).await?;
    let reopened = context.open_dht_record(key.clone(), Some(owner));
    reopened.await.map(|_| key).map_err(Into::into)
}

// a new record, open for writing, and what to keep of it.
async fn create(context: &RoutingContext) -> Result<(RecordKey, Kept)> {
    let created = context
        .create_dht_record(CRYPTO_KIND_VLD0, schema()?, None)
        .await?;
    let owner = created
        .owner_keypair()
        .ok_or("a new record has no owner secret")?;
    let kept = Kept {
        key: created.key().to_string(),
        owner: owner.to_string(),
    };
    Ok((created.key(), kept))
}

/// the record the library called `name` takes over without making one: the
/// one from when the node had one library, if `name` is that library's
/// heir, no record is kept for it yet, and no library has taken that one
/// over already.
fn inherited(
    all: &BTreeMap<String, Kept>,
    name: &str,
    heir: Option<&str>,
    legacy: Option<Kept>,
) -> Option<Kept> {
    let legacy = legacy.filter(|legacy| !all.values().any(|kept| kept == legacy));
    (heir == Some(name) && !all.contains_key(name))
        .then_some(legacy)
        .flatten()
}

async fn load_legacy(state: &dyn StateStore) -> Option<Kept> {
    serde_json::from_slice(&state.get(LEGACY_NAME).await?).ok()
}

/// have the library called `name` take over the record from when the node
/// had one library now, if it is that library's `heir` and has not yet:
/// what a library renamed before its record was first opened would
/// otherwise never do. one caller at a time.
pub async fn inherit_kept(state: &dyn StateStore, name: &str, heir: Option<&str>) -> Result<()> {
    let mut all = load_kept(state).await;
    let Some(kept) = inherited(&all, name, heir, load_legacy(state).await) else {
        return Ok(());
    };
    all.insert(name.to_owned(), kept);
    save_kept(state, &all).await
}

/// the record of the library called `name`, open for writing: the one kept,
/// or for the `heir` of a node's one library that library's, or a new one,
/// kept from now on. one caller at a time: the records are kept together.
pub async fn open_library_record(
    context: &RoutingContext,
    state: &dyn StateStore,
    name: &str,
    heir: Option<&str>,
) -> Result<RecordKey> {
    let mut all = load_kept(state).await;
    if let Some(kept) = all.get(name) {
        return reopen(context, kept).await;
    }
    let (key, kept) = match inherited(&all, name, heir, load_legacy(state).await) {
        Some(kept) => (reopen(context, &kept).await?, kept),
        None => create(context).await?,
    };
    all.insert(name.to_owned(), kept);
    save_kept(state, &all).await?;
    Ok(key)
}

/// what becomes of the kept records when a library is renamed, which keeps
/// its record and so its link, or with no new name, is given up.
pub fn moved(
    mut all: BTreeMap<String, Kept>,
    from: &str,
    to: Option<&str>,
) -> BTreeMap<String, Kept> {
    if let (Some(kept), Some(to)) = (all.remove(from), to) {
        all.insert(to.to_owned(), kept);
    }
    all
}

/// keep the record of the library called `from` under `to` from now on, or
/// forget it. one caller at a time.
pub async fn move_kept(state: &dyn StateStore, from: &str, to: Option<&str>) -> Result<()> {
    let all = load_kept(state).await;
    save_kept(state, &moved(all, from, to)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::MemoryState;
    use futures_util::FutureExt;

    fn kept(n: u8) -> Kept {
        Kept {
            key: format!("VLD0:key{n}:secret"),
            owner: format!("VLD0:public{n}:secret"),
        }
    }

    #[test]
    fn kept_records_are_read_back_and_bad_ones_are_as_none() {
        let state = MemoryState::default();
        let load = || load_kept(&state).now_or_never().unwrap();
        assert!(load().is_empty());
        let all = BTreeMap::from([("music".to_owned(), kept(1))]);
        save_kept(&state, &all).now_or_never().unwrap().unwrap();
        assert_eq!(load(), all);
        let bad = state.put(RECORDS_NAME, b"not json".to_vec());
        bad.now_or_never().unwrap().unwrap();
        assert!(load().is_empty());
    }

    #[test]
    fn a_renamed_library_keeps_its_record_and_a_removed_one_loses_it() {
        let state = MemoryState::default();
        let load = || load_kept(&state).now_or_never().unwrap();
        let all = BTreeMap::from([("music".to_owned(), kept(1)), ("docs".to_owned(), kept(2))]);
        save_kept(&state, &all).now_or_never().unwrap().unwrap();
        let renaming = move_kept(&state, "music", Some("tunes"));
        renaming.now_or_never().unwrap().unwrap();
        assert_eq!(load().get("tunes"), Some(&kept(1)));
        assert_eq!(load().get("music"), None);
        let removing = move_kept(&state, "docs", None);
        removing.now_or_never().unwrap().unwrap();
        assert_eq!(load().into_keys().collect::<Vec<_>>(), ["tunes"]);
        // one that was never kept is neither moved nor missed.
        assert_eq!(moved(load(), "nope", Some("x")), load());
    }

    #[test]
    fn the_library_a_nodes_one_library_became_takes_over_its_record() {
        let none = BTreeMap::new();
        let legacy = || Some(kept(9));
        assert_eq!(
            inherited(&none, "the shelf", Some("the shelf"), legacy()),
            legacy()
        );
        // no other library does, and nor does the heir once it has a record
        // of its own, or where there is no record from before.
        assert_eq!(
            inherited(&none, "downloads", Some("the shelf"), legacy()),
            None
        );
        assert_eq!(inherited(&none, "the shelf", None, legacy()), None);
        assert_eq!(inherited(&none, "the shelf", Some("the shelf"), None), None);
        let held = BTreeMap::from([("the shelf".to_owned(), kept(1))]);
        assert_eq!(
            inherited(&held, "the shelf", Some("the shelf"), legacy()),
            None
        );
        // nor a later library of the heir's old name, once the heir has
        // taken the record with it to another name.
        let taken = BTreeMap::from([("renamed".to_owned(), kept(9))]);
        assert_eq!(
            inherited(&taken, "the shelf", Some("the shelf"), legacy()),
            None
        );
    }

    #[test]
    fn an_heir_renamed_before_its_record_was_opened_still_takes_it_over() {
        let state = MemoryState::default();
        let legacy = serde_json::to_vec(&kept(9)).unwrap();
        let put = state.put(LEGACY_NAME, legacy);
        put.now_or_never().unwrap().unwrap();
        let heir = Some("the shelf");
        let inheriting = inherit_kept(&state, "the shelf", heir);
        inheriting.now_or_never().unwrap().unwrap();
        let renaming = move_kept(&state, "the shelf", Some("tunes"));
        renaming.now_or_never().unwrap().unwrap();
        let all = load_kept(&state).now_or_never().unwrap();
        assert_eq!(all, BTreeMap::from([("tunes".to_owned(), kept(9))]));
        // no other library is given it, asked or not.
        let other = inherit_kept(&state, "downloads", heir);
        other.now_or_never().unwrap().unwrap();
        assert_eq!(load_kept(&state).now_or_never().unwrap(), all);
    }
}
