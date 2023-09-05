#![allow(dead_code)]
pub mod tracker {
    use core::fmt;
    use tracing::{event, Level};
    use std::collections::HashMap;

    use ethers::{core::types::TxHash, types::{U64, H64, Bloom, H256, ValueOrArray, Address, Topic, Log}};

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

    pub struct ChainCacheOptions {
        pub max_block_cached: u32,

        pub retries_count: u32,

        pub retry_delay_ms: u32,

        pub batch_size: u32,
    }

    pub struct BlockCache<Provider: BlockProvider<TxHash>> {
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

    pub trait BlockProvider<TX> {
        fn get_block(&self, number: BlockIdentifier) -> Result<Block<TX>, String>;

        fn get_block_batch(&self, from: BlockIdentifier, to: BlockIdentifier) -> Result<Vec<Block<TX>>, String>;

        fn get_logs(&self, filter: Filter) -> Result<Vec<Log>, String>;
    }

    pub struct Filter {
        pub block_option: FilterBlockOption,

        pub address: Option<ValueOrArray<Address>>,

        pub topics: [Option<Topic>; 4],
    }

    impl<Provider: BlockProvider<TxHash>> BlockCache<Provider> {
        fn add_block(&mut self, block: Block<TxHash>) {
            event!(Level::INFO, "add_block {}", block);
            self.last_block = Some(block);
        }

        fn initialize(&mut self, block: Block<TxHash>) -> Option<String> {
            let rpc_block_result = self.rpc_provider.get_block(BlockIdentifier::Hash(block.hash));

            match rpc_block_result {
                Err(error) => {
                    Some(error)
                }
                Ok(block) => {
                    self.add_block(block);
                    None
                },
            }
        }

        fn find_common_ancestor(self) -> Result<Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self.last_block.unwrap();
            self.rpc_provider.get_block(BlockIdentifier::Number(last_block.number - 1))
        }

        fn handle_block(&mut self, block: Block<TxHash>) {
            let last_block = self.last_block.as_ref().unwrap();
            if last_block.hash != block.parent_hash {
                event!(Level::WARN, "handle_block reorg detected last_block: {}, newblock: {}", last_block, block);
            }

        }
    }
}
