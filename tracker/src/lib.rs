#![allow(dead_code)]

pub mod tracker {
    use async_trait::async_trait;
    use tokio::try_join;

    use core::fmt;
    use std::{collections::HashMap, fmt::Debug};
    use tracing::{event, Level};

    use ethers::{
        core::types::TxHash,
        types::{Address, Bloom, Log, Topic, ValueOrArray, H256, H64, U64, BlockId, BlockNumber},
    };

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct Block<TX> {
        pub hash: H256,

        pub parent_hash: H256,

        pub number: U64,

        pub transactions: Vec<TX>,

        pub nonce: Option<H64>,

        pub logs_bloom: Option<Bloom>,
    }

    impl fmt::Display for Block<TxHash> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "(hash: {}, parent_hash: {}, number: {}, transactions_length: {:?}, nonce: {}, logs_bloom: {})", 
                self.hash,
                self.parent_hash,
                self.number,
                self.transactions.len(),
                self.nonce.as_ref().map(|nonce| nonce.to_string()).unwrap_or("No nonce".to_string()), 
                self.logs_bloom.as_ref().map(|bloom| bloom.to_string()).unwrap_or("No bloom".to_string()))
        }
    }

    pub struct Filter {
        pub address: Option<ValueOrArray<Address>>,

        pub topics: Option<[Option<Topic>; 4]>,
    }

    pub struct CacheOptions {
        pub max_block_cached: u32,

        pub batch_size: u32,

        pub block_back_count: u32,

        pub filter: Filter,
    }

    pub struct BlockCache<'a, Provider: BlockProvider<TxHash>> {
        pub rpc_provider: &'a Provider,

        pub options: CacheOptions,

        pub last_block: Option<u64>,

        pub blocks_map: HashMap<u64, Block<TxHash>>,
    }

    pub enum BlockIdentifier {
        Latest,
        Number(u64),
        Hash(H256),
    }

    impl BlockIdentifier {
        pub fn to_block_id(&self) -> BlockId {
            match self {
                BlockIdentifier::Latest => BlockId::Number(BlockNumber::Latest),
                BlockIdentifier::Number(number) => BlockId::Number(BlockNumber::Number(U64([number.clone()]))),
                BlockIdentifier::Hash(hash) => BlockId::Hash(hash.clone()),
            } 
        }
    }

    pub enum FilterBlockOption {
        Range { from: u64, to: u64 },
    }

    #[async_trait]
    pub trait BlockProvider<TX>: Debug + Send + Sync {
        async fn get_block(&self, number: &BlockIdentifier) -> Result<Block<TX>, String>;

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

    impl<'a, Provider: BlockProvider<TxHash>> BlockCache<'a, Provider> {
        pub fn new(rpc_provider: &'a Provider, options: CacheOptions) -> Self {
            BlockCache {
                rpc_provider,
                options,
                last_block: None,
                blocks_map: HashMap::new(),
            }
        }

        pub async fn initialize(&mut self, block: &Block<TxHash>) -> Result<(), String> {
            let rpc_block = self
                .rpc_provider
                .get_block(&BlockIdentifier::Hash(block.hash))
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

            Ok(())
        }

        async fn find_common_ancestor(&self) -> Result<&Block<TxHash>, String> {
            if self.blocks_map.len() < 2 {
                return Err("Block cache too small can't find ancestor".to_string());
            }

            let last_block = self
                .last_block
                .ok_or("No last block available".to_string())?;
            let min_block_back = std::cmp::min(
                self.blocks_map.len() as u32 - 1,
                self.options.block_back_count,
            );

            event!(
                Level::DEBUG,
                "find_common_ancestor last_block: {} min_block_back {} blocks_map length {} {:?}",
                last_block,
                min_block_back,
                self.blocks_map.len(),
                self.blocks_map,
            );
            let from = last_block - min_block_back as u64;
            let batch = self
                .rpc_provider
                .get_minimal_block_batch(from, last_block)
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

        pub async fn handle_batch_block(
            &mut self,
            from: &u64,
            to_number: &u64,
            fill_cache: bool,
        ) -> Result<Vec<&Log>, String> {
            let mut from_number = from.clone();

            event!(
                Level::DEBUG,
                "handle_batch_block {} {} {}",
                from_number,
                to_number,
                fill_cache
            );
            let mut to_number = std::cmp::min(
                from_number + self.options.batch_size as u64,
                to_number.clone(),
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

                    blocks.into_iter().for_each(|block| {
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
                from_number = to_number;

                event!(Level::DEBUG, "New last block {}", to_number);

                to_number = std::cmp::min(
                    from_number + self.options.batch_size as u64,
                    to_number,
                );

                if from_number == to_number {
                    break;
                }
            }

            Ok(aggregated_logs)
        }

        fn get_last_block(&self) -> Result<&Block<TxHash>, String> {
            let last_block_number = self.last_block.ok_or("Missing last block number")?;

            event!(Level::DEBUG, "get last block {}", last_block_number);

            let last_block = self
                .blocks_map
                .get(&last_block_number)
                .ok_or("Missing last block")?;

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
                let new_last_block_number = last_safe_block.number.as_u64();

                for bn in (last_safe_block.number.as_u64() + 1)..last_block.number.as_u64() + 1 {
                    event!(Level::DEBUG, "Remove block with number = {}", bn);
                    self.blocks_map.remove(&bn);
                }

                event!(
                    Level::DEBUG,
                    "set last block number {} length {}",
                    last_safe_block,
                    self.blocks_map.len()
                );
                self.last_block = Some(new_last_block_number);
                self.blocks_map
                    .insert(new_last_block_number, last_safe_block.clone());
            }

            let logs = self
                .handle_batch_block(
                    &(self.blocks_map
                        .get(&self.last_block.unwrap())
                        .unwrap()
                        .number.as_u64() + 1),
                    &block.number.as_u64(),
                    true,
                )
                .await?;

            Ok((block, rollback_block, logs))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::tracker::*;
    use async_trait::async_trait;
    use ethers::types::{Log, TxHash, H256, U256, U64};
    use tracing::debug;
    use tracing_test::traced_test;

    #[derive(Clone, Debug)]
    struct MockRpcProvider<TX> {
        latest: Block<TX>,
        blocks_map_by_number: HashMap<u64, (Block<TX>, Vec<Log>)>,
        blocks_map_by_hash: HashMap<H256, (Block<TX>, Vec<Log>)>,
    }

    fn generate_mock_provider(
        from: u64,
        to: u64,
        logs_count: u32,
        diverge_at: u64,
        prefix: u8,
    ) -> MockRpcProvider<TxHash> {
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
                transactions: Vec::new(),
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
            blocks_map_by_number,
        }
    }

    #[async_trait]
    impl BlockProvider<TxHash> for MockRpcProvider<TxHash> {
        async fn get_block(&self, number: &BlockIdentifier) -> Result<Block<TxHash>, String> {
            match number {
                BlockIdentifier::Latest => Ok(self.latest.clone()),
                BlockIdentifier::Number(block_number) => Ok(self
                    .blocks_map_by_number
                    .get(&block_number)
                    .ok_or("MockRpcProvider get block by number: missing block")?
                    .0
                    .clone()),
                BlockIdentifier::Hash(block_hash) => Ok(self
                    .blocks_map_by_hash
                    .get(&block_hash)
                    .ok_or("MockRpcProvider get block by hash: missing block")?
                    .0
                    .clone()),
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
                        .0
                        .clone(),
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
                    debug!("get logs block {} {}", from, to);
                    for bn in from..to + 1 {
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
         hash.as_bytes()
            .iter()
            .map(|byte| format!("{:02X}", byte))
            .collect::<Vec<_>>()
            .join("") // Combine the hexadecimal values into a single strin
    }

    fn block_to_string_minimal(block: &Block<TxHash>) -> String {
        format!(
            "[{},{},{}]",
            block.number,
            remove_leading_zeros(h256_to_string(block.hash)),
            remove_leading_zeros(h256_to_string(block.parent_hash))
        )
    }

    fn block_option_to_string(block: Option<Block<TxHash>>) -> String {
        match block {
            None => "None".to_string(),
            Some(blk) => block_to_string_minimal(&blk),
        }
    }

    fn log_to_string(log: &Log) -> String {
        format!(
            "{},{},{}",
            log.block_number.unwrap(),
            remove_leading_zeros(h256_to_string(log.block_hash.unwrap())),
            log.log_index.unwrap()
        )
    }

    fn concatenated_logs(logs: Vec<&Log>) -> String {
        logs.iter()
            .enumerate()
            .fold(String::new(), |acc, (index, &log)| {
                if index == 0 {
                    log_to_string(&log)
                } else {
                    acc + "|" + &log_to_string(&log)
                }
            })
    }

    enum ScenarioTask<'a> {
        SetCacheProvider(&'a MockRpcProvider<TxHash>),
        InitializeCache(&'a Block<TxHash>),
        InjectBlockFromProvider((&'a MockRpcProvider<TxHash>, BlockIdentifier, String)),
        InjectRangeFromProvider(
            (
                u64,
                u64,
                String,
            ),
        ),
    }

    async fn test_scneario<'a>(
        options: CacheOptions,
        rpc_provider: &MockRpcProvider<TxHash>,
        tasks: Vec<ScenarioTask<'a>>,
    ) {
        let mut cache = BlockCache::new(rpc_provider, options);

        for task in tasks.iter() {
            debug!("new task");
            match task {
                ScenarioTask::SetCacheProvider(provider) => {
                    cache.rpc_provider = provider;
                }
                ScenarioTask::InitializeCache(block) => {
                    cache.initialize(block).await.unwrap();
                }
                ScenarioTask::InjectBlockFromProvider((provider, identifier, expected_result)) => {
                    let block = provider.get_block(identifier).await.unwrap();
                    let (returned_block, last_safe_block, logs) =
                        cache.handle_block(block).await.unwrap();

                    let logs = concatenated_logs(logs);

                    let _result = format!(
                        "{},{},[{}]",
                        block_to_string_minimal(&returned_block),
                        block_option_to_string(last_safe_block),
                        logs,
                    );

                    assert_eq!(_result, expected_result.clone());
                }
                ScenarioTask::InjectRangeFromProvider((from, to, expected_result)) => {
                    let logs = cache.handle_batch_block(from, to, false).await.unwrap();

                    let logs = concatenated_logs(logs);

                    let _result = format!(
                        "[{}]",
                        logs,
                    );

                    assert_eq!(_result, expected_result.clone());
                }
            }
        }
    }

    #[traced_test]
    #[tokio::test]
    async fn test_scnearios_with_reorg_protection() {
        let mock_provider1 = generate_mock_provider(1, 10, 1, 5, 1);
        let mock_provider2 = generate_mock_provider(1, 10, 1, 2, 2);

        let options = CacheOptions {
            max_block_cached: 32,
            batch_size: 10,
            block_back_count: 5,
            filter: Filter {
                address: None,
                topics: None,
            },
        };

        let initial_block = mock_provider1
            .get_block(&BlockIdentifier::Number(1))
            .await
            .unwrap();

        let tasks: Vec<ScenarioTask> = vec![
            ScenarioTask::InitializeCache(&initial_block),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider1,
                BlockIdentifier::Number(2),
                "[2,0x2,0x1],None,[2,0x2,0]".to_string(),
            )),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider1,
                BlockIdentifier::Number(3),
                "[3,0x3,0x2],None,[3,0x3,0]".to_string(),
            )),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider1,
                BlockIdentifier::Number(5),
                "[5,0x5,0x4],[3,0x3,0x2],[4,0x4,0|5,0x5,0]".to_string(),
            )),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider1,
                BlockIdentifier::Number(6),
                "[6,0x106,0x5],None,[6,0x106,0]".to_string(),
            )),
            ScenarioTask::SetCacheProvider(&mock_provider2),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider2,
                BlockIdentifier::Number(5),
                "[5,0x205,0x204],[2,0x2,0x1],[3,0x203,0|4,0x204,0|5,0x205,0]".to_string(),
            )),
            ScenarioTask::SetCacheProvider(&mock_provider1),
            ScenarioTask::InjectBlockFromProvider((
                &mock_provider1,
                BlockIdentifier::Number(7),
                "[7,0x107,0x106],[2,0x2,0x1],[3,0x3,0|4,0x4,0|5,0x5,0|6,0x106,0|7,0x107,0]".to_string(),
            )),
        ];

        test_scneario(options, &mock_provider1, tasks).await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_scnearios_with_range() {
        let mock_provider1 = generate_mock_provider(1, 12, 1, 100, 1);

        let options = CacheOptions {
            max_block_cached: 32,
            batch_size: 10,
            block_back_count: 5,
            filter: Filter {
                address: None,
                topics: None,
            },
        };

        let initial_block = mock_provider1
            .get_block(&BlockIdentifier::Number(1))
            .await
            .unwrap();

        let tasks: Vec<ScenarioTask> = vec![
            ScenarioTask::InitializeCache(&initial_block),
            ScenarioTask::InjectRangeFromProvider((
                2,
                4,
                "[2,0x2,0|3,0x3,0|4,0x4,0]".to_string(),
            )),
            ScenarioTask::InjectRangeFromProvider((
                5,
                10,
                "[5,0x5,0|6,0x6,0|7,0x7,0|8,0x8,0|9,0x9,0|10,0xA,0]".to_string(),
            )),
        ];

        test_scneario(options, &mock_provider1, tasks).await;
    }
}
