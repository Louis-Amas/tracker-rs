pub mod providers {

    use std::time::Duration;

    use async_trait::async_trait;
    use ethers::types::{TxHash, Log};
    use ethers_providers::{Http, Provider, Middleware};
    use tracker::tracker::{BlockProvider, Block, BlockIdentifier, FilterBlockOption, Filter};
    use tokio::time;

    #[derive(Clone, Debug)]
    pub struct HttpProvider {
        provider: Provider<Http>,
        retries: u32,
        retry_delay_ms: u64,
    }

    impl HttpProvider {
        pub fn new(url: String, retries: u32, retry_delay_ms: u64) -> Self {
            HttpProvider {
                provider: Provider::<Http>::try_from(url).unwrap(),
                retries,
                retry_delay_ms,
            }
        }
    }

    fn ethers_block_to_tracker_block(block: ethers::types::Block<TxHash>) -> Result<Block<TxHash>, String> {
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
                
            Err(format!("Failed {} times, last error is {}", self.retries, error))
        }

        async fn get_minimal_block_batch(
            &self,
            from: u64,
            to: u64,
        ) -> Result<Vec<Block<TxHash>>, String> {
            Err("".to_string())
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

    use std::time::Duration;

    use super::providers::HttpProvider;
    use tracing_test::traced_test;
    use tracker::tracker::{BlockProvider, BlockIdentifier};
    use tokio::time::sleep;
    use ethers::core::utils::Anvil;

    #[traced_test]
    #[tokio::test]
    async fn test() {
        let anvil = Anvil::new()
            .block_time(1 as u64)
            .spawn();

        let http_url = format!("http://127.0.0.1:{}", anvil.port());

        let http_provider = HttpProvider::new(http_url, 5, 200);
        let block = http_provider.get_block(&BlockIdentifier::Latest).await.unwrap();

        assert_eq!(block.number.as_u64(), 0);

        sleep(Duration::from_secs(2)).await;

        let block = http_provider.get_block(&BlockIdentifier::Latest).await.unwrap();
        assert_eq!(block.number.as_u64() > 0, true);
    }

}
