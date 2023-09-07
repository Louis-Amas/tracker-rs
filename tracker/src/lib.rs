#![allow(dead_code)]

pub mod tracker {
    use async_trait::async_trait;
    use tokio::try_join;

    use core::fmt;
    use tracing::{event, Level};
    use std::{collections::HashMap, fmt::Debug};

    use ethers::{core::types::TxHash, types::{U64, H64, Bloom, H256, ValueOrArray, Address, Topic, Log}};

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct Block<TX> {
        pub hash: H256,

        pub parent_hash: H256,

        pub number: U64,

        pub transactions: Option<Vec<TX>>,

        pub nonce: Option<H64>, 

        pub logs_bloom: Option<Bloom>,
    }

    impl fmt::Display for Block<TxHash> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "(hash: {}, parent_hash: {}, number: {}, transactions_length: {:?}, nonce: {}, logs_bloom: {})", 
                self.hash, 
                self.parent_hash, 
                self.number, 
                self.transactions.as_ref()
                    .map(|tx| tx.len().to_string())
                    .unwrap_or_else(|| "No transactions".to_string()), 
                self.nonce.as_ref().map(|nonce| nonce.to_string()).unwrap_or("No nonce".to_string()), 
                self.logs_bloom.as_ref().map(|bloom| bloom.to_string()).unwrap_or("No bloom".to_string()))
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

    #[async_trait]
    pub trait BlockProvider<TX>: Debug + Send + Sync {
        async fn get_block(&self, number: BlockIdentifier) -> Result<&Block<TX>, String>;

        async fn get_minimal_block_batch<'a>(&self, from: BlockIdentifier, to: BlockIdentifier) -> Result<Vec<&'a Block<TX>>, String>;

        async fn get_logs<'a>(&self, filter_block: FilterBlockOption, filter: &Filter) -> Result<Vec<&'a Log>, String>;
    }

    impl<'a, Provider: BlockProvider<TxHash>> BlockCache<'a, Provider> {
        fn add_block(&mut self, block: &'a Block<TxHash>) {
            event!(Level::INFO, "add_block {}", block);
            self.last_block = Some(block);
            self.blocks_map.insert(block.number.as_u64(), block);
        }

        async fn initialize(&mut self, block: &'a Block<TxHash>) -> Result<&'a Block<TxHash>, String> {
            let rpc_block = self.rpc_provider.get_block(BlockIdentifier::Hash(block.hash)).await?;

            if rpc_block.hash != block.hash {
                return Err("Rpc block hash not equal to the one provided to initialize".to_string());
            }

            self.add_block(block);

            Ok(block)
        }

        async fn find_common_ancestor(&self) -> Result<&'a Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self.last_block.ok_or("No last block available".to_string())?;
            let min_block_back = std::cmp::min(self.blocks_map.len() as u32, self.options.block_back_count);
            let from = last_block.number - min_block_back;
            let batch = self.rpc_provider.get_minimal_block_batch(BlockIdentifier::Number(from.as_u64()), BlockIdentifier::Number(last_block.number.as_u64() - 1)).await?;

            for block in batch.iter().rev() {
                let cached_block = self.blocks_map.get(&block.number.as_u64()).ok_or("Missing cached block".to_string())?;
                if cached_block.hash == block.hash {
                    return Ok(cached_block);
                }
            }

            Err("Did not find common ancestor".to_string())
        }

        async fn handle_batch_block(&mut self, from: &Block<TxHash>, to: &Block<TxHash>, fill_cache: bool) -> Result<Vec<&Log>, String> {
            let mut from_number = from.number.as_u64();
            let mut to_number = std::cmp::min(from_number + self.options.batch_size as u64, to.number.as_u64());
            let mut aggregated_logs: Vec<&Log> = Vec::new();
            loop {
                let logs_future = self.rpc_provider
                    .get_logs(
                        FilterBlockOption::Range { 
                            from_identifier: BlockIdentifier::Number(from_number), 
                            to_identifier: BlockIdentifier::Number(to_number) }, &self.options.filter);

                if fill_cache {
                    let blocks_batch_future = self.rpc_provider
                        .get_minimal_block_batch(BlockIdentifier::Number(from_number), BlockIdentifier::Number(to_number));

                    let (logs, blocks) = try_join!(logs_future, blocks_batch_future)?;

                    let _ = blocks.iter().map(|block| self.blocks_map.insert(block.number.as_u64(), block));
                    logs.iter().for_each(|log| aggregated_logs.push(log));

                    if logs.len() > 0 {
                        let last_log = logs.last().unwrap();
                        self.blocks_map.get(&last_log.block_number.unwrap().as_u64()).ok_or("Log where the block has been emitted is not in cache".to_string())?;


                    }
                } else {
                    logs_future.await?.iter().for_each(|log| aggregated_logs.push(log));
                }
 
                from_number = to.number.as_u64();
                to_number = std::cmp::min(from_number + self.options.batch_size as u64, to.number.as_u64()) ;

                if from_number == to.number.as_u64() {
                    break;
                }
            }

            Ok(aggregated_logs)
        }

        async fn handle_block(&mut self, block: &'a Block<TxHash>) -> Result<(&Block<TxHash>, Option<&Block<TxHash>>, Vec<&Log>), String> {
            let last_block = self.last_block.ok_or("Missing last block")?;
            let mut rollback_block: Option<&Block<TxHash>> = None;
            if last_block.hash != block.parent_hash {
                event!(Level::WARN, "handle_block reorg detected last_block: {}, newblock: {}", last_block, block);

                let common_ancestor = self.find_common_ancestor().await?;

                /* clean cache from reorged blocks */
                for bn in(common_ancestor.number.as_u64() + 1)..last_block.number.as_u64() {
                    self.blocks_map.remove(&bn);
                }

                self.last_block = Some(&common_ancestor);
                rollback_block = Some(common_ancestor);

                event!(Level::DEBUG, "common ancestor found {}", common_ancestor);
            }

            let logs = self.handle_batch_block(last_block, &block, true).await?;

            Ok((&block, rollback_block, logs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tracker::*;
    use async_trait::async_trait;
    use ethers::types::{TxHash, Log};

    #[derive(Clone, Debug)]
    struct MockRpcProvider<TX> {
        latest: Block<TX>,
    }

    #[async_trait]
    impl BlockProvider<TxHash> for MockRpcProvider<TxHash> {

        async fn get_block(&self, number: BlockIdentifier) -> Result<&Block<TxHash>, String> {
            Ok(&self.latest)
        }

        async fn get_minimal_block_batch<'a>(&self, from: BlockIdentifier, to: BlockIdentifier) -> Result<Vec<&'a Block<TxHash>>, String> {
            let vec: Vec<&Block<TxHash>> = Vec::new();
            Ok(vec)
        }

        async fn get_logs<'a>(&self, filter_block: FilterBlockOption, filter: &Filter) -> Result<Vec<&'a Log>, String> {
            let vec: Vec<&Log> = Vec::new();
            Ok(vec)
        }

    }

    #[test]
    fn test() {
        assert_eq!(true, true);
    }
}
