use std::fs;

use ethers_core::{abi::Abi, types::Bytes};

use ethers::prelude::Abigen;

pub struct ContractArtifact {
    pub abi: Abi,
    pub bytecode: Bytes,
}

fn generate_abi_file_from_forge_artifact(
    path: String,
    to: String,
    name: String,
) -> Result<(), String> {
    let contract_artifact_as_str = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let contract_artifact_as_json: serde_json::Value =
        serde_json::from_str(&contract_artifact_as_str).map_err(|err| err.to_string())?;

    let str = contract_artifact_as_json["abi"].to_string();

    Abigen::new(&name, &str)
        .unwrap()
        .generate()
        .unwrap()
        .write_module_in_dir(to)
        .unwrap();

    return Ok(());
}

fn main() {
    println!("Build script is running!");

    // Call your Rust function here
    generate_abi_file_from_forge_artifact(
        "./contracts/anvil-helpers-contracts/out/Multicall2.sol/Multicall2.json".into(), 
        "./src/".into(), 
        "Multicall2".into(),
    ).unwrap();
}
