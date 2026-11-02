//! Explicit Linux -> Windows/Wine speed RPC handoff.
//!
//! No bottle discovery, guessed runner, process attachment, imported DLL
//! mutation or shell interpolation. The target RPC endpoint must already
//! exist in the selected, initialized Wine prefix. Game-clock application
//! remains accurately reported by the Windows client.

use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
};

const USAGE: &str = "Usage: intimatr-bottles speed --prefix ABSOLUTE_WINEPREFIX --runner ABSOLUTE_WINE --client ABSOLUTE_INTIMATR_SPEED_EXE (--tcp LOOPBACK_IP:PORT | --pipe NAME) (status | increase | decrease | toggle | reset | preset MULTIPLIER)";

#[derive(Debug, Clone, PartialEq)]
struct SpeedForward {
    prefix: PathBuf,
    runner: PathBuf,
    client: PathBuf,
    endpoint_kind: &'static str,
    endpoint: String,
    action: String,
    preset: Option<String>,
}

fn take_value(args: &[String], at: &mut usize, option: &str) -> Result<String, String> {
    let value = args
        .get(*at)
        .ok_or_else(|| format!("missing value for {option}"))?
        .clone();
    if value.is_empty() || value.starts_with("--") {
        return Err(format!("missing value for {option}"));
    }
    *at += 1;
    Ok(value)
}

impl SpeedForward {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut prefix = None;
        let mut runner = None;
        let mut client = None;
        let mut endpoint = None;
        let mut i = 0;
        while i < args.len() && args[i].starts_with("--") {
            let flag = args[i].as_str();
            i += 1;
            let value = take_value(args, &mut i, flag)?;
            match flag {
                "--prefix" if prefix.is_none() => prefix = Some(PathBuf::from(value)),
                "--runner" if runner.is_none() => runner = Some(PathBuf::from(value)),
                "--client" if client.is_none() => client = Some(PathBuf::from(value)),
                "--tcp" if endpoint.is_none() => {
                    let address: SocketAddr = value
                        .parse()
                        .map_err(|_| "--tcp requires a numeric loopback IP and port")?;
                    if !address.ip().is_loopback() || address.port() == 0 {
                        return Err(
                            "remote, wildcard and zero-port RPC endpoints are refused".into()
                        );
                    }
                    endpoint = Some(("--tcp", value));
                }
                "--pipe" if endpoint.is_none() => {
                    if value.len() > 128
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    {
                        return Err("--pipe requires a simple local pipe name".into());
                    }
                    endpoint = Some(("--pipe", value));
                }
                _ => return Err(format!("unexpected or duplicate flag {flag}. {USAGE}")),
            }
        }
        let action = args.get(i).ok_or(USAGE)?.clone();
        i += 1;
        let preset = match action.as_str() {
            "status" | "increase" | "decrease" | "toggle" | "reset" => None,
            "preset" => {
                let raw = args
                    .get(i)
                    .ok_or("preset requires a finite positive multiplier")?;
                let speed: f64 = raw
                    .parse()
                    .map_err(|_| "preset requires a finite positive multiplier")?;
                if !speed.is_finite() || speed <= 0.0 {
                    return Err("preset requires a finite positive multiplier".into());
                }
                i += 1;
                Some(raw.clone())
            }
            _ => return Err(format!("unsupported speed action {action}. {USAGE}")),
        };
        if i != args.len() {
            return Err("unexpected extra arguments after speed action".into());
        }
        let prefix = prefix.ok_or(USAGE)?;
        let runner = runner.ok_or(USAGE)?;
        let client = client.ok_or(USAGE)?;
        let (endpoint_kind, endpoint) = endpoint.ok_or(USAGE)?;
        Ok(Self {
            prefix,
            runner,
            client,
            endpoint_kind,
            endpoint,
            action,
            preset,
        })
    }

    fn validate_files(&self) -> Result<(), String> {
        for (name, path) in [
            ("Wine prefix", &self.prefix),
            ("Wine runner", &self.runner),
            ("Windows speed client", &self.client),
        ] {
            if !path.is_absolute() {
                return Err(format!("{name} must be an absolute Linux path"));
            }
        }
        if !self.prefix.is_dir() || !self.prefix.join("drive_c").is_dir() {
            return Err("Wine prefix must already be initialized and contain drive_c".into());
        }
        let runner = fs::metadata(&self.runner)
            .map_err(|error| format!("cannot inspect explicitly selected Wine runner: {error}"))?;
        if !runner.is_file() || runner.permissions().mode() & 0o111 == 0 {
            return Err("Wine runner must be an executable file".into());
        }
        if !self.client.is_file()
            || self
                .client
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
        {
            return Err("Windows speed client must be an existing .exe file".into());
        }
        Ok(())
    }

    fn execute(&self) -> Result<String, String> {
        self.validate_files()?;
        let mut command = Command::new(&self.runner);
        command
            .env("WINEPREFIX", &self.prefix)
            .arg(&self.client)
            .args([self.endpoint_kind, &self.endpoint, &self.action]);
        if let Some(preset) = &self.preset {
            command.arg(preset);
        }
        let output = command
            .output()
            .map_err(|error| format!("cannot start selected Wine runner: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Wine speed RPC client failed (exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| format!("Windows speed RPC client returned non-UTF8 output: {error}"))
    }
}

pub fn run(args: Vec<String>) -> Result<(), String> {
    let request = SpeedForward::parse(&args)?;
    let output = request.execute()?;
    print!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn args(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn refuses_implicit_targets_and_network_endpoints() {
        let base = [
            "--prefix",
            "/tmp/prefix",
            "--runner",
            "/bin/wine",
            "--client",
            "/tmp/intimatr-speed.exe",
        ];
        assert!(SpeedForward::parse(&args(&base)).is_err());
        for endpoint in ["192.168.1.2:1234", "0.0.0.0:2345", "127.0.0.1:0"] {
            let mut input = base.to_vec();
            input.extend(["--tcp", endpoint, "status"]);
            assert!(SpeedForward::parse(&args(&input)).is_err());
        }
        for invalid in ["NaN", "inf", "-2", "0"] {
            let mut input = base.to_vec();
            input.extend(["--tcp", "127.0.0.1:31337", "preset", invalid]);
            assert!(SpeedForward::parse(&args(&input)).is_err());
        }
        let mut duplicate = base.to_vec();
        duplicate.extend([
            "--tcp",
            "127.0.0.1:1234",
            "--tcp",
            "127.0.0.1:1235",
            "status",
        ]);
        assert!(SpeedForward::parse(&args(&duplicate)).is_err());
    }

    #[test]
    fn accepts_only_explicit_loopback_or_simple_local_pipe() {
        let mut tokens = args(&[
            "--prefix",
            "/tmp/prefix with spaces",
            "--runner",
            "/tmp/wine patched",
            "--client",
            "/tmp/intimatr speed.exe",
            "--pipe",
            "intimatr_local",
            "preset",
            "1.5",
        ]);
        let parsed = SpeedForward::parse(&tokens).unwrap();
        assert_eq!(parsed.action, "preset");
        assert_eq!(parsed.preset.as_deref(), Some("1.5"));
        assert_eq!(parsed.endpoint_kind, "--pipe");
        tokens.splice(tokens.len() - 4.., args(&["--tcp", "[::1]:31337", "reset"]));
        assert_eq!(SpeedForward::parse(&tokens).unwrap().endpoint_kind, "--tcp");
    }

    #[test]
    fn launches_explicit_runner_without_shell_and_preserves_prefix() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "intimatr-speed-forward-{}-{nonce}",
            std::process::id()
        ));
        let prefix = root.join("selected bottle");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        let runner = root.join("fake runner.sh");
        let client = root.join("intimatr speed.exe");
        fs::write(&client, b"fixture").unwrap();
        let mut file = fs::File::create(&runner).unwrap();
        file.write_all(b"#!/bin/sh\nprintf '%s\\n' \"$WINEPREFIX\" \"$1\" \"$2\" \"$3\"\n")
            .unwrap();
        drop(file);
        let mut perms = fs::metadata(&runner).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&runner, perms).unwrap();
        let request = SpeedForward {
            prefix: prefix.clone(),
            runner,
            client: client.clone(),
            endpoint_kind: "--tcp",
            endpoint: "127.0.0.1:1234".to_owned(),
            action: "status".to_owned(),
            preset: None,
        };
        let result = request.execute().unwrap();
        let parts = result.lines().collect::<Vec<_>>();
        assert_eq!(parts[0], prefix.to_string_lossy());
        assert_eq!(parts[1], client.to_string_lossy());
        assert_eq!(parts[2], "--tcp");
        assert_eq!(parts[3], "127.0.0.1:1234");
        fs::remove_dir_all(root).unwrap();
    }
}
