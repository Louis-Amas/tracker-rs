pub mod multicall_2;

pub mod helpers {

    use std::{fs, path::PathBuf};

    use ethers_core::types::Bytes;

    pub fn get_bytecode_from_forge_artifact(path: PathBuf) -> Result<Bytes, String> {
        let contract_artifact_as_str = fs::read_to_string(path).map_err(|err| err.to_string())?;
        let contract_artifact_as_json: serde_json::Value =
            serde_json::from_str(&contract_artifact_as_str).map_err(|err| err.to_string())?;

        let mut bytecode = contract_artifact_as_json["bytecode"]["object"].to_string().as_bytes().to_vec();

        bytecode.remove(0);
        bytecode.remove(0);

        Ok(bytecode.into())
    }

}
