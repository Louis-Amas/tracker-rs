pub mod anvil_helpers {
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Sender};

    use std::process::{Command, ExitStatus, Stdio};
    use std::thread::{self, sleep, JoinHandle};

    use regex::Regex;
    use std::net::TcpStream;
    use std::time::Duration;
    use tracing::{event, Level};

    pub struct Account {
        pub public_key: &'static str,
        pub private_key: &'static str,
    }

    pub struct ContractInfo {
        pub path: String,
        pub name: String,
    }

    pub struct Anvil {
        pub port: u16,
        pub accounts: [Account; 6],
        thread: JoinHandle<Result<ExitStatus, String>>,
        sender_parent_to_child: Sender<bool>,
        pub contracts_map: HashMap<String, String>,
    }

    pub fn is_port_open(host: &str, port: u16) -> bool {
        match TcpStream::connect(format!("{}:{}", host, port)) {
            Ok(_) => true,   // Connection succeeded, port is open.
            Err(_) => false, // Connection failed, port is closed or unreachable.
        }
    }

    const ACCOUNTS: [Account; 6] = [
        Account {
            public_key: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        },
        Account {
            public_key: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            private_key: "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
        },
        Account {
            public_key: "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
            private_key: "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
        },
        Account {
            public_key: "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
            private_key: "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
        },
        Account {
            public_key: "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65",
            private_key: "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
        },
        Account {
            public_key: "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc",
            private_key: "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
        },
    ];

    impl Anvil {
        pub fn new_contract(&self, contract: &ContractInfo) -> Result<String, String> {
            let mut cmd = Command::new("forge");

            cmd.current_dir("./contracts/");
            cmd.arg("create");
            cmd.arg("--rpc-url");
            cmd.arg(format!("http://127.0.0.1:{}", self.port));
            cmd.arg(format!("{}:{}", contract.path, contract.name));
            cmd.arg("--private-key");
            cmd.arg(self.accounts[0].private_key);

            let output = Stdio::piped();
            cmd.stdout(output);

            let child = cmd.spawn().map_err(|err| err.to_string())?;

            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            let regex = Regex::new(r"Deployed to: (0x[0-9a-fA-F]*)").unwrap();
            let stdout_str = String::from_utf8(output.stdout).unwrap();

            event!(Level::DEBUG, stdout_str);
            let capture = regex.captures(&stdout_str).unwrap();

            Ok(capture.get(1).unwrap().as_str().to_string())
        }

        pub fn new(port: Option<u16>, block_time_s: Option<u32>, contracts: Vec<ContractInfo>) -> Self {
            event!(Level::DEBUG, "New anvil");

            let port = port.unwrap_or(8545);

            let (sender_child_to_parent, receiver_child_to_parent) = mpsc::channel::<bool>();
            let (sender_parent_to_child, receiver_parent_to_child) = mpsc::channel::<bool>();

            let thread = thread::spawn(move || {
                let mut cmd = Command::new("anvil");

                cmd.arg("-p");
                cmd.arg(format!("{}", port));

                if block_time_s.is_some() {
                    cmd.arg("--block-time");
                    cmd.arg(format!("{}", block_time_s.unwrap()));
                } else {
                    cmd.arg("--no-mining");
                }

                cmd.stdin(Stdio::piped());
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());

                match cmd.spawn() {
                    Err(error) => Err(error.to_string()),
                    Ok(mut child) => {
                        while !is_port_open("localhost", port) {
                            sleep(Duration::from_millis(500));
                        }

                        sender_child_to_parent
                            .send(true)
                            .map_err(|err| err.to_string())?;
                        receiver_parent_to_child
                            .recv()
                            .map_err(|err| err.to_string())?;
                        child.kill().map_err(|err| err.to_string())?;
                        Ok(child.wait().map_err(|err| err.to_string())?)
                    }
                }
            });

            receiver_child_to_parent.recv().unwrap();

            let mut anvil = Anvil {
                port,
                accounts: ACCOUNTS,
                thread,
                sender_parent_to_child,
                contracts_map: HashMap::new(),
            };

            if block_time_s.is_none() && contracts.len() > 0 {
                event!(Level::ERROR, "Could not initialize contract as there is no block-time");
                return anvil;
            }

            for contract in contracts.into_iter() {
                let addr = anvil.new_contract(&contract).unwrap();

                anvil.contracts_map.insert(contract.name, addr);
            } 

            anvil
        }

        pub fn kill(self) {
            self.sender_parent_to_child.send(true).unwrap();

            self.thread.join().unwrap().unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::anvil_helpers::ContractInfo;

    use super::anvil_helpers::{is_port_open, Anvil};

    use tracing_test::traced_test;

    #[test]
    #[traced_test]
    fn test() {
        let anvil = Anvil::new(None, Some(2), Vec::new());

        assert_eq!(anvil.port, 8545);

        assert_eq!(is_port_open("localhost", anvil.port), true);

        anvil.kill();
    }

    #[test]
    #[traced_test]
    fn test_deploy_multicall2() {
        let anvil = Anvil::new(None, Some(2), vec![
            ContractInfo {
                path: "src/Multicall2.sol".to_string(),
                name: "Multicall2".to_string(),
            }]
        );

        assert_eq!(anvil.port, 8545);

        assert_eq!(is_port_open("localhost", anvil.port), true);

        let addr = anvil.contracts_map.get(&"Multicall2".to_string());

        assert_eq!(addr.is_some(), true);

        anvil.kill();
    }
}
