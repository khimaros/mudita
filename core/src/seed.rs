//! sharing whole files: hash one into its manifest and merkle tree, join its
//! rendezvous record, and hand it to a peer to serve. shares.rs does this
//! for files shared one at a time; library.rs does it for every file in a
//! library's folders.

use crate::merkle::{blake3_hash, build_levels, leaf_hash, proof_for, root_of};
use crate::node::{owner_keypair, record_key, record_schema};
use crate::peer::{Peer, Pieces, Proven, Share};
use crate::protocol::{
    decode_manifest, encode_manifest, hash_from_hex, piece_count, piece_range, share_keys, to_hex,
    Hash, Manifest, ShareKeys, MANIFEST_SUBKEY, PIECE_BYTES, PROTOCOL_VERSION,
};
use crate::rt::retry;
use crate::store::{BoxFuture, Source};
use crate::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use veilid_core::{RecordKey, RoutingContext, VeilidAPI, CRYPTO_KIND_VLD0};

// a node fresh on the network often fails to build a working route for a
// minute or two, so the first slot is tried for about that long before
// giving up.
pub const JOIN_ATTEMPTS: u32 = 30;

/// a whole file and its merkle tree.
struct SourcePieces {
    source: Arc<dyn Source>,
    size: u64,
    levels: Vec<Vec<Hash>>,
}

impl Pieces for SourcePieces {
    fn count(&self) -> u64 {
        self.levels[0].len() as u64
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn piece(&self, index: u64) -> BoxFuture<'_, Result<Option<Proven>>> {
        Box::pin(async move {
            let (start, end) = piece_range(index, self.size);
            let piece = self.source.read(start, end).await?;
            Ok(Some((piece, proof_for(&self.levels, index as usize))))
        })
    }
}

/// read the file once, front to back, for the leaf hash of every piece.
/// `hashed` counts the pieces done, for whoever shows progress.
pub async fn hash_leaves(source: &dyn Source, hashed: &AtomicU64) -> Result<Vec<Hash>> {
    let size = source.size();
    let mut leaves = Vec::new();
    for index in 0..piece_count(size) {
        let (start, end) = piece_range(index, size);
        let piece = source.read(start, end).await?;
        if piece.len() as u64 != end - start {
            return Err("the file is shorter than it was".into());
        }
        leaves.push(leaf_hash(blake3_hash, &piece));
        hashed.fetch_add(1, Ordering::Relaxed);
    }
    Ok(leaves)
}

/// open the share's record as a writer, creating it if nobody has shared the
/// file before.
pub async fn open_share(
    api: &VeilidAPI,
    context: &RoutingContext,
    keys: &ShareKeys,
) -> Result<RecordKey> {
    let key = record_key(api, keys).await?;
    let owner = owner_keypair(keys);
    let opened = context.open_dht_record(key.clone(), Some(owner.clone()));
    if opened.await.is_ok() {
        return Ok(key);
    }
    // veilid creates a record under a random encryption key, so it is closed
    // and reopened under the derived one.
    let created = context
        .create_dht_record(CRYPTO_KIND_VLD0, record_schema()?, Some(owner.clone()))
        .await?;
    context.close_dht_record(created.key()).await?;
    let reopened = context.open_dht_record(key.clone(), Some(owner));
    reopened.await.map(|_| key).map_err(Into::into)
}

/// write the manifest unless the record already describes this file.
async fn publish_manifest(
    context: &RoutingContext,
    key: &RecordKey,
    manifest: &Manifest,
) -> Result<()> {
    let held = context
        .get_dht_value(key.clone(), MANIFEST_SUBKEY, true)
        .await?;
    let held = held.and_then(|value| decode_manifest(value.data()).ok());
    if held.as_ref() != Some(manifest) {
        context
            .set_dht_value(
                key.clone(),
                MANIFEST_SUBKEY,
                encode_manifest(manifest)?,
                None,
            )
            .await?;
    }
    Ok(())
}

/// the manifest of a file of `size` bytes whose leaf hashes are `leaves`.
pub fn manifest_of(size: u64, leaves: &[Hash]) -> Result<Manifest> {
    if piece_count(size) > u64::from(u32::MAX) {
        return Err("file is too large".into());
    }
    Ok(Manifest {
        version: PROTOCOL_VERSION,
        size,
        piece_bytes: PIECE_BYTES,
        root: to_hex(&root_of(&build_levels(blake3_hash, leaves.to_vec()))),
    })
}

/// join the record of the file `manifest` describes and serve it from
/// `source` over `peer`, under the source's name, trying `attempts` times
/// to claim a slot.
pub async fn share_file(
    api: &VeilidAPI,
    context: &RoutingContext,
    peer: &Peer,
    source: Arc<dyn Source>,
    manifest: &Manifest,
    leaves: Vec<Hash>,
    attempts: u32,
) -> Result<Arc<Share>> {
    let root = hash_from_hex(&manifest.root)?;
    let keys = share_keys(blake3_hash, &root, manifest.size);
    let key = open_share(api, context, &keys).await?;
    retry(|| publish_manifest(context, &key, manifest)).await?;
    let name = source.name();
    let pieces = Arc::new(SourcePieces {
        source,
        size: manifest.size,
        levels: build_levels(blake3_hash, leaves),
    });
    peer.add(key, root, &name, pieces, true, attempts).await
}
