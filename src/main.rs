use dotenv;

use ethers::providers::{Provider, Http, Middleware, ProviderError};
use ethers::core::types::{BlockNumber, BlockId, U64, TxHash};

mod types;
mod tracker;

#[derive(Debug)]
enum Error {
    ProviderError(ProviderError),
    Error(),
}

impl std::error::Error for Error {}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Implement your error formatting logic here
        // This will be used when displaying the error using the `Display` trait.
        write!(f, "Custom error: {:?}", self)
    }
}

async fn get_block(provider: Provider<Http>, number: U64) -> Result<types::block::Block<TxHash>, Error> {
    let result = provider.get_block(BlockId::Number(BlockNumber::Number(number))).await;

    let block = result.map_err(Error::ProviderError)?;

    let blck = block.ok_or(Error::Error())?;

    Ok(types::block::Block{ 
        hash: blck.hash.ok_or(Error::Error())?,
        parent_hash: blck.parent_hash,
        number: blck.number.ok_or(Error::Error())?,
        transactions: blck.transactions,
        nonce: blck.nonce.ok_or(Error::Error())?,
        logs_bloom: blck.logs_bloom.ok_or(Error::Error())?,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    let rpc_http = std::env::var("RPC_HTTP_137").unwrap();

    println!("rpc_http: {}", rpc_http);

    let provider = Provider::<Http>::try_from(rpc_http)?;

    let block_number: U64 = provider.get_block_number().await?;
    println!("{block_number}");

    let block = provider.get_block(block_number).await?;

    match block {
        None => println!("Failed to get {block_number}"),
        Some(_block) => {
            match _block.hash {
                None => println!("Block is missing hash"),
                Some(hash) => println!("{hash}"),
            }
        }
    }

    let block = get_block(provider, block_number).await?;

    println!("{}", block.hash);

    Ok(())
}
