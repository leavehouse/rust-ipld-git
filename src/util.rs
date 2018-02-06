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

pub fn sha1_to_cid(digest: &[u8]) -> Result<Cid, Error> {
    // TODO: this constructor is a little ugly because you have to
    // manually specify "20" for the length. could use multihash::Hash::SHA1.size(),
    // but that returns a u8 and mh_len is expecting a usize, so youd have to cast
    // i.e. `multihash::Hash::SHA1.size() as usize` instead of `20`, which is uglier
    // still. Open issue with rust-cid suggesting that the version, the codec, and the
    // hash type should be all thats needed for constructing a Cid from a hash digest
    if digest.len() != 20 {
        return Err(format!("Cannot convert byte slice to Cid: SHA-1 digests \
                            are 20-bytes, this is {} bytes", digest.len()))
    }

    let hash_alg = multihash::Hash::SHA1;
    let mut mh = Vec::with_capacity(digest.len() + 2);
    mh.push(hash_alg.code());
    mh.push(hash_alg.size());
    mh.extend_from_slice(digest);

    Ok(Cid::new(cid::Codec::GitRaw, cid::Version::V1, &mh))
}

#[cfg(test)]
mod test {
    use sha1_to_cid;

    #[test]
    fn test_sha1_to_cid() {
        use cid;
        use hex;
        use multihash;

        // SHA-1 of "test" (`echo -n "test" | sha1sum`)
        let test_sha1 = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
        let test_digest_bytes = hex::decode(test_sha1).unwrap();

        let cid = sha1_to_cid(&test_digest_bytes).unwrap();

        assert_eq!(cid.version, cid::Version::V1);
        assert_eq!(cid.codec, cid::Codec::GitRaw);

        let mh = multihash::decode(&cid.hash).unwrap();
        assert_eq!(mh.alg, multihash::Hash::SHA1);
        assert_eq!(mh.digest, &test_digest_bytes[..]);
    }
}
