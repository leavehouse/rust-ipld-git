extern crate cid;
extern crate multihash;

use cid::Cid;
use std::collections::HashMap;
use std::str;

pub use node::Node;

mod node;

pub struct Blob(Vec<u8>);

impl Node for Blob {
    fn links(&self) -> Vec<node::Link> {
        Vec::new()
    }
}

pub struct Tree {
    entries: HashMap<String, TreeEntry>,
    order: Vec<String>,
}

impl Tree {
    fn new() -> Tree {
        Tree { entries: HashMap::new(), order: Vec::new() }
    }

    fn add_entry(&mut self, entry: TreeEntry) {
        let name = entry.name.clone();
        self.entries.insert(entry.name.clone(), entry);
        self.order.push(name);
    }
}

impl Node for Tree {
    fn links(&self) -> Vec<node::Link> {
        self.entries.iter()
                    .map(|(_, entry)| node::Link::new(&entry.cid))
                    .collect::<Vec<_>>()
    }
}

// TODO: change to use bytes crate for zero copy? constructing current struct
//       requires copying and will be slow for large repos, I believe
// TODO: file names can maybe have non-utf8 characters. figure out how git
//       handles them?
struct TreeEntry {
    pub mode: String,
    pub name: String,
    pub cid: Cid,
}

// TODO: encoding, gpgsig, mergetag, non-standard headers?
pub struct Commit {
    tree: Cid,
    parents: Vec<Cid>,
    author: UserInfo,
    committer: UserInfo,
}

impl Node for Commit {
    fn links(&self) -> Vec<node::Link> {
        let mut v = vec![node::Link::new(&self.tree)];
        let parent_links = self.parents.iter()
                    .map(|cid| node::Link::new(&cid));
        v.extend(parent_links);
        v
    }
}

struct UserInfo {
    pub name: String,
    pub email: String,
    pub timestamp: String,
    pub timezone: String,
}

type Error = String;

// Split `s` into two slices at the first slice element to equal `val`,
// removing the matched element
// TODO: generalize to take a predicate function?
fn cleave_out_at_value(s: &[u8], val: u8) -> Option<(&[u8], &[u8])> {
    let i = match s.iter().enumerate().find(|&(_, &el)| el == val) {
        Some((i, _)) => i,
        None => return None,
    };

    Some((&s[..i], &s[i+1..]))
}

// Parse the header of a serialized git object.
// A git object is of the form "<type> <size>\x00<object bytes>", with the
// header containing the type and size.
fn parse_object_header<'a>(
    buf: &'a [u8],
    expected_type: &[u8]
) -> Result<&'a [u8], Error> {
    let (header, bytes) = match cleave_out_at_value(buf, 0) {
        Some((h, b)) => (h, b),
        None => return Err("Invalid format for git object, missing null byte"
                       .to_string()),
    };

    let (type_, size) = match cleave_out_at_value(header, b' ') {
        Some((h, b)) => (h, b),
        None => return Err("Invalid format for git object header, must be
                            '<type> <size>'".to_string()),
    };

    if type_ != expected_type {
        return Err(format!("Expected type '{:?}', got: '{:?}'",
                           expected_type, type_))
    }

    let size = match str::from_utf8(size) {
        Err(e) => return Err(format!("Error converting the object size to a
                                      string: {}", e)),
        Ok(s) => match s.parse::<u64>() {
            Err(e) => return Err(format!("Error parsing object size as an
                                          integer: {}", e)),
            Ok(n) => n,
        },
    };

    if (bytes.len() as u64) != size {
        return Err(format!("Size mismatch: {} bytes specified, but actual size \
                            was {} bytes", size, bytes.len()))
    }

    Ok(bytes)
}

pub fn parse_blob_object(buf: &[u8]) -> Result<Blob, Error> {
    let bytes = parse_object_header(buf, b"blob")?;
    Ok(Blob(bytes.to_vec()))
}

// A tree object is one(?) or more entries that look like:
//
//     <file permissions> <file name>\x00<sha-1 hash of the tree or blob object>
//
// (A delimeter is not needed between end of one entry and start of next
// because hashes have a fixed length of 20 bytes.)
pub fn parse_tree_object(buf: &[u8]) -> Result<Tree, Error> {
    let mut buf = parse_object_header(buf, b"tree")?;
    let mut tree = Tree::new();
    while let (Some(entry), rest) = parse_tree_entry(buf)? {
        buf = rest;
        tree.add_entry(entry);
    }
    Ok(tree)
}

fn parse_tree_entry(buf: &[u8]) -> Result<(Option<TreeEntry>, &[u8]), Error> {
    if buf.len() == 0 {
        return Ok((None, buf))
    }

    let (mode_bytes, rest) = match cleave_out_at_value(buf, b' ') {
        None => return Err("Could not read mode of tree entry".to_string()),
        Some((m, r)) => (m, r),
    };
    let (name_bytes, rest) = match cleave_out_at_value(rest, 0) {
        None => return Err("Could not read name of tree entry".to_string()),
        Some((n, h)) => (n, h),
    };
    let (hash_bytes, rest) = (&rest[..20], &rest[20..]);

    let mode = match str::from_utf8(mode_bytes) {
        Err(e) => return Err(format!("Tree entry mode is invalid, contains non utf-8
                                      characters: {}", e)),
        Ok(s) => s.to_string(),
    };
    let name = match str::from_utf8(name_bytes) {
        Err(e) => return Err(format!("Tree entry name is invalid, contains non utf-8
                                      characters: {}", e)),
        Ok(s) => s.to_string(),
    };

    let entry = Some(TreeEntry {
        mode: mode,
        name: name,
        cid: sha1_to_cid(hash_bytes)?
    });

    Ok((entry, rest))
}

// Commit objects are structured:
//
//     <commit header>
//
//     <commit message>
//
// where a blank line separates the header and message, and where the header
// looks like, for example:
//
//     tree <tree hash>
//     parent <first parent hash>
//     parent <second parent hash>
//     author <author string>
//     committer <committer string>
//
pub fn parse_commit_object(buf: &[u8]) -> Result<Commit, Error> {
    let mut buf = parse_object_header(buf, b"commit")?;
    // parse the commit header, which is repeatedly parsing lines
    // until you see a blank line
    let mut tree_cid: Option<Cid> = None;
    let mut parents: Vec<Cid> = Vec::new();
    let mut author_info: Option<UserInfo> = None;
    let mut committer_info: Option<UserInfo> = None;
    loop {
        let (line, rest) = match cleave_out_at_value(buf, b'\n') {
            None => return Err("Unexpected end of bytes".to_string()),
            Some((l, r)) => (l, r),
        };
        buf = rest;

        if line.len() == 0 {
            break;
        }

        let (name, data) = match cleave_out_at_value(line, b' ') {
            None => return Err("Invalid commit header line, should be of the
                                form '<name> <data>'".to_string()),
            Some((l, r)) => (l, r),
        };

        match name {
            b"tree" => {
                if tree_cid.is_some() {
                    return Err("Invalid second tree entry found".to_string())
                }
                tree_cid = Some(sha1_to_cid(data)?);
            },
            b"parent" => {
                parents.push(sha1_to_cid(data)?);
            },
            b"author" => {
                if author_info.is_some() {
                    return Err("Invalid second author entry found".to_string())
                }
                author_info = Some(parse_user_info(data)?)
            },
            b"committer" => {
                if committer_info.is_some() {
                    return Err("Invalid second committer entry found".to_string())
                }
                committer_info = Some(parse_user_info(data)?)
            },
            _ => return Err(format!("Unrecognized commit header field name: {:?}", name)),
        }
    }

    fn missing_field_error(name: &str) -> String {
        format!("Missing header field '{}'", name)
    }

    // TODO: map each `Option<T>` to a `Result<T, Error>` and then use `?`
    // instead?
    if tree_cid.is_none() {
        return Err(missing_field_error("tree"))
    } else if author_info.is_none() {
        return Err(missing_field_error("author"))
    } else if committer_info.is_none() {
        return Err(missing_field_error("committer"))
    }

    Ok(Commit {
        tree: tree_cid.unwrap(),
        parents: parents,
        author: author_info.unwrap(),
        committer: committer_info.unwrap(),
    })
}

fn parse_user_info(buf: &[u8]) -> Result<UserInfo, Error> {
    let (mut name, buf) = match cleave_out_at_value(buf, b'<') {
        None => return Err("User info is missing an email enclosed in angle \
                            brackets".to_string()),
        Some((n, r)) => (n, r),
    };
    // get rid of the space on the end of name
    name = &name[..name.len()-1];

    let (email, mut buf) = match cleave_out_at_value(buf, b'>') {
        None => return Err("User info email is missing a closing angle bracket \
                            ('>')".to_string()),
        Some((n, r)) => (n, r),
    };

    // get rid of initial space
    buf = &buf[1..];

    let (timestamp, timezone) = match cleave_out_at_value(buf, b' ') {
        None => return Err("User info date must be of the form \
                            '<unix timestamp> <timezone offset>'".to_string()),
        Some((n, r)) => (n, r),
    };

    Ok(UserInfo {
        name: byteslice_to_string(name)?,
        email: byteslice_to_string(email)?,
        timestamp: byteslice_to_string(timestamp)?,
        timezone: byteslice_to_string(timezone)?,
    })
}

fn byteslice_to_string(s: &[u8]) -> Result<String, Error> {
    str::from_utf8(s).map(|s| s.to_string())
                     .map_err(|e| format!("{}", e))
}

fn sha1_to_cid(sha1: &[u8]) -> Result<Cid, Error> {
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

#[cfg(test)]
mod test {
}
