pub mod providers {

    use std::time::Duration;

    use async_trait::async_trait;
    use ethers::types::{Log, TxHash};
    use ethers_core::types::{H256, U64};
    use ethers_providers::{Http, Middleware, Provider};
    use tokio::time;
    use tracker::tracker::{Block, BlockIdentifier, BlockProvider, Filter, FilterBlockOption};

    use anvil_helpers::multicall_2::{multicall_2, Call};

    #[derive(Clone, Debug)]
    pub struct HttpProvider {
        provider: Provider<Http>,
        retries: u32,
        retry_delay_ms: u64,
        multicall2: multicall_2::Multicall2<Provider<Http>>,
    }

    impl HttpProvider {
        pub fn new(
            url: String,
            retries: u32,
            retry_delay_ms: u64,
            multicall2: multicall_2::Multicall2<Provider<Http>>,
        ) -> Self {
            HttpProvider {
                provider: Provider::<Http>::try_from(url).unwrap(),
                retries,
                retry_delay_ms,
                multicall2,
            }
        }
    }

    fn ethers_block_to_tracker_block(
        block: ethers::types::Block<TxHash>,
    ) -> Result<Block<TxHash>, String> {
        Ok(Block {
            hash: block.hash.ok_or("Missing block hash")?,
            parent_hash: block.parent_hash,
            number: block.number.ok_or("Missing block number")?,
            transactions: block.transactions,
            nonce: block.nonce,
            logs_bloom: block.logs_bloom,
        })
    }

    #[async_trait]
    impl BlockProvider<TxHash> for HttpProvider {
        async fn get_block(&self, identifier: &BlockIdentifier) -> Result<Block<TxHash>, String> {
            let mut error = "Error".to_string();
            for _ in 0..self.retries {
                let result = self.provider.get_block(identifier.to_block_id()).await;
                match result {
                    Err(e) => {
                        error = e.to_string();
                    }
                    Ok(block) => {
                        return Ok(ethers_block_to_tracker_block(block.unwrap())?);
                    }
                }
                time::sleep(Duration::from_millis(self.retry_delay_ms)).await;
            }

            Err(format!(
                "Failed {} times, last error is {}",
                self.retries, error
            ))
        }

        async fn get_minimal_block_batch(
            &self,
            from: u64,
            to: u64,
        ) -> Result<Vec<Block<TxHash>>, String> {
            let mut calls = Vec::new();

            for block_number in (from - 1)..(to + 1) {
                let call_data = self
                    .multicall2
                    .encode("getBlockHash", block_number)
                    .unwrap();

                calls.push(Call {
                    target: self.multicall2.address(),
                    call_data,
                });
            }

            let calls_results = self.multicall2.try_aggregate(true, calls).call().await;

            let blocks_encoded = calls_results.map_err(|err| err.to_string())?;

            let blocks_len = blocks_encoded.len();
            let mut blocks = Vec::new();

            let mut last_hash = H256::zero();
            for (index, block) in blocks_encoded.into_iter().enumerate() {
                let hash = self
                    .multicall2
                    .decode_output("getBlockHash", block.return_data)
                    .unwrap();

                if index == blocks_len - 1 {
                    break;
                }
                blocks.push(Block {
                    number: U64([from + index as u64; 1]),
                    hash,
                    parent_hash: last_hash,
                    transactions: Vec::new() as Vec<TxHash>,
                    nonce: None,
                    logs_bloom: None,
                });

                last_hash = hash
            }

            Ok(blocks)
        }

        async fn get_logs(
            &self,
            filter_block: FilterBlockOption,
            filter: &Filter,
        ) -> Result<Vec<&Log>, String> {
            Err("".to_string())
        }
    }
}

#[cfg(test)]
mod test {

    use std::{path::PathBuf, sync::Arc, time::Duration};

    use super::providers::HttpProvider;
    use ethers::{
        contract::ContractFactory,
        core::utils::Anvil,
        middleware::SignerMiddleware,
        providers::{Http, Provider},
        signers::{LocalWallet, Signer},
        types::H256,
        utils::AnvilInstance,
    };

    use tokio::time::sleep;
    use tracing::{debug, event};
    use tracing_test::traced_test;
    use tracker::tracker::{BlockIdentifier, BlockProvider};

    use anvil_helpers::{
        helpers::get_bytecode_from_forge_artifact,
        multicall_2::{multicall_2, Multicall2},
    };

    fn setup() -> AnvilInstance {
        Anvil::new().block_time(1 as u64).spawn()
    }

    async fn setup_anvil() -> (AnvilInstance, Multicall2<Provider<Http>>) {
        let anvil = setup();
        let provider = Provider::<Http>::try_from(anvil.endpoint())
            .unwrap()
            .interval(Duration::from_millis(10u64));
        // let provider = Provider::<Http>::try_from("http://127.0.0.1:8545").unwrap().interval(Duration::from_millis(10u64));

        let mut wallet: LocalWallet = anvil.keys()[0].clone().into();
        wallet = wallet.with_chain_id(anvil.chain_id());

        let client = SignerMiddleware::new(&provider, wallet);
        let client = Arc::new(client);

        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push(
            "../anvil-helpers/contracts/anvil-helpers-contracts/out/Multicall2.sol/Multicall2.json",
        );

        let bytecode = get_bytecode_from_forge_artifact(d).unwrap();

        let abi = multicall_2::MULTICALL2_ABI.clone();
        let factory = ContractFactory::new(abi, bytecode, client.clone());

        let mut contract_creation_tx = factory.deploy(()).unwrap();

        contract_creation_tx.tx.set_gas(1_000_000);
        let result = contract_creation_tx.send().await;

        assert_eq!(result.is_ok(), true);

        let instance = result.unwrap();

        let contract = multicall_2::Multicall2::new(instance.address(), Arc::new(provider));

        (anvil, contract)
    }

    #[traced_test]
    #[tokio::test]
    async fn test_deploy_multicall2() {
        let (anvil, multicall2) = setup_anvil().await;

        let bn = multicall2.get_block_number().await;
        assert_eq!(bn.unwrap().as_u32(), 1)
    }

    #[traced_test]
    #[tokio::test]
    async fn test() {
        let (anvil, multicall2) = setup_anvil().await;

        let http_url = format!("http://127.0.0.1:{}", anvil.port());

        let http_provider = HttpProvider::new(http_url, 5, 200, multicall2);
        let block = http_provider
            .get_block(&BlockIdentifier::Latest)
            .await
            .unwrap();

        assert_eq!(block.number.as_u64(), 1);

        sleep(Duration::from_secs(2)).await;

        let block = http_provider
            .get_block(&BlockIdentifier::Latest)
            .await
            .unwrap();
        assert_eq!(block.number.as_u64() > 0, true);
    }

    #[traced_test]
    #[tokio::test]
    async fn test_get_block_batch() {
        let (anvil, multicall2) = setup_anvil().await;

        let http_url = format!("http://127.0.0.1:{}", anvil.port());

        let http_provider = HttpProvider::new(http_url, 5, 200, multicall2);
        let block = http_provider
            .get_block(&BlockIdentifier::Latest)
            .await
            .unwrap();

        assert_eq!(block.number.as_u64(), 1);

        sleep(Duration::from_secs(2)).await;

        let blocks = http_provider.get_minimal_block_batch(1, 3).await.unwrap();

        debug!("{:#?} success", blocks);

        let mut hash = H256::zero();
        let mut number = 1;
        for block in blocks {
            assert_eq!(block.parent_hash == hash, true);
            assert_eq!(block.number.as_u64() == number, true);
            number = number + 1;
            hash = block.hash;
        }

        assert_eq!(number - 1, 3);
    }
}
