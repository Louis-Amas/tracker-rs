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

        pub batch_size: u32,

        pub block_back_count: u32,

        pub filter: Filter,
    }

    pub struct BlockCache<'a, Provider: BlockProvider<TxHash>> {
        pub rpc_provider: &'a Provider,

        pub options: ChainCacheOptions,

        pub last_block: Option<u64>,

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

    impl<'a, Provider: BlockProvider<TxHash>> BlockCache<'a, Provider> 
    {
        pub async fn initialize(
            &mut self,
            block: Block<TxHash>,
        ) -> Result<Block<TxHash>, String> {
            let rpc_block = self
                .rpc_provider
                .get_block(BlockIdentifier::Hash(block.hash))
                .await?;

            if rpc_block.hash != block.hash {
                return Err(
                    "Rpc block hash not equal to the one provided to initialize".to_string()
                );
            }

            let bn = block.number.as_u64();
            self.last_block = Some(block.number.as_u64());
            self.blocks_map.insert(bn, block.clone());

            event!(Level::INFO, "Initialized with block {}", block);

            Ok(block)
        }

        async fn find_common_ancestor(&self) -> Result<&Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self
                .last_block
                .ok_or("No last block available".to_string())?;
            let min_block_back =
                std::cmp::min(self.blocks_map.len() as u32 - 1, self.options.block_back_count);

            event!(Level::DEBUG, "find_common_ancestor last_block: {} min_block_back {}", last_block, min_block_back);
            let from = last_block - min_block_back as u64;
            let batch = self
                .rpc_provider
                .get_minimal_block_batch(from, last_block - 1)
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
            event!(Level::DEBUG, "handle_batch_block {} {} {}", from, to, fill_cache);
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

                    blocks
                        .into_iter()
                        .for_each(|block| {
                            self.blocks_map.insert(block.number.as_u64(), block);
                        });

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

                self.last_block = Some(to_number);
                from_number = to.number.as_u64();

                event!(Level::DEBUG, "New last block {}", to_number);

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

        fn get_last_block(&self) -> Result<&Block<TxHash>, String> {
            let last_block_number = self.last_block.ok_or("Missing last block number")?;

                event!(Level::DEBUG, "get last block {}", last_block_number);

            let last_block = self.blocks_map.get(&last_block_number).ok_or("Missing last block")?;

            Ok(last_block)
        }

        pub async fn handle_block(
            &mut self,
            block: Block<TxHash>,
        ) -> Result<(Block<TxHash>, Option<Block<TxHash>>, Vec<&Log>), String> {
            event!(Level::DEBUG, "Handle block {}", block);
            let last_block = self.get_last_block()?;

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
                let new_last_block_number = last_block.number.as_u64();

                for bn in (last_safe_block.number.as_u64() + 1)..last_block.number.as_u64() {
                    event!(Level::DEBUG, "Remove block with number > {}", bn);
                    self.blocks_map.remove(&bn);
                }

                event!(Level::DEBUG, "set last block number {}", last_safe_block);
                self.last_block = Some(new_last_block_number);
                self.blocks_map.insert(new_last_block_number, last_safe_block.clone());
            }

            let logs = self.handle_batch_block(self.blocks_map.get(&self.last_block.unwrap()).unwrap().clone(), block.clone(), true).await?;

            Ok((block, rollback_block, logs))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tracing::{info, debug};
    use super::tracker::*;
    use async_trait::async_trait;
    use ethers::types::{Log, TxHash, H256, U64, U256}; 
    use tracing_test::traced_test;

    #[derive(Clone, Debug)]
    struct MockRpcProvider<TX> {
        latest: Block<TX>,
        blocks_map_by_number: HashMap<u64, (Block<TX>, Vec<Log>)>,
        blocks_map_by_hash: HashMap<H256, (Block<TX>, Vec<Log>)>,
    }

    fn generate_mock_provider(from: u64, to: u64, logs_count: u32, diverge_at: u64, prefix: u8) -> MockRpcProvider<TxHash> {
        let mut blocks_map_by_number: HashMap<u64, (Block<TxHash>, Vec<Log>)> = HashMap::new();
        let mut blocks_map_by_hash: HashMap<H256, (Block<TxHash>, Vec<Log>)> = HashMap::new();

        let mut zero = H256::zero();
        let initial_hash_values = zero.as_bytes_mut();
        let mut parent_hash = H256::from_slice(initial_hash_values);
      
        for bn in from..to {
            initial_hash_values[31] += 1;
            if bn > diverge_at {
                initial_hash_values[30] = prefix;
            }
            let hash = H256::from_slice(initial_hash_values);

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

            debug!("insert block {} with hash {}", bn, hash);

            blocks_map_by_number.insert(bn, (block.clone(), logs.clone()));
            blocks_map_by_hash.insert(block.clone().hash, (block.clone(), logs));
        }

        MockRpcProvider {
            blocks_map_by_hash,
            latest: blocks_map_by_number.get(&(to - 1)).unwrap().0.clone(),
            blocks_map_by_number
        }

    }

    #[async_trait]
    impl BlockProvider<TxHash> for MockRpcProvider<TxHash> {
        async fn get_block(&self, number: BlockIdentifier) -> Result<Block<TxHash>, String> {
            match number {
                BlockIdentifier::Latest => Ok(self.latest.clone()),
                BlockIdentifier::Number(block_number) => Ok(self
                    .blocks_map_by_number
                    .get(&block_number)
                    .ok_or("MockRpcProvider get block by number: missing block")?
                    .0.clone()),
                BlockIdentifier::Hash(block_hash) => Ok(self
                    .blocks_map_by_hash
                    .get(&block_hash)
                    .ok_or("MockRpcProvider get block by hash: missing block")?
                    .0.clone()),
            }
        }

        async fn get_minimal_block_batch(
            &self,
            from_number: u64,
            to_number: u64,
        ) -> Result<Vec<Block<TxHash>>, String> {
            let mut vec: Vec<Block<TxHash>> = Vec::new();

            debug!("get minimal block batch {} {}", from_number, to_number);
            for bn in from_number..(to_number + 1) {
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

    fn remove_leading_zeros(string: String) -> String {
        let hex_string = string.as_str();
        // Strip the "0x" prefix if present
        let hex_string = if hex_string.starts_with("0x") {
            &hex_string[2..]
        } else {
            hex_string
        };

        // Use an iterator to find the index of the first non-zero character
        let first_non_zero_index = hex_string
            .char_indices()
            .find(|&(_, c)| c != '0')
            .map(|(idx, _)| idx)
            .unwrap_or(hex_string.len());
        let new_string = hex_string[first_non_zero_index..].to_string();
        if new_string.len() == 0 {
            "0x0".to_string()
        } else {
            format!("0x{}", new_string)
        }
    }

fn remove_trailing_zeros(hex_string: &str) -> String {
    // Check if the string starts with "0x" and split it into prefix and value
    if let Some((prefix, value)) = hex_string.split_once("0x") {
        // Remove trailing zeros from the hexadecimal value
        let trimmed_value = value.trim_end_matches('0');
        
        // Reconstruct the string with "0x" prefix and the trimmed value
        format!("{}{}", prefix, trimmed_value)
    } else {
        // If there's no "0x" prefix, return the original string
        hex_string.to_string()
    }
}
    fn h256_to_string(hash: H256) -> String {
        let str = hash.as_bytes(); 
        let num_strings: Vec<String> = str.iter().map(|&byte| byte.to_string()).collect();

        format!("0x{}", num_strings.join(""))
    }

    fn block_to_string_minimal(block: &Block<TxHash>) -> String {
        format!(
            "[{},{},{}]", 
            block.number, 
            remove_leading_zeros(h256_to_string(block.hash)), 
            remove_leading_zeros(h256_to_string(block.parent_hash))
        )
    }

    #[traced_test]
    #[tokio::test]
    async fn test() {
        let mock_provider1 = generate_mock_provider(1, 10, 2, 5, 1);

        let mock_provider2 = generate_mock_provider(1, 10, 1, 2, 2);

        let options = ChainCacheOptions {
            max_block_cached: 32,
            batch_size: 10,
            block_back_count: 5,
            filter: Filter { 
                address: None, 
                topics: None
            }
        };

        let mut cache = BlockCache {
            rpc_provider: &mock_provider1,
            options,
            last_block: None,
            blocks_map: HashMap::new(),
        };

        let mut block = mock_provider1.get_block(BlockIdentifier::Number(1)).await.unwrap();
        info!("Starting with block {}", block);

        let result = cache.initialize(block).await;
        assert_eq!(block_to_string_minimal(&result.unwrap()), "[1,0x1,0x0]");

        block = mock_provider1.get_block(BlockIdentifier::Number(2)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[2,0x2,0x1]");

        block = mock_provider1.get_block(BlockIdentifier::Number(3)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[3,0x3,0x2]");

        block = mock_provider2.get_block(BlockIdentifier::Number(3)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[3,0x23,0x2]");

        block = mock_provider1.get_block(BlockIdentifier::Number(3)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[3,0x3,0x2]");

        block = mock_provider1.get_block(BlockIdentifier::Number(4)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[4,0x4,0x3]");

        cache.rpc_provider = &mock_provider2;
        block = mock_provider2.get_block(BlockIdentifier::Number(5)).await.unwrap();
        let handle_block_result = cache.handle_block(block).await.unwrap();
        assert_eq!(block_to_string_minimal(&handle_block_result.0), "[5,0x25,0x24]");

        assert_eq!(true, true);
    }
}
