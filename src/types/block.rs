use core::fmt;

use ethers::{core::types::{H256, TxHash}, types::{U64, Bloom, H64}};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Block<TX> {

    pub hash: H256,

    pub parent_hash: H256,

    pub number: U64,

    pub transactions: Vec<TX>,

    pub nonce: H64, 

    pub logs_bloom: Bloom,
}

impl fmt::Display for Block<TxHash> {

    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(hash: {}, parent_hash: {}, number: {}, transactions_length: {}, nonce: {}, logs_bloom: {})", self.hash, self.parent_hash, self.number, self.transactions.len(), self.nonce, self.logs_bloom)
    }
}
