//! Explicit Windows/Wine command-line client for the existing Intimatr
//! speed RPC contract. It never selects a process, attaches a DLL or installs
//! timer hooks. The status field is truthful about preview-only operation.

#[cfg(not(windows))]
fn main() {
    eprintln!("intimatr-speed is a Windows/Wine client; no Linux process was contacted");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows_client {
    use std::net::SocketAddr;

    use intimatr::{
        command::{Command, CommandResult},
        rpc::RpcClient,
        speed_commands::SpeedRequest,
    };

    const MAX_REQUEST_BYTES: usize = 256 * 1024;
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    const USAGE: &str = "Usage: intimatr-speed (--pipe PIPE_NAME | --tcp 127.0.0.1:PORT) (status | increase | decrease | toggle | reset | preset MULTIPLIER)\nOnly an explicitly selected Intimatr endpoint is contacted; speed state may be preview-only.";

    #[derive(Debug, Clone, PartialEq)]
    enum Endpoint {
        Pipe(String),
        Tcp(SocketAddr),
    }

    fn parse(args: &[String]) -> Result<(Endpoint, SpeedRequest), String> {
        let (transport, address, action, preset) = match args {
            [transport, address, action] => {
                (transport.as_str(), address.as_str(), action.as_str(), None)
            }
            [transport, address, action, preset] => (
                transport.as_str(),
                address.as_str(),
                action.as_str(),
                Some(preset.as_str()),
            ),
            _ => return Err(USAGE.to_owned()),
        };
        let endpoint = match transport {
            "--pipe" => {
                if address.is_empty()
                    || address.len() > 128
                    || !address
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                {
                    return Err("pipe name must contain only ASCII letters, digits, dots, dashes and underscores".into());
                }
                Endpoint::Pipe(address.to_owned())
            }
            "--tcp" => {
                let socket: SocketAddr = address
                    .parse()
                    .map_err(|_| "TCP endpoint must be a numeric IP address and port")?;
                if !socket.ip().is_loopback() || socket.port() == 0 {
                    return Err(
                        "TCP speed commands require an explicit loopback address and nonzero port"
                            .into(),
                    );
                }
                Endpoint::Tcp(socket)
            }
            _ => return Err(USAGE.to_owned()),
        };
        let request = match (action, preset) {
            ("status", None) => SpeedRequest::Status,
            ("increase", None) => SpeedRequest::Increase,
            ("decrease", None) => SpeedRequest::Decrease,
            ("toggle", None) => SpeedRequest::Toggle,
            ("reset", None) => SpeedRequest::Reset,
            ("preset", Some(raw)) => {
                let multiplier: f64 = raw
                    .parse()
                    .map_err(|_| "preset must be a finite positive number")?;
                if !multiplier.is_finite() || multiplier <= 0.0 {
                    return Err("preset must be a finite positive number".into());
                }
                SpeedRequest::SelectPreset { multiplier }
            }
            _ => return Err(USAGE.to_owned()),
        };
        Ok((endpoint, request))
    }

    pub fn run() -> Result<(), String> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let (endpoint, request) = parse(&args)?;
        let mut client = match endpoint {
            Endpoint::Pipe(name) => {
                RpcClient::connect_named_pipe(&name, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES)
            }
            Endpoint::Tcp(address) => {
                RpcClient::connect_tcp(address, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES)
            }
        }
        .map_err(|error| format!("cannot reach selected Intimatr endpoint: {error}"))?;

        let result = client
            .call(Command::Speed { request })
            .map_err(|error| format!("speed command failed: {error}"))?;
        let CommandResult::Speed { status } = result else {
            return Err("endpoint did not return the expected speed status".into());
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&status)
                .map_err(|error| format!("cannot encode speed status: {error}"))?
        );
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn args(parts: &[&str]) -> Vec<String> {
            parts.iter().map(|part| (*part).to_owned()).collect()
        }

        #[test]
        fn explicit_loopback_and_pipe_endpoints() {
            assert!(matches!(
                parse(&args(&["--tcp", "127.0.0.1:4000", "status"])),
                Ok((Endpoint::Tcp(_), SpeedRequest::Status))
            ));
            assert!(matches!(
                parse(&args(&["--tcp", "[::1]:4000", "preset", "2.0"])),
                Ok((
                    Endpoint::Tcp(_),
                    SpeedRequest::SelectPreset { multiplier: 2.0 }
                ))
            ));
            assert!(matches!(
                parse(&args(&["--pipe", "intimatr-Game", "reset"])),
                Ok((Endpoint::Pipe(_), SpeedRequest::Reset))
            ));
        }

        #[test]
        fn refuses_network_hosts_and_ambiguous_targets() {
            for candidate in [
                args(&["--tcp", "192.168.1.2:4000", "status"]),
                args(&["--tcp", "0.0.0.0:4000", "status"]),
                args(&["--tcp", "127.0.0.1:0", "status"]),
                args(&["--pipe", "../other", "status"]),
                args(&["--pipe", r"\\.\pipe\untrusted", "status"]),
                args(&["--tcp", "127.0.0.1:4000", "preset", "NaN"]),
                args(&["--tcp", "127.0.0.1:4000", "preset", "-2"]),
                args(&["--tcp", "127.0.0.1:4000", "preset"]),
                args(&["status"]),
            ] {
                assert!(parse(&candidate).is_err(), "accepted {candidate:?}");
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_client::run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
