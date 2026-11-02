use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use intimatr::{
    command::{
        Command, CommandError, CommandExecution, CommandExecutor, CommandResult, PostAction,
    },
    config::{RpcConfig, RpcTransport},
    rpc::{PostActionHandler, RpcClient, RpcEndpoint, start_server},
    speed_commands::{SpeedPreviewExecutor, SpeedRequest},
    speed_config::SpeedSettings,
    speed_hotkeys::SpeedHotkeyConfig,
};

struct TestExecutor;

impl CommandExecutor for TestExecutor {
    fn execute(&self, command: Command) -> Result<CommandExecution, CommandError> {
        match command {
            Command::Ping => Ok(CommandExecution {
                result: CommandResult::Pong,
                post_action: None,
            }),
            Command::Shutdown => Ok(CommandExecution {
                result: CommandResult::ShutdownAccepted,
                post_action: Some(PostAction::Shutdown),
            }),
            _ => Err(CommandError::NotImplemented("test command")),
        }
    }
}

fn post_action_recorder(flag: Arc<AtomicBool>) -> PostActionHandler {
    Arc::new(move |action| match action {
        PostAction::Shutdown => flag.store(true, Ordering::Release),
    })
}

#[test]
fn loopback_tcp_client_and_server_round_trip_commands() {
    let config = RpcConfig {
        transport: RpcTransport::Tcp,
        bind: "127.0.0.1:0".to_owned(),
        ..RpcConfig::default()
    };
    let shutdown_seen = Arc::new(AtomicBool::new(false));
    let mut server = start_server(
        config.clone(),
        Arc::new(TestExecutor),
        post_action_recorder(Arc::clone(&shutdown_seen)),
    )
    .expect("TCP RPC server should start");

    let address = match server.endpoint() {
        RpcEndpoint::Tcp(address) => *address,
        other => panic!("unexpected endpoint: {other:?}"),
    };
    let mut client =
        RpcClient::connect_tcp(address, config.max_request_bytes, config.max_response_bytes)
            .expect("TCP client should connect");

    assert_eq!(client.call(Command::Ping).unwrap(), CommandResult::Pong);
    assert_eq!(
        client.call(Command::Shutdown).unwrap(),
        CommandResult::ShutdownAccepted
    );

    for _ in 0..100 {
        if shutdown_seen.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(shutdown_seen.load(Ordering::Acquire));
    server.stop().expect("server should stop cleanly");
}

#[test]
fn speed_preview_rpc_round_trips_and_shares_in_memory_state() {
    let base: Arc<dyn CommandExecutor> = Arc::new(TestExecutor);
    let shared = Arc::new(
        SpeedPreviewExecutor::with_profile(
            base,
            &SpeedSettings {
                enabled: true,
                ..SpeedSettings::default()
            },
            &SpeedHotkeyConfig::default(),
            "rpc-fixture",
        )
        .unwrap(),
    );
    let executor: Arc<dyn CommandExecutor> = shared.clone();
    let config = RpcConfig {
        transport: RpcTransport::Tcp,
        bind: "127.0.0.1:0".to_owned(),
        ..RpcConfig::default()
    };
    let mut server = start_server(
        config.clone(),
        executor,
        post_action_recorder(Arc::new(AtomicBool::new(false))),
    )
    .unwrap();
    let address = match server.endpoint() {
        RpcEndpoint::Tcp(address) => *address,
        other => panic!("expected loopback TCP endpoint, got {other:?}"),
    };
    let mut client =
        RpcClient::connect_tcp(address, config.max_request_bytes, config.max_response_bytes)
            .unwrap();

    let first = client
        .call(Command::Speed {
            request: SpeedRequest::Status,
        })
        .unwrap();
    let CommandResult::Speed { status: first } = first else {
        panic!("expected speed preview status");
    };
    assert!(first.configured);
    assert_eq!(first.preview_multiplier, 1.0);
    assert!(!first.clock_applied_to_game);

    let changed = client
        .call(Command::Speed {
            request: SpeedRequest::Increase,
        })
        .unwrap();
    let CommandResult::Speed { status: changed } = changed else {
        panic!("expected updated preview speed");
    };
    assert_eq!(changed.preview_multiplier, 1.5);
    assert!(!changed.clock_applied_to_game);
    let local = shared
        .execute(Command::Speed {
            request: SpeedRequest::Status,
        })
        .unwrap();
    let CommandResult::Speed { status: local } = local.result else {
        panic!("expected shared coordinator status");
    };
    assert_eq!(local.preview_multiplier, changed.preview_multiplier);
    assert!(
        client
            .call(Command::Speed {
                request: SpeedRequest::SelectPreset { multiplier: 1.25 },
            })
            .is_err()
    );

    server.stop().unwrap();
}

#[cfg(windows)]
#[test]
fn windows_named_pipe_client_and_server_round_trip() {
    let config = RpcConfig {
        transport: RpcTransport::NamedPipe,
        pipe_name: format!(
            "intimatr-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),
        ..RpcConfig::default()
    };
    let shutdown_seen = Arc::new(AtomicBool::new(false));
    let mut server = start_server(
        config.clone(),
        Arc::new(TestExecutor),
        post_action_recorder(shutdown_seen),
    )
    .expect("named-pipe RPC server should start");

    let pipe = match server.endpoint() {
        RpcEndpoint::NamedPipe(pipe) => pipe.clone(),
        other => panic!("unexpected endpoint: {other:?}"),
    };
    let mut client =
        RpcClient::connect_named_pipe(&pipe, config.max_request_bytes, config.max_response_bytes)
            .expect("named-pipe client should connect");

    assert_eq!(client.call(Command::Ping).unwrap(), CommandResult::Pong);
    server.stop().expect("server should stop cleanly");
}
