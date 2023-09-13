#![allow(dead_code)]

pub mod tracker {
    use async_trait::async_trait;
    use tokio::try_join;

    use core::fmt;
    use std::{collections::HashMap, fmt::Debug};
    use tracing::{event, Level};

    use ethers::{
        core::types::TxHash,
        types::{Address, Bloom, Log, Topic, ValueOrArray, H256, H64, U64},
    };

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

    pub struct BlockCache<Provider: BlockProvider<TxHash>> {
        pub rpc_provider: Provider,

        pub options: ChainCacheOptions,

        pub last_block: Option<Block<TxHash>>,

        pub blocks_map: HashMap<u64, Block<TxHash>>,
    }

    pub enum BlockIdentifier {
        Latest,
        Number(u64),
        Hash(H256),
    }

    pub enum FilterBlockOption {
        Range { from: u64, to: u64 },
    }

    #[async_trait]
    pub trait BlockProvider<TX>: Debug + Send + Sync {
        async fn get_block(&self, number: BlockIdentifier) -> Result<Block<TX>, String>;

        async fn get_minimal_block_batch(
            &self,
            from: u64,
            to: u64,
        ) -> Result<Vec<Block<TX>>, String>;

        async fn get_logs(
            &self,
            filter_block: FilterBlockOption,
            filter: &Filter,
        ) -> Result<Vec<&Log>, String>;
    }

    impl<Provider: BlockProvider<TxHash>> BlockCache<Provider> 
    {
        fn add_block(&mut self, block: Block<TxHash>) -> &Block<TxHash> {
            event!(Level::INFO, "add_block {}", block);
            let inserted_block = self.blocks_map.insert(block.number.as_u64(), block);
            self.last_block = Some(inserted_block.unwrap());
            return &self.last_block.as_ref().unwrap();
        }

        async fn initialize(
            &mut self,
            block: Block<TxHash>,
        ) -> Result<&Block<TxHash>, String> {
            let rpc_block = self
                .rpc_provider
                .get_block(BlockIdentifier::Hash(block.hash))
                .await?;

            if rpc_block.hash != block.hash {
                return Err(
                    "Rpc block hash not equal to the one provided to initialize".to_string()
                );
            }

            Ok(self.add_block(block))
        }

        async fn find_common_ancestor(&self) -> Result<&Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self
                .last_block
                .as_ref()
                .ok_or("No last block available".to_string())?;
            let min_block_back =
                std::cmp::min(self.blocks_map.len() as u32, self.options.block_back_count);
            let from = last_block.number - min_block_back;
            let batch = self
                .rpc_provider
                .get_minimal_block_batch(from.as_u64(), last_block.number.as_u64() - 1)
                .await?;

            for block in batch.iter().rev() {
                let cached_block = self
                    .blocks_map
                    .get(&block.number.as_u64())
                    .ok_or("Missing cached block".to_string())?;
                if cached_block.hash == block.hash {
                    return Ok(cached_block);
                }
            }

            Err("Did not find common ancestor".to_string())
        }

        async fn handle_batch_block(
            &mut self,
            from: Block<TxHash>,
            to: Block<TxHash>,
            fill_cache: bool,
        ) -> Result<Vec<&Log>, String> {
            let mut from_number = from.number.as_u64() + 1;
            let mut to_number = std::cmp::min(
                from_number + self.options.batch_size as u64,
                to.number.as_u64(),
            );
            let mut aggregated_logs: Vec<&Log> = Vec::new();
            loop {
                let logs_future = self.rpc_provider.get_logs(
                    FilterBlockOption::Range {
                        from: from_number,
                        to: to_number,
                    },
                    &self.options.filter,
                );

                if fill_cache {
                    let blocks_batch_future = self
                        .rpc_provider
                        .get_minimal_block_batch(from_number, to_number);

                    let (logs, blocks) = try_join!(logs_future, blocks_batch_future)?;

                    let _ = blocks
                        .into_iter()
                        .map(|block| self.blocks_map.insert(block.number.as_u64(), block));

                    logs.iter().for_each(|log| aggregated_logs.push(log));

                    if logs.len() > 0 {
                        let last_log = logs.last().unwrap();
                        self.blocks_map
                            .get(&last_log.block_number.unwrap().as_u64())
                            .ok_or(
                                "Log where the block has been emitted is not in cache".to_string(),
                            )?;
                    }
                } else {
                    logs_future
                        .await?
                        .iter()
                        .for_each(|log| aggregated_logs.push(log));
                }

                from_number = to.number.as_u64();
                to_number = std::cmp::min(
                    from_number + self.options.batch_size as u64,
                    to.number.as_u64(),
                );

                if from_number == to.number.as_u64() {
                    break;
                }
            }

            Ok(aggregated_logs)
        }

        async fn handle_block(
            &mut self,
            block: Block<TxHash>,
        ) -> Result<(Block<TxHash>, Option<Block<TxHash>>, Vec<&Log>), String> {
            let last_block = self.last_block.as_ref().ok_or("Missing last block")?;
            let mut rollback_block: Option<Block<TxHash>> = None;
            if last_block.hash != block.parent_hash {
                event!(
                    Level::WARN,
                    "handle_block reorg detected last_block: {}, newblock: {}",
                    last_block,
                    block
                );

                let common_ancestor = self.find_common_ancestor().await?;

                rollback_block = Some(common_ancestor.to_owned());
                event!(Level::DEBUG, "common ancestor found {}", common_ancestor);
            }

            /* clean cache from reorged blocks */
            if let Some(last_safe_block) = rollback_block.as_ref() {
                for bn in (last_safe_block.number.as_u64() + 1)..last_block.number.as_u64() {
                    self.blocks_map.remove(&bn);
                }
            }
            self.last_block = rollback_block.clone();

            let from = self.last_block.clone().unwrap();
            let logs = self.handle_batch_block(from, block.clone(), true).await?;

            Ok((block, rollback_block, logs))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::tracker::*;
    use async_trait::async_trait;
    use ethers::types::{Log, TxHash, H256, U64, U256};

    #[derive(Clone, Debug)]
    struct MockRpcProvider<TX> {
        // latest: Block<TX>,
        blocks_map_by_number: HashMap<u64, (Block<TX>, Vec<Log>)>,
        // blocks_map_by_hash: HashMap<H256, (Block<TX>, Vec<Log>)>,
    }

    fn generate_mock_provider(from: u64, to: u64, logs_count: u32, diverge_at: u64) -> MockRpcProvider<TxHash> {
        let mut blocks_map_by_number: HashMap<u64, (Block<TxHash>, Vec<Log>)> = HashMap::new();
        // let mut blocks_map_by_hash: HashMap<H256, (Block<TxHash>, Vec<Log>)> = HashMap::new();

        let mut zero = H256::zero();
        let initial_hash_values = zero.as_bytes_mut();
        let mut parent_hash = H256::from_slice(initial_hash_values);
        
        for bn in from..to {
            initial_hash_values[0] += 1;
            let mut hash = H256::from_slice(initial_hash_values);
            if bn >= diverge_at {
                hash = H256::random();
            }


            let block = Block {
                number: U64([bn; 1]),
                parent_hash,
                hash,
                transactions: None,
                nonce: None,
                logs_bloom: None,
            };

            let mut logs: Vec<Log> = Vec::new();

            for i in 0..logs_count {
                let mut log = Log::default();
                log.block_hash = Some(block.hash);
                log.block_number = Some(block.number);
                log.log_index = Some(U256([i as u64, 0, 0, 0]));

                logs.push(log);
            }
                       
            parent_hash = block.hash;

            blocks_map_by_number.insert(bn, (block, logs));
            // blocks_map_by_hash.insert(block.hash, (block, logs));

        }

        MockRpcProvider {
            // blocks_map_by_hash,
            blocks_map_by_number,
            // latest: blocks_map_by_number.get(&to).unwrap().0
        }

    }

    #[async_trait]
    impl BlockProvider<TxHash> for MockRpcProvider<TxHash> {
        async fn get_block(&self, number: BlockIdentifier) -> Result<Block<TxHash>, String> {
            match number {
                // BlockIdentifier::Latest => Ok(self.latest),
                BlockIdentifier::Latest => Err("Not implemented".to_string()),
                BlockIdentifier::Number(block_number) => Ok(self
                    .blocks_map_by_number
                    .get(&block_number)
                    .ok_or("MockRpcProvider get block by number: missing block")?
                    .0.clone()),
                // BlockIdentifier::Hash(block_hash) => Ok(self
                //     .blocks_map_by_hash
                //     .get(&block_hash)
                //     .ok_or("MockRpcProvider get block by hash: missing block")?
                //     .0),
                BlockIdentifier::Hash(_block_hash) => Err("Error".to_string()),
            }
        }

        async fn get_minimal_block_batch(
            &self,
            from_number: u64,
            to_number: u64,
        ) -> Result<Vec<Block<TxHash>>, String> {
            let mut vec: Vec<Block<TxHash>> = Vec::new();

            for bn in from_number..to_number {
                vec.push(
                    self.blocks_map_by_number
                        .get(&bn)
                        .ok_or("MockRpcProvider: get_minimal_block_batch: missing block number")?
                        .0.clone(),
                )
            }
            Ok(vec)
        }

        async fn get_logs(
            &self,
            filter_block: FilterBlockOption,
            _filter: &Filter,
        ) -> Result<Vec<&Log>, String> {
            let mut aggregated_logs: Vec<&Log> = Vec::new();

            match filter_block {
                FilterBlockOption::Range { from, to } => {
                    for bn in from..to {
                        let logs = &self
                            .blocks_map_by_number

                            .get(&bn)
                            .ok_or("MockRpcProvider: get_logs: missing logs")?
                            .1;
                        logs.iter().for_each(|log| aggregated_logs.push(log));
                    }
                }
            }

            Ok(aggregated_logs)
        }
    }

    #[test]
    fn test() {
        assert_eq!(true, true);
    }
}
