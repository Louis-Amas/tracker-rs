use crate::types::block::Block;
use ethers::core::types::TxHash;

pub struct ChainCache {
   
    pub last_block: Block<TxHash>,
}
