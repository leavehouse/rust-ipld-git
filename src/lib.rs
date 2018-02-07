extern crate cid;
extern crate hex;
extern crate multihash;

use cid::Cid;
use std::collections::HashMap;
use std::str;

pub use node::Node;
use util::{cleave_out_at_value, sha1_to_cid};

mod node;
pub mod util;

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

pub type Error = String;

pub fn parse_object(buf: &[u8]) -> Result<Box<Node>, Error> {
    let (bytes, obj_type) = parse_object_header(buf)?;
    match obj_type {
        ObjectType::Blob => Ok(Box::new(parse_blob_object(bytes)?)),
        ObjectType::Tree => Ok(Box::new(parse_tree_object(bytes)?)),
        ObjectType::Commit => Ok(Box::new(parse_commit_object(bytes)?)),
        ObjectType::Tag => unimplemented!(),
    }
}

enum ObjectType {
    Blob,
    Tree,
    Commit,
    Tag
}

// Parse the header of a serialized git object.
// A git object is of the form "<type> <size>\x00<object bytes>", with the
// header containing the type and size.
fn parse_object_header(buf: &[u8]) -> Result<(&[u8], ObjectType), Error> {
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

    let obj_type = match type_ {
        b"blob" => ObjectType::Blob,
        b"tree" => ObjectType::Tree,
        b"commit" => ObjectType::Commit,
        b"tag" => ObjectType::Tag,
        _ => return Err(format!("Invalid object type: expected one of \
                                 \"blob\", \"tree\", \"commit\" or \"tag\", \
                                 got: {:?}", type_))
    };

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

    Ok((bytes, obj_type))
}

fn parse_blob_object(bytes: &[u8]) -> Result<Blob, Error> {
    Ok(Blob(bytes.to_vec()))
}

// A tree object is one(?) or more entries that look like:
//
//     <file permissions> <file name>\x00<sha-1 hash of the tree or blob object>
//
// (A delimeter is not needed between end of one entry and start of next
// because hashes have a fixed length of 20 bytes.)
fn parse_tree_object(mut bytes: &[u8]) -> Result<Tree, Error> {
    let mut tree = Tree::new();
    while let (Some(entry), rest) = parse_tree_entry(bytes)? {
        bytes = rest;
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
fn parse_commit_object(mut bytes: &[u8]) -> Result<Commit, Error> {
    // parse the commit header, which is repeatedly parsing lines
    // until you see a blank line
    let mut tree_cid: Option<Cid> = None;
    let mut parents: Vec<Cid> = Vec::new();
    let mut author_info: Option<UserInfo> = None;
    let mut committer_info: Option<UserInfo> = None;
    loop {
        let (line, rest) = match cleave_out_at_value(bytes, b'\n') {
            None => return Err("Unexpected end of bytes".to_string()),
            Some((l, r)) => (l, r),
        };
        bytes = rest;

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
                // TODO: convert data from 40-byte ASCII string to 20-byte bytestring
                let digest = hex::decode(data)
                    .map_err(|_| "Tree hash is not valid hexadecimal")?;
                if tree_cid.is_some() {
                    return Err("Invalid second tree entry found".to_string())
                }
                tree_cid = Some(sha1_to_cid(&digest)?);
            },
            b"parent" => {
                let digest = hex::decode(data)
                    .map_err(|_| "Tree hash is not valid hexadecimal")?;
                parents.push(sha1_to_cid(&digest)?);
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
    str::from_utf8(s)
        .map(|s| s.to_string())
        .map_err(|e| format!("Error converting to utf-8 string: {}", e))
}

#[cfg(test)]
mod test {
    use hex;
    use multihash;
    use util::sha1_to_cid;

    // header: 'commit 182'
    const INIT_COMMIT: &'static [u8] = b"\
        tree 7cee6dfa7d13e124220d2c04923f0cb0347ba27c\n\
        author Moloch <pure_machinery@example.com> 1517911033 -0600\n\
        committer Jaden Doe <j.doe@example.com> 1517914295 +0100\n\
        \n\
        Initial commit.\n";

    #[test]
    fn parse_commit() {
        let commit = match super::parse_commit_object(INIT_COMMIT) {
            Err(e) => panic!("Parsing error: {}", e),
            Ok(c) => c,
        };

        println!("commit.tree.hash = {:?}", &commit.tree.hash);
        let commit_tree_multihash = multihash::decode(&commit.tree.hash).unwrap();

        let tree_hash_hex = "7cee6dfa7d13e124220d2c04923f0cb0347ba27c";
        let tree_hash = hex::decode(&tree_hash_hex).unwrap();

        assert_eq!(commit_tree_multihash.digest, &tree_hash[..]);

        assert!(commit.parents.len() == 0);

        assert_eq!(&commit.author.name, "Moloch");
        assert_eq!(&commit.author.email, "pure_machinery@example.com");
        assert_eq!(&commit.author.timestamp, "1517911033");
        assert_eq!(&commit.author.timezone, "-0600");

        assert_eq!(&commit.committer.name, "Jaden Doe");
        assert_eq!(&commit.committer.email, "j.doe@example.com");
        assert_eq!(&commit.committer.timestamp, "1517914295");
        assert_eq!(&commit.committer.timezone, "+0100");
    }
}
