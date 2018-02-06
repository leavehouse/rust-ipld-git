use cid::Cid;

pub trait Node {
    // TODO: return an `impl Iterator<Item = Link<'a>>` instead?
    fn links<'a>(&'a self) -> Vec<Link<'a>>;
}

#[derive(Debug)]
pub struct Link<'a> {
    pub cid: &'a Cid,
}

impl<'a> Link<'a> {
    pub fn new(cid: &'a Cid) -> Link<'a> {
        Link { cid: cid }
    }
}
