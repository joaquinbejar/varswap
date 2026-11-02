//! Window records (`docs/PLAN-improvements.md`, I7): every committed record in `docs/windows/`
//! verifies offline (this is the CI step), and the offline negative controls fail, each for its
//! reason. The `--rpc` controls need the cluster; they are recorded in `docs/windows/README.md`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::panic_in_result_fn,
    missing_docs
)]

use serde_json::Value;
use varswap_cli::record::verify_offline;

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/windows")
}

fn records() -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let v = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            (p.file_name().unwrap().to_string_lossy().into_owned(), v)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn demo() -> Value {
    records()
        .into_iter()
        .find(|(n, _)| n.contains("demo"))
        .unwrap()
        .1
}

fn fails(b: &Value, why: &str) {
    let e = verify_offline(b).expect_err("a forged record verified");
    let msg = e.to_string();
    assert!(msg.contains(why), "failed for another reason: {msg}");
}

fn sample_tx_index(b: &Value) -> usize {
    b["history"]["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .position(|t| !t["samples"].as_array().unwrap().is_empty())
        .unwrap()
}

#[test]
fn test_committed_records_verify_offline() {
    let all = records();
    assert!(all.len() >= 2, "docs/windows has {} records", all.len());
    for (name, b) in &all {
        let d = verify_offline(b).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_ne!(d["label"], "unknown", "{name}");
    }
}

#[test]
fn test_edited_bucket_price_in_the_account_fails() {
    let mut b = demo();
    // The price array follows the 144 byte header and the bitmap; flip one byte inside it.
    let mut hex = b["account"]["data_hex"].as_str().unwrap().to_owned();
    let at = 2 * (144 + 8 + 2);
    let c = &hex[at..at + 1];
    let flipped = if c == "0" { "1" } else { "0" };
    hex.replace_range(at..at + 1, flipped);
    b["account"]["data_hex"] = Value::String(hex);
    fails(&b, "sum_sq");
}

#[test]
fn test_deleted_event_fails() {
    let mut b = demo();
    let i = sample_tx_index(&b);
    b["history"]["transactions"][i]["samples"] = Value::Array(vec![]);
    // The account still has the bucket filled: the history no longer covers it, so the label
    // the events give is `unknown`, not the stated one.
    fails(&b, "derived.");
}

#[test]
fn test_edited_event_bounds_or_price_fail() {
    let mut b = demo();
    let i = sample_tx_index(&b);
    let last = b["history"]["transactions"][i]["samples"][0]["last"]
        .as_u64()
        .unwrap();
    b["history"]["transactions"][i]["samples"][0]["last"] = Value::from(last + 1);
    fails(&b, "derived.");

    let mut b = demo();
    let p: i64 = b["history"]["transactions"][i]["samples"][0]["price"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    b["history"]["transactions"][i]["samples"][0]["price"] = Value::String((p + 1).to_string());
    fails(&b, "derived.");
}

#[test]
fn test_forged_label_fails() {
    let mut b = demo();
    assert_eq!(b["derived"]["label"], "complete");
    b["derived"]["label"] = Value::String("compressed".into());
    fails(&b, "derived.label");
    let mut b = demo();
    b["derived"]["realized_var"] = Value::String("0.2".into());
    fails(&b, "derived.realized_var");
}

#[test]
fn test_history_without_create_window_is_unknown() {
    let mut b = demo();
    let txs = b["history"]["transactions"].as_array_mut().unwrap();
    txs.retain(|t| !t["creates_window"].as_bool().unwrap());
    // Claiming the history reaches create_window without listing it is rejected outright.
    fails(&b, "claims to reach create_window");
    // Stated honestly, the label must be `unknown`.
    b["history"]["reaches_create_window"] = Value::Bool(false);
    fails(&b, "the account and events give");
    let d = {
        b["derived"]["label"] = Value::String("unknown".into());
        b["derived"]["runs"] = Value::Null;
        b["derived"]["compressed_runs"] = Value::Null;
        b["derived"]["longest_run"] = Value::Null;
        b["derived"]["unproven"] = Value::String("the history does not reach create_window".into());
        verify_offline(&b).unwrap()
    };
    assert_eq!(d["label"], "unknown");
}

#[test]
fn test_derived_that_is_not_an_object_fails() {
    for bad in [Value::Null, Value::String("x".into()), Value::Array(vec![])] {
        let mut b = demo();
        b["derived"] = bad;
        fails(&b, "derived is missing or not an object");
    }
    let mut b = demo();
    b.as_object_mut().unwrap().remove("derived");
    fails(&b, "derived is missing or not an object");
}

/// A cluster that serves the record's own account and signature list, plus or minus a change.
struct Fake {
    data: Vec<u8>,
    owner: varswap_sdk::Address,
    signatures: Vec<String>,
    unavailable: Vec<String>,
}

impl varswap_cli::record::History for Fake {
    fn signatures(
        &mut self,
        _: &varswap_sdk::Address,
        before: Option<&str>,
    ) -> Result<Vec<String>, varswap_cli::CliError> {
        // One page, then an empty one.
        Ok(if before.is_none() {
            self.signatures.clone()
        } else {
            vec![]
        })
    }

    fn transaction(&mut self, s: &str) -> Result<Option<Value>, varswap_cli::CliError> {
        assert!(
            self.unavailable.iter().any(|u| u == s),
            "fetched {s} before the list check"
        );
        Ok(None)
    }

    fn account_at(
        &mut self,
        _: &varswap_sdk::Address,
    ) -> Result<(u64, Option<varswap_cli::AccountInfo>), varswap_cli::CliError> {
        Ok((
            1,
            Some(varswap_cli::AccountInfo {
                owner: self.owner,
                data: self.data.clone(),
            }),
        ))
    }
}

fn fake(b: &Value) -> Fake {
    let hex = b["account"]["data_hex"].as_str().unwrap();
    let data = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let signatures = b["history"]["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .map(|t| t["signature"].as_str().unwrap().to_owned())
        .collect();
    Fake {
        data,
        owner: b["program_id"].as_str().unwrap().parse().unwrap(),
        signatures,
        unavailable: vec![],
    }
}

#[test]
fn test_rpc_fails_on_an_extra_listed_signature_even_if_unavailable() {
    let b = demo();
    let mut f = fake(&b);
    f.signatures
        .insert(0, "extraSignatureNotInTheBundle".to_owned());
    f.unavailable
        .push("extraSignatureNotInTheBundle".to_owned());
    let e = varswap_cli::record::verify_rpc(&mut f, &b)
        .unwrap_err()
        .to_string();
    assert!(e.contains("missing from the bundle"), "{e}");
}

#[test]
fn test_rpc_fails_when_a_listed_transaction_cannot_be_fetched() {
    let b = demo();
    let mut f = fake(&b);
    f.unavailable = f.signatures.clone();
    let e = varswap_cli::record::verify_rpc(&mut f, &b)
        .unwrap_err()
        .to_string();
    assert!(e.contains("completeness unproven"), "{e}");
}
