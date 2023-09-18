pub mod providers {

    use async_trait::async_trait;
    use ethers::types::{TxHash, Log};
    use ethers_providers::{Http, Provider};
    use tracker::tracker::{BlockProvider, Block, BlockIdentifier, FilterBlockOption, Filter};

    #[derive(Clone, Debug)]
    struct HttpProvider {
        provider: Provider<Http>,
    }

    impl HttpProvider {
        pub fn new(url: String) -> Self {
            HttpProvider {
                provider: Provider::<Http>::try_from(url).unwrap(),
            }
        }
    }

    #[async_trait]
    impl BlockProvider<TxHash> for HttpProvider {
        async fn get_block(&self, number: &BlockIdentifier) -> Result<Block<TxHash>, String> {
            Err("".to_string())
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
