//! merkle tree over file pieces (see DESIGN.md). a tree is stored as levels:
//! levels[0] holds the leaves and the last level holds only the root. an odd
//! node at the end of a level is promoted unchanged to the next level.

use crate::protocol::Hash;
use std::collections::HashMap;

pub type Hasher = fn(&[u8]) -> Hash;

// distinct prefixes keep an interior node from being passed off as a leaf.
const LEAF_PREFIX: u8 = 0;
const NODE_PREFIX: u8 = 1;

/// the veilid vld0 hash.
pub fn blake3_hash(data: &[u8]) -> Hash {
    *blake3::hash(data).as_bytes()
}

pub fn leaf_hash(hash: Hasher, piece: &[u8]) -> Hash {
    hash(&[&[LEAF_PREFIX][..], piece].concat())
}

fn node_hash(hash: Hasher, left: &Hash, right: &Hash) -> Hash {
    hash(&[&[NODE_PREFIX][..], left, right].concat())
}

pub fn build_levels(hash: Hasher, leaves: Vec<Hash>) -> Vec<Vec<Hash>> {
    let mut levels = vec![leaves];
    while levels[levels.len() - 1].len() > 1 {
        let next = levels[levels.len() - 1]
            .chunks(2)
            .map(|pair| match pair {
                [left, right] => node_hash(hash, left, right),
                _ => pair[0],
            })
            .collect();
        levels.push(next);
    }
    levels
}

pub fn root_of(levels: &[Vec<Hash>]) -> Hash {
    levels[levels.len() - 1][0]
}

/// sibling hashes from leaf to root, omitting levels where the node is promoted.
pub fn proof_for(levels: &[Vec<Hash>], mut index: usize) -> Vec<Hash> {
    let mut proof = Vec::new();
    for level in &levels[..levels.len() - 1] {
        if let Some(sibling) = level.get(index ^ 1) {
            proof.push(*sibling);
        }
        index >>= 1;
    }
    proof
}

// where the proof hashes of a piece sit: (level, index of the sibling), leaf
// level first. the same walk verify_proof makes.
fn proof_path(mut index: u64, leaf_count: u64) -> Vec<(usize, u64)> {
    let mut path = Vec::new();
    let (mut level, mut width) = (0, leaf_count);
    while width > 1 {
        if (index ^ 1) < width {
            path.push((level, index ^ 1));
        }
        index >>= 1;
        width = width.div_ceil(2);
        level += 1;
    }
    path
}

/// the tree as a downloader comes to know it: the proof hashes that arrived
/// with its pieces, which are the hashes it needs to prove those pieces to
/// the next downloader. pieces that share a parent share its hash, so this
/// holds at most two hashes per piece.
#[derive(Default)]
pub struct ProofStore {
    siblings: HashMap<(usize, u64), Hash>,
}

impl ProofStore {
    /// keep the verified proof of a piece.
    pub fn add(&mut self, index: u64, leaf_count: u64, proof: &[Hash]) {
        let path = proof_path(index, leaf_count);
        self.siblings
            .extend(path.into_iter().zip(proof.iter().copied()));
    }

    /// the proof of a piece, if it was added.
    pub fn proof(&self, index: u64, leaf_count: u64) -> Option<Vec<Hash>> {
        let path = proof_path(index, leaf_count);
        path.iter()
            .map(|at| self.siblings.get(at).copied())
            .collect()
    }
}

/// check that a leaf hash sits at index in a tree of leaf_count leaves.
pub fn verify_proof(
    hash: Hasher,
    root: &Hash,
    leaf: Hash,
    mut index: u64,
    leaf_count: u64,
    proof: &[Hash],
) -> bool {
    let mut node = leaf;
    let mut siblings = proof.iter();
    let mut width = leaf_count;
    while width > 1 {
        if (index ^ 1) < width {
            let Some(sibling) = siblings.next() else {
                return false;
            };
            node = if index & 1 == 1 {
                node_hash(hash, sibling, &node)
            } else {
                node_hash(hash, &node, sibling)
            };
        }
        index >>= 1;
        width = width.div_ceil(2);
    }
    siblings.next().is_none() && node == *root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{hash_from_hex, to_hex, HASH_BYTES};
    use crate::{toy_hash, vectors};

    #[test]
    fn toy_hash_matches_vectors() {
        assert_eq!(to_hex(&toy_hash(b"abc")), vectors()["toyHashOfAbc"]);
    }

    fn hex(value: &serde_json::Value) -> Vec<u8> {
        let text = value.as_str().unwrap();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn trees_match_vectors() {
        for case in vectors()["merkle"].as_array().unwrap() {
            let pieces: Vec<Vec<u8>> = case["pieces"].as_array().unwrap().iter().map(hex).collect();
            let leaves: Vec<Hash> = pieces.iter().map(|p| leaf_hash(toy_hash, p)).collect();
            let levels = build_levels(toy_hash, leaves.clone());
            let root = root_of(&levels);
            assert_eq!(to_hex(&root), case["root"]);
            for (index, leaf) in leaves.iter().enumerate() {
                let proof = proof_for(&levels, index);
                let expected: Vec<Hash> = case["proofs"][index]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| hash_from_hex(h.as_str().unwrap()).unwrap())
                    .collect();
                assert_eq!(proof, expected);
                let count = leaves.len() as u64;
                assert!(verify_proof(
                    toy_hash,
                    &root,
                    *leaf,
                    index as u64,
                    count,
                    &proof
                ));
            }
        }
    }

    #[test]
    fn a_wrong_piece_index_or_proof_is_rejected() {
        let leaves: Vec<Hash> = (0..11u8).map(|n| leaf_hash(blake3_hash, &[n, 7])).collect();
        let levels = build_levels(blake3_hash, leaves.clone());
        let root = root_of(&levels);
        let proof = proof_for(&levels, 4);
        let verify =
            |leaf, index, proof: &[Hash]| verify_proof(blake3_hash, &root, leaf, index, 11, proof);
        assert!(verify(leaves[4], 4, &proof));
        assert!(!verify(leaf_hash(blake3_hash, &[99]), 4, &proof));
        assert!(!verify(leaves[4], 5, &proof));
        assert!(!verify(leaves[4], 4, &proof[1..]));
        assert!(!verify(
            leaves[4],
            4,
            &[&proof[..], &[[0; HASH_BYTES]]].concat()
        ));
    }

    #[test]
    fn a_proof_store_gives_back_the_proofs_it_was_given() {
        for count in 1..=17u64 {
            let leaves: Vec<Hash> = (0..count)
                .map(|n| leaf_hash(blake3_hash, &[n as u8]))
                .collect();
            let levels = build_levels(blake3_hash, leaves);
            let mut store = ProofStore::default();
            assert_eq!(store.proof(0, count).is_some(), count == 1);
            // out of order, as pieces arrive.
            for index in (0..count).rev() {
                store.add(index, count, &proof_for(&levels, index as usize));
            }
            for index in 0..count {
                let proof = store.proof(index, count).unwrap();
                assert_eq!(proof, proof_for(&levels, index as usize));
            }
        }
    }

    #[test]
    fn an_interior_node_cannot_stand_in_for_a_leaf() {
        let leaves = vec![leaf_hash(blake3_hash, b"a"), leaf_hash(blake3_hash, b"b")];
        let levels = build_levels(blake3_hash, leaves.clone());
        let piece = [leaves[0], leaves[1]].concat();
        assert_ne!(leaf_hash(blake3_hash, &piece), root_of(&levels));
    }
}
