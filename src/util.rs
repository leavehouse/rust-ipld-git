use cid::{self, Cid};
use multihash;

use Error;

// Split `s` into two slices at the first slice element to equal `val`,
// removing the matched element
// TODO: generalize to take a predicate function?
pub fn cleave_out_at_value(s: &[u8], val: u8) -> Option<(&[u8], &[u8])> {
    let i = match s.iter().enumerate().find(|&(_, &el)| el == val) {
        Some((i, _)) => i,
        None => return None,
    };

    Some((&s[..i], &s[i+1..]))
}

pub fn sha1_to_cid(sha1: &[u8]) -> Result<Cid, Error> {
    // TODO: this constructor is a little ugly because you have to
    // manually specify "20" for the length. could use multihash::Hash::SHA1.size(),
    // but that returns a u8 and mh_len is expecting a usize, so youd have to cast
    // i.e. `multihash::Hash::SHA1.size() as usize` instead of `20`, which is uglier
    // still. Open issue with rust-cid suggesting that the version, the codec, and the
    // hash type should be all thats needed for constructing a Cid from a hash digest
    if sha1.len() != 20 {
        return Err("Cannot convert byte slice to Cid: must be a 20-byte SHA-1 digest".to_string())
    }
    let cid_prefix = cid::Prefix {
        version: cid::Version::V1,
        codec: cid::Codec::GitRaw,
        mh_type: multihash::Hash::SHA1,
        mh_len: 20,
    };
    Ok(Cid::new_from_prefix(&cid_prefix, &sha1))
}
