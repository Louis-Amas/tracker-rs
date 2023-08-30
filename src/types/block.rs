use ethers::{core::types::H256, types::{U64, Bloom, H64}};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Block<TX> {

    pub hash: H256,

    pub parent_hash: H256,

    pub number: U64,

    pub transactions: Vec<TX>,

    pub nonce: H64, 

    pub logs_bloom: Bloom,
}
