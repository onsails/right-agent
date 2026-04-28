use std::fmt;

#[derive(Debug)]
pub struct BlockAlreadyRendered;

impl fmt::Display for BlockAlreadyRendered {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result { Ok(()) }
}

impl std::error::Error for BlockAlreadyRendered {}
