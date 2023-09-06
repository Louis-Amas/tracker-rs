#![allow(dead_code)]

mod utils;

pub mod tracker {
    use core::fmt;
    use tracing::{event, Level};
    use std::collections::HashMap;

    use ethers::{core::types::TxHash, types::{U64, H64, Bloom, H256, ValueOrArray, Address, Topic, Log}};

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct MinimalBlock {
        pub hash: H256,

        pub parent_hash: H256,

        pub number: U64,
    }

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

    pub struct Filter {
        pub address: Option<ValueOrArray<Address>>,

        pub topics: Option<[Option<Topic>; 4]>,
    }

    pub struct ChainCacheOptions {
        pub max_block_cached: u32,

        pub retries_count: u32,

        pub retry_delay_ms: u32,

        pub batch_size: u32,

        pub block_back_count: u32,

        filter: Filter,
    }

    pub struct BlockCache<'a, Provider: BlockProvider<TxHash>> {
        pub rpc_provider: Provider,

        pub options: ChainCacheOptions,

        pub last_block: Option<&'a Block<TxHash>>,

        pub blocks_map: HashMap<u64, &'a Block<TxHash>>,
    }

    pub enum BlockIdentifier {
        Latest,
        Number(u64),
        Hash(H256),
    }

    pub enum FilterBlockOption {
        Range {
            from_identifier: BlockIdentifier,
            to_identifier: BlockIdentifier,
        },
        AtBlock(BlockIdentifier),
    }

    pub trait BlockProvider<TX> {
        fn get_block(&self, number: BlockIdentifier) -> Result<Block<TX>, String>;

        fn get_minimal_block_batch(&self, from: BlockIdentifier, to: BlockIdentifier) -> Result<Vec<MinimalBlock>, String>;

        fn get_logs(&self, filter_block: FilterBlockOption, filter: &Filter) -> Result<Vec<Log>, String>;
    }

    impl<'a, Provider: BlockProvider<TxHash>> BlockCache<'a, Provider> {
        fn add_block(&mut self, block: &'a Block<TxHash>) {
            event!(Level::INFO, "add_block {}", block);
            self.last_block = Some(block);
            self.blocks_map.insert(block.number.as_u64(), block);
        }

        fn initialize(&mut self, block: &'a Block<TxHash>) -> Option<String> {
            let rpc_block_result = self.rpc_provider.get_block(BlockIdentifier::Hash(block.hash));

            match rpc_block_result {
                Err(error) => {
                    Some(error)
                }
                Ok(_block) => {
                    self.add_block(&block);
                    None
                },
            }
        }

        fn find_common_ancestor(&self) -> Result<&'a Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self.last_block.ok_or("No last block available".to_string())?;
            let min_block_back = std::cmp::min(self.blocks_map.len() as u32, self.options.block_back_count);
            let from = last_block.number - min_block_back;
            let batch = self.rpc_provider.get_minimal_block_batch(BlockIdentifier::Number(from.as_u64()), BlockIdentifier::Number(last_block.number.as_u64() - 1))?;

            for block in batch.iter().rev() {
                let cached_block = self.blocks_map.get(&block.number.as_u64()).ok_or("Missing cached block".to_string())?;
                if cached_block.hash == block.hash {
                    return Ok(cached_block);
                }
            }

            Err("Did not find common ancestor".to_string())
        }

        fn handle_batch_block(&mut self, from: &Block<TxHash>, to: &Block<TxHash>, validate_block_exist: bool) -> Result<Vec<Log>, String> {
            let mut from_number = from.number.as_u64();
            let mut to_number = std::cmp::min(from_number + self.options.batch_size as u64, to.number.as_u64());

            let mut aggregated_logs: Vec<Log> = Vec::new();
            while from_number == to.number.as_u64() {
                let mut logs = self.rpc_provider
                    .get_logs(
                        FilterBlockOption::Range { 
                            from_identifier: BlockIdentifier::Number(from_number), 
                            to_identifier: BlockIdentifier::Number(to_number) }, &self.options.filter)?;

                if validate_block_exist {
                    if let Some(log) = logs.last() {
                        self.rpc_provider.get_block(BlockIdentifier::Hash(log.block_hash.unwrap()))
                            .map_err(|_err| "Block hash received in log does not exist")?;
                    }
                }

                aggregated_logs.append(&mut logs);
 
                from_number = to.number.as_u64();
                to_number = std::cmp::min(from_number + self.options.batch_size as u64, to.number.as_u64()) ;
            }

            Ok(aggregated_logs)
        }


        fn handle_block(&mut self, block: &'a Block<TxHash>) -> Result<(&Block<TxHash>, Option<&Block<TxHash>>, Vec<Log>), String> {
            let last_block = self.last_block.ok_or("Missing last block")?;
            let mut rollback_block: Option<&Block<TxHash>> = None;
            if last_block.hash != block.parent_hash {
                event!(Level::WARN, "handle_block reorg detected last_block: {}, newblock: {}", last_block, block);

                let common_ancestor = self.find_common_ancestor()?;

                /* clean cache from reorged blocks */
                for bn in(common_ancestor.number.as_u64() + 1)..last_block.number.as_u64() {
                    self.blocks_map.remove(&bn);
                }

                self.last_block = Some(&common_ancestor);
                rollback_block = Some(common_ancestor);

                event!(Level::DEBUG, "common ancestor found {}", common_ancestor);
            }

            let logs = self.handle_batch_block(last_block, &block, true)?;

            Ok((&block, rollback_block, logs))
        }
    }
}
