#![allow(dead_code)]
use tracing::{event, Level};
use std::collections::HashMap;

use crate::types::{block::Block, self};
use ethers::{core::types::TxHash, types::{U64, H256, ValueOrArray, Address, Topic, Log}};

pub struct ChainCacheOptions {
    pub max_block_cached: u32,

    pub retries_count: u32,

    pub retry_delay_ms: u32,

    pub batch_size: u32,
}

pub struct BlockCache<Provider: RpcProvider<TxHash>> {
    pub rpc_provider: Provider,

    pub options: ChainCacheOptions,

    pub last_block: Option<Block<TxHash>>,

    pub blocks_map: HashMap<U64, Block<TxHash>>,
}

pub enum BlockIdentifier {
    Latest,
    Number(U64),
    Hash(H256),
}

pub enum FilterBlockOption {
    Range {
        from_block: BlockIdentifier,
        to_block: BlockIdentifier,
    },
    AtBlock(BlockIdentifier),
}

pub trait RpcProvider<TX> {
    fn get_block(&self, number: BlockIdentifier) -> Result<types::block::Block<TX>, String>;

    fn get_logs(&self, filter: Filter) -> Result<Vec<Log>, String>;
}

pub struct Filter {
    pub block_option: FilterBlockOption,

    pub address: Option<ValueOrArray<Address>>,

    pub topics: [Option<Topic>; 4],
}

impl<Provider: RpcProvider<TxHash>> BlockCache<Provider> {
    fn add_block(&mut self, block: Block<TxHash>) {
        event!(Level::INFO, "add_block {}", block);
        self.last_block = Some(block);
    }

    fn initialize(&mut self, block: Block<TxHash>) {

        self.add_block(block);
    }

    fn handle_block(&mut self, _block: Block<TxHash>) {

    }
}
