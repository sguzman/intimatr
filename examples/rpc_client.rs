use std::{env, error::Error, net::SocketAddr};

use intimatr::{command::Command, rpc::RpcClient};

const DEFAULT_ENDPOINT: &str = "127.0.0.1:31337";
const DEFAULT_MAX_REQUEST_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let endpoint: SocketAddr = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned())
        .parse()?;
    let mut client = RpcClient::connect_tcp(
        endpoint,
        DEFAULT_MAX_REQUEST_BYTES,
        DEFAULT_MAX_RESPONSE_BYTES,
    )?;

    println!("ping: {:#?}", client.call(Command::Ping)?);
    println!(
        "lifecycle: {:#?}",
        client.call(Command::LifecycleState)?
    );
    Ok(())
}
