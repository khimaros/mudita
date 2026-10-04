//! the mudita core: the protocol of DESIGN.md and everything that speaks it,
//! with no runtime, filesystem or process of its own, so that the command
//! line and a browser run the same code. whoever embeds it supplies the
//! files, the state that is kept between runs, and the veilid node's config
//! (see `store`), and drives it through an `Engine` (see `serve`).

pub mod chunked;
pub mod control;
pub mod download_log;
pub mod downloads;
pub mod fetch;
pub mod library;
pub mod library_record;
pub mod members;
pub mod merkle;
pub mod neighbors;
pub mod node;
pub mod owned;
pub mod peer;
pub mod protocol;
pub mod rt;
pub mod seed;
pub mod serve;
pub mod shares;
pub mod store;
pub mod units;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
pub fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../../testdata/protocol_vectors.json")).unwrap()
}

#[cfg(test)]
const FNV_OFFSET: u32 = 2166136261;
#[cfg(test)]
const FNV_PRIME: u32 = 16777619;

/// the toy hash the shared vectors are built with (see their toyHash field).
#[cfg(test)]
pub fn toy_hash(data: &[u8]) -> protocol::Hash {
    let word = |seed: u32| {
        data.iter().fold(FNV_OFFSET ^ seed, |h, byte| {
            (h ^ u32::from(*byte)).wrapping_mul(FNV_PRIME)
        })
    };
    let words: Vec<u8> = (0..8).flat_map(|seed| word(seed).to_be_bytes()).collect();
    words.try_into().unwrap()
}
