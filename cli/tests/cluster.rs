//! The `solana-rpc-client` cluster without a network (its mock sender), URL redaction on real
//! client errors and preflight logs (ported from metalbook-cli), and the argument checks.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_macros,
    missing_docs
)]

use std::collections::HashMap;

use base64::Engine;
use serde_json::json;
use solana_keypair::Keypair;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::{Error as ClientError, ErrorKind};
use solana_rpc_client_api::request::{RpcError, RpcRequest, RpcResponseErrorData};
use solana_signer::Signer;
use varswap_cli::rpc::{client_error, redact, Cluster};
use varswap_cli::{create_window_ix, init_config_ix, parse_feed_id, submit, CliError, Rpc};
use varswap_sdk::pda::WindowKey;
use varswap_sdk::Address;

const PROGRAM: Address = Address::new_from_array([0x4d; 32]);

fn init_ix(admin: &Keypair) -> solana_instruction::Instruction {
    init_config_ix(&PROGRAM, &admin.pubkey(), &Address::new_from_array([9; 32]))
}

#[test]
fn test_cluster_mock_signs_sends_and_reads() {
    let admin = Keypair::new();
    let mut ok = Cluster::with_client(RpcClient::new_mock("succeeds"));
    let signature = submit(&mut ok, &[init_ix(&admin)], &admin).unwrap();
    // A base58 ed25519 signature: 64 bytes are 86 to 88 characters.
    assert!((86..=88).contains(&signature.len()), "{signature}");
    assert_eq!(ok.account(&PROGRAM).unwrap(), None);

    // An account as getAccountInfo returns it (base64 data).
    let owner = Address::new_from_array([7; 32]);
    let data = vec![1u8, 2, 3, 250];
    let mut mocks = HashMap::new();
    mocks.insert(
        RpcRequest::GetAccountInfo,
        json!({"context": {"slot": 1}, "value": {
            "lamports": 1, "owner": owner.to_string(), "executable": false, "rentEpoch": 0,
            "space": 4, "data": [base64::engine::general_purpose::STANDARD.encode(&data), "base64"]
        }}),
    );
    let mut with_account = Cluster::with_client(RpcClient::new_mock_with_mocks("succeeds", mocks));
    let info = with_account.account(&PROGRAM).unwrap().unwrap();
    assert_eq!(info.owner, owner);
    assert_eq!(info.data, data);
}

#[test]
fn test_cluster_failed_transaction_is_an_rpc_error() {
    let admin = Keypair::new();
    let mut failing = Cluster::with_client(RpcClient::new_mock("instruction_error"));
    match submit(&mut failing, &[init_ix(&admin)], &admin) {
        Err(CliError::Rpc(message)) => assert!(
            message.contains("instruction requires an initialized account"),
            "{message}"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_client_error_preflight_failures_carry_the_program_logs() {
    let simulation = serde_json::from_value(json!({
        "err": null,
        "logs": ["Program 4d invoke [1]", "Program 4d failed: custom program error: 0x6c"],
    }))
    .unwrap();
    let e = ClientError::from(ErrorKind::RpcError(RpcError::RpcResponseError {
        code: -32002,
        message: "Transaction simulation failed: custom program error: 0x6c".into(),
        data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
    }));
    let text = client_error(&e, "https://rpc.example").to_string();
    assert!(text.starts_with("RPC: RPC response error -32002"), "{text}");
    assert!(
        text.ends_with(
            "\n  Program 4d invoke [1]\n  Program 4d failed: custom program error: 0x6c"
        ),
        "{text}"
    );
    // Each log line once: the client's own message already renders them (seen on devnet,
    // where every line was printed twice).
    for line in [
        "Program 4d invoke [1]",
        "Program 4d failed: custom program error: 0x6c",
    ] {
        assert_eq!(text.matches(line).count(), 1, "{line} in {text}");
    }
    assert_eq!(text.matches("2 log messages:").count(), 1, "{text}");
}

#[test]
fn test_client_error_echoed_urls_and_keys_in_preflight_logs_are_redacted() {
    let url = "https://rpc.example/v1/TOKENPATH123/?api-key=SECRETKEY99";
    let simulation = serde_json::from_value(json!({
        "err": null,
        "logs": [
            "Program log: provider https://rpc.example/v1/TOKENPATH123/?api-key=SECRETKEY99",
            "Program log: key SECRETKEY99 and token TOKENPATH123 echoed bare",
            "Program 4d failed: custom program error: 0x6c",
        ],
    }))
    .unwrap();
    let e = ClientError::from(ErrorKind::RpcError(RpcError::RpcResponseError {
        code: -32002,
        message: format!("Transaction simulation failed via {url}"),
        data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
    }));
    let err = client_error(&e, url);
    let text = format!("{err} {err:?}");
    for leak in ["SECRETKEY99", "TOKENPATH123", "rpc.example"] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    assert!(text.contains("custom program error: 0x6c"), "{text}");
    assert!(
        text.contains("key <redacted> and token <redacted> echoed bare"),
        "{text}"
    );
}

#[test]
fn test_redact_urls_in_any_form() {
    let url = "https://rpc.example?api-key=SECRET";
    for text in [
        "error sending request for url (https://rpc.example/?api-key=SECRET)",
        "error sending request for url (https://rpc.example?api-key=SECRET): refused",
        "https://rpc.example?api-key=SECRET",
        "see \"http://other.example/SECRET\" and 'https://x/SECRET'",
    ] {
        let clean = redact(text, url);
        assert!(!clean.contains("SECRET"), "{clean}");
        assert!(clean.contains("<rpc url>"), "{clean}");
    }
    assert_eq!(
        redact(
            "error sending request for url (https://a/?k=SECRET): refused",
            ""
        ),
        "error sending request for url (<rpc url>): refused"
    );
    assert_eq!(redact("no url here", "https://a"), "no url here");
}

#[test]
fn test_cluster_real_transport_error_hides_the_url() {
    // Port 9 on the loopback refuses the connection at once; no request leaves the machine.
    let url = "http://127.0.0.1:9/?api-key=SECRET";
    let mut cluster = Cluster::new(url);
    let err = cluster.latest_blockhash().unwrap_err();
    let text = format!("{err} {err:?} {cluster:?}");
    assert!(!text.contains("SECRET"), "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(text.contains("<rpc url>"), "{text}");
}

#[test]
fn test_arguments_checked_before_signing() {
    let payer = Address::new_from_array([1; 32]);
    let key = |start, end, interval| WindowKey {
        feed: [7; 32],
        start,
        end,
        interval,
    };
    assert!(create_window_ix(&PROGRAM, &payer, &key(0, 3_600, 3_600)).is_ok());
    for (s, e, i) in [(10, 10, 1), (10, 5, 1), (0, 3_601, 3_600), (0, 3_600, 0)] {
        assert!(matches!(
            create_window_ix(&PROGRAM, &payer, &key(s, e, i)),
            Err(CliError::Usage(_))
        ));
    }
    let sol = "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
    assert_eq!(parse_feed_id(sol).unwrap()[0], 0xef);
    assert_eq!(parse_feed_id(&format!("0x{sol}")).unwrap()[31], 0x6d);
    for bad in [
        "",
        "0x12",
        &sol[1..],
        &format!("{sol}00"),
        &sol.replace('e', "g"),
    ] {
        assert!(parse_feed_id(bad).is_err(), "{bad}");
    }
}

fn clock_account(owner: &str, unix_timestamp: i64) -> serde_json::Value {
    let mut data = vec![0u8; 40];
    data[32..40].copy_from_slice(&unix_timestamp.to_le_bytes());
    json!({
        "lamports": 1, "owner": owner, "executable": false, "rentEpoch": 0, "space": 40,
        "data": [base64::engine::general_purpose::STANDARD.encode(&data), "base64"],
    })
}

const SYSVAR: &str = "Sysvar1111111111111111111111111111111111111";

fn snapshot_of(value: serde_json::Value) -> Result<varswap_cli::index::Snapshot, CliError> {
    use varswap_cli::index::SeriesCluster;
    let mut mocks = HashMap::new();
    mocks.insert(
        RpcRequest::GetMultipleAccounts,
        json!({"context": {"slot": 77}, "value": value}),
    );
    Cluster::with_client(RpcClient::new_mock_with_mocks("succeeds", mocks)).snapshot(&PROGRAM)
}

#[test]
fn test_snapshot_reads_an_explicit_null_with_its_clock() {
    let s = snapshot_of(json!([null, clock_account(SYSVAR, 1_793_725_260)])).unwrap();
    assert_eq!((s.slot, s.clock), (77, 1_793_725_260));
    assert_eq!(s.account, None);
}

#[test]
fn test_snapshot_reads_an_account_with_its_clock() {
    let owner = Address::new_from_array([7; 32]);
    let account = json!({
        "lamports": 1, "owner": owner.to_string(), "executable": false, "rentEpoch": 0, "space": 4,
        "data": [base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3, 4]), "base64"],
    });
    let s = snapshot_of(json!([account, clock_account(SYSVAR, 5)])).unwrap();
    assert_eq!(s.clock, 5);
    assert_eq!(s.account.unwrap().data, [1, 2, 3, 4]);
}

#[test]
fn test_snapshot_without_a_real_clock_is_an_error_not_a_null() {
    for value in [
        json!([null, null]),
        json!([null]),
        json!([null, clock_account("11111111111111111111111111111111", 5)]),
    ] {
        let e = snapshot_of(value.clone()).expect_err("a snapshot with no clock");
        assert!(matches!(e, CliError::Rpc(_)), "{value}: {e}");
    }
}

#[test]
fn test_transport_errors_are_transient_and_a_nodes_answers_are_not() {
    use solana_rpc_client_api::client_error::{Error, ErrorKind};
    use solana_rpc_client_api::request::{RpcError, RpcResponseErrorData};

    let io = Error::from(ErrorKind::Io(std::io::Error::other("reset")));
    assert!(matches!(
        client_error(&io, "http://x"),
        CliError::RpcTransient(_)
    ));
    let response = |data| {
        Error::from(ErrorKind::RpcError(RpcError::RpcResponseError {
            code: -32005,
            message: "m".to_owned(),
            data,
        }))
    };
    let behind = response(RpcResponseErrorData::NodeUnhealthy {
        num_slots_behind: Some(9),
    });
    assert!(matches!(
        client_error(&behind, "http://x"),
        CliError::RpcTransient(_)
    ));
    assert!(matches!(
        client_error(&response(RpcResponseErrorData::Empty), "http://x"),
        CliError::Rpc(_)
    ));
}
