//! Reproducible window records (`docs/PLAN-improvements.md`, I7).
//!
//! A record is a JSON bundle: the window account's bytes at a finalized slot, every transaction
//! that touched the window back to its `create_window` (signature, slot, success and, for each
//! successful `sample` instruction on this window, its event
//! `varswap:sample first last publish_time price expo` and instruction position), and the
//! fields derived from them: coverage, `sum_sq`, the realized variance, the runs and the
//! quality label.
//!
//! Labels: `complete` (every run is one bucket), `compressed` (some run spans several buckets:
//! an outage compressed into one return), `degraded` (no realized variance) and `unknown` (the
//! history does not reach `create_window`, or its events do not reconcile with the account).
//! Reconciliation: the successful `sample` events must partition the filled buckets exactly
//! (each filled bucket in one run, no run over an empty bucket, no overlap), and each run's
//! price and exponent must equal the stored bucket prices.
//!
//! [`verify_offline`] checks a bundle against itself: the account bytes decode to a valid
//! window, `sum_sq` and the realized variance recompute from its prices with `varswap-core`,
//! and every derived field (label included) is what the bytes and the listed events give. That
//! is consistency only: it authenticates nothing against the chain, and a consistent forgery
//! (an edited `publish_time` that keeps the run and the prices) passes it. [`verify_rpc`] adds
//! the chain: the account and every listed transaction are fetched again at finalized
//! commitment and must match, and no successful transaction of the window may be missing from
//! the list.

use serde_json::{json, Value};
use varswap_core::{LnKernel, ReturnKernel, SumSq, Window, WindowState};
use varswap_sdk::Address;

use crate::{AccountInfo, CliError};

/// The bundle format version.
pub const RECORD_VERSION: u64 = 1;
const CREATE_WINDOW_TAG: u8 = 2;
const SAMPLE_TAG: u8 = 3;
const EVENT_PREFIX: &str = "Program log: varswap:sample ";

/// One `sample` event, as the program logged it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEvent {
    /// Position of the `sample` instruction in its transaction.
    pub instruction: u32,
    /// First bucket of the run.
    pub first: u32,
    /// Last bucket of the run.
    pub last: u32,
    /// The print's publish time.
    pub publish_time: i64,
    /// Price mantissa.
    pub price: i64,
    /// Exponent.
    pub expo: i32,
}

/// One transaction that touched the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxRecord {
    /// Its first signature.
    pub signature: String,
    /// Its slot.
    pub slot: u64,
    /// Its block time, if the cluster has it.
    pub block_time: Option<i64>,
    /// Whether it succeeded.
    pub success: bool,
    /// Whether it ran `create_window` for this window (successfully).
    pub creates_window: bool,
    /// The events of its successful `sample` instructions on this window.
    pub samples: Vec<SampleEvent>,
}

/// What a record needs from a cluster, at finalized commitment.
pub trait History {
    /// Signatures of transactions that touched `address`, newest first, before `before`.
    ///
    /// # Errors
    ///
    /// The cluster cannot answer.
    fn signatures(
        &mut self,
        address: &Address,
        before: Option<&str>,
    ) -> Result<Vec<String>, CliError>;

    /// `getTransaction` (`json` encoding), or `None` if the cluster does not have it.
    ///
    /// # Errors
    ///
    /// The cluster cannot answer.
    fn transaction(&mut self, signature: &str) -> Result<Option<Value>, CliError>;

    /// The account and the slot it was read at.
    ///
    /// # Errors
    ///
    /// The cluster cannot answer.
    fn account_at(&mut self, address: &Address) -> Result<(u64, Option<AccountInfo>), CliError>;
}

fn bad(what: impl Into<String>) -> CliError {
    CliError::Record(what.into())
}

/// Base58 decoding (Bitcoin alphabet), for instruction data.
fn base58(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut out: Vec<u8> = Vec::new();
    for c in s.bytes() {
        let digit = ALPHABET.iter().position(|a| *a == c)?;
        let mut carry = u32::try_from(digit).ok()?;
        for b in out.iter_mut().rev() {
            let v = u32::from(*b).checked_mul(58)?.checked_add(carry)?;
            *b = u8::try_from(v & 0xff).ok()?;
            carry = v >> 8;
        }
        while carry > 0 {
            out.insert(0, u8::try_from(carry & 0xff).ok()?);
            carry >>= 8;
        }
    }
    let zeros = s.bytes().take_while(|c| *c == b'1').count();
    let mut full = vec![0u8; zeros];
    full.extend(out);
    Some(full)
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i.checked_add(2)?)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect()
}

/// Parses `first=.. last=.. publish_time=.. price=.. expo=..`.
fn parse_event(rest: &str, instruction: u32) -> Option<SampleEvent> {
    let mut first = None;
    let mut last = None;
    let mut publish_time = None;
    let mut price = None;
    let mut expo = None;
    for field in rest.split_whitespace() {
        let (k, v) = field.split_once('=')?;
        match k {
            "first" => first = v.parse().ok(),
            "last" => last = v.parse().ok(),
            "publish_time" => publish_time = v.parse().ok(),
            "price" => price = v.parse().ok(),
            "expo" => expo = v.parse().ok(),
            _ => return None,
        }
    }
    Some(SampleEvent {
        instruction,
        first: first?,
        last: last?,
        publish_time: publish_time?,
        price: price?,
        expo: expo?,
    })
}

/// Reads one `getTransaction` answer: which top-level instructions are this program's
/// `create_window` or `sample` on this window, and the events those `sample`s logged.
///
/// # Errors
///
/// `CliError::Record` if the answer does not have the expected shape.
pub fn parse_transaction(
    v: &Value,
    program_id: &Address,
    window: &Address,
) -> Result<TxRecord, CliError> {
    let signature = v
        .pointer("/transaction/signatures/0")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("transaction without a signature"))?
        .to_owned();
    let slot = v
        .get("slot")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("transaction without a slot"))?;
    let block_time = v.get("blockTime").and_then(Value::as_i64);
    let meta = v
        .get("meta")
        .ok_or_else(|| bad("transaction without meta"))?;
    let success = meta.get("err").is_some_and(Value::is_null);

    let mut keys: Vec<String> = v
        .pointer("/transaction/message/accountKeys")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("transaction without account keys"))?
        .iter()
        .filter_map(|k| k.as_str().map(str::to_owned))
        .collect();
    for part in ["writable", "readonly"] {
        if let Some(extra) = meta
            .pointer(&format!("/loadedAddresses/{part}"))
            .and_then(Value::as_array)
        {
            keys.extend(extra.iter().filter_map(|k| k.as_str().map(str::to_owned)));
        }
    }
    let key = |i: &Value| {
        i.as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| keys.get(i))
    };
    let program = program_id.to_string();
    let window_s = window.to_string();

    let mut creates_window = false;
    let mut sample_ixs: Vec<u32> = Vec::new();
    let instructions = v
        .pointer("/transaction/message/instructions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("transaction without instructions"))?;
    for (i, ix) in instructions.iter().enumerate() {
        let i = u32::try_from(i).map_err(|_| bad("too many instructions"))?;
        if ix.get("programIdIndex").and_then(key) != Some(&program) {
            continue;
        }
        let data = ix
            .get("data")
            .and_then(Value::as_str)
            .and_then(base58)
            .ok_or_else(|| bad("instruction data is not base58"))?;
        let first_account = ix
            .get("accounts")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(key);
        let on_window = first_account == Some(&window_s);
        match data.first().copied() {
            Some(CREATE_WINDOW_TAG) => {
                // create_window's accounts are (payer, config, feed config, window, …): the
                // window is not first, so look for it anywhere in the instruction.
                let touches = ix
                    .get("accounts")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|k| key(k) == Some(&window_s)));
                creates_window |= touches && success;
            }
            Some(SAMPLE_TAG) if on_window => sample_ixs.push(i),
            _ => {}
        }
    }

    let mut samples = Vec::new();
    if success && !sample_ixs.is_empty() {
        let logs = meta
            .get("logMessages")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("successful transaction without logs"))?;
        let top_invoke = format!("Program {program} invoke [1]");
        let mut top: Option<u32> = None;
        let mut current_is_ours = false;
        for line in logs.iter().filter_map(Value::as_str) {
            if line.starts_with("Program ") && line.ends_with(" invoke [1]") {
                top = Some(top.map_or(Ok(0), |t| {
                    t.checked_add(1).ok_or_else(|| bad("too many instructions"))
                })?);
                current_is_ours = line == top_invoke;
                continue;
            }
            if let (Some(rest), true, Some(t)) =
                (line.strip_prefix(EVENT_PREFIX), current_is_ours, top)
            {
                if sample_ixs.contains(&t) {
                    samples
                        .push(parse_event(rest, t).ok_or_else(|| bad("malformed sample event"))?);
                }
            }
        }
        if samples.len() != sample_ixs.len() {
            return Err(bad(format!(
                "{signature}: {} sample instruction(s) on the window, {} event(s)",
                sample_ixs.len(),
                samples.len()
            )));
        }
    }
    Ok(TxRecord {
        signature,
        slot,
        block_time,
        success,
        creates_window,
        samples,
    })
}

/// The fields a record derives from the account bytes and the events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    /// Window state.
    pub state: WindowState,
    /// Buckets.
    pub buckets: u32,
    /// Filled buckets.
    pub filled: u32,
    /// `sum_sq`, raw, recomputed from the stored prices (equal to the stored one).
    pub sum_sq_raw: u128,
    /// Realized variance, raw (`Finalized` only), recomputed (equal to the stored one).
    pub realized_var_raw: Option<u128>,
    /// Runs from the events (`None` when the history is unproven).
    pub runs: Option<Vec<(u32, u32)>>,
    /// Why the history is unproven, if it is.
    pub unproven: Option<String>,
    /// The quality label.
    pub label: &'static str,
}

/// Recomputes everything from the account bytes and the transaction records.
///
/// # Errors
///
/// `CliError::Record` if the bytes are not a valid window, or the stored `sum_sq` or realized
/// variance differ from the ones its prices give (a corrupt or edited account).
pub fn derive(
    data: &[u8],
    records: &[TxRecord],
    history_reaches_creation: bool,
) -> Result<Derived, CliError> {
    let mut copy = data.to_vec();
    let w = Window::load(&mut copy).map_err(|e| bad(format!("not a valid window: {e:?}")))?;
    let spec = *w.spec();
    let state = w.state().map_err(|e| bad(format!("{e:?}")))?;
    let buckets = spec.buckets().map_err(|e| bad(format!("{e:?}")))?;
    let mut prices = Vec::new();
    for k in 0..buckets {
        prices.push(w.price(k).map_err(|e| bad(format!("{e:?}")))?);
    }
    let filled = u32::try_from(prices.iter().filter(|p| p.is_some()).count())
        .map_err(|_| bad("bucket count"))?;

    let mut sum = SumSq::from_raw(0);
    for pair in prices.windows(2) {
        if let [Some(a), Some(b)] = pair {
            let term = LnKernel
                .squared_log_return(*a, *b)
                .map_err(|e| bad(format!("{e:?}")))?;
            sum = sum.checked_add(term).map_err(|e| bad(format!("{e:?}")))?;
        }
    }
    if sum != w.sum_sq() {
        return Err(bad(
            "the stored sum_sq is not the sum of its prices' squared returns",
        ));
    }
    let realized_var_raw = match state {
        WindowState::Finalized => {
            let span = spec.span_seconds().map_err(|e| bad(format!("{e:?}")))?;
            let v = LnKernel
                .annualize(sum, span)
                .map_err(|e| bad(format!("{e:?}")))?;
            if Some(v) != w.realized_var().map_err(|e| bad(format!("{e:?}")))? {
                return Err(bad(
                    "the stored realized variance is not its sum_sq annualized",
                ));
            }
            Some(v)
        }
        _ => None,
    };

    let reconciled = if history_reaches_creation {
        reconcile(&prices, records)
    } else {
        Err("the history does not reach create_window".to_owned())
    };
    let (runs, unproven) = match reconciled {
        Ok(runs) => (Some(runs), None),
        Err(e) => (None, Some(e)),
    };
    let label = match (&runs, state) {
        (None, _) => "unknown",
        (Some(_), WindowState::Degraded) => "degraded",
        (Some(r), _) if r.iter().all(|(a, b)| a == b) => "complete",
        (Some(_), _) => "compressed",
    };
    Ok(Derived {
        state,
        buckets,
        filled,
        sum_sq_raw: sum.raw(),
        realized_var_raw,
        runs,
        unproven,
        label,
    })
}

/// The runs of the successful `sample` events, if they partition the filled buckets exactly
/// and carry the stored prices.
fn reconcile(
    prices: &[Option<varswap_core::StoredPrice>],
    records: &[TxRecord],
) -> Result<Vec<(u32, u32)>, String> {
    let mut covered = vec![false; prices.len()];
    let mut runs = Vec::new();
    for r in records.iter().filter(|r| r.success) {
        for e in &r.samples {
            if e.first > e.last {
                return Err(format!(
                    "{}: run {}..{} is reversed",
                    r.signature, e.first, e.last
                ));
            }
            for k in e.first..=e.last {
                let i = usize::try_from(k).map_err(|_| "bucket index".to_owned())?;
                let stored = (*prices
                    .get(i)
                    .ok_or_else(|| format!("{}: bucket {k} is off the grid", r.signature))?)
                .ok_or_else(|| format!("{}: bucket {k} is empty in the account", r.signature))?;
                if stored.price() != e.price || i32::from(stored.exponent()) != e.expo {
                    return Err(format!("{}: bucket {k} holds another price", r.signature));
                }
                let c = covered
                    .get_mut(i)
                    .ok_or_else(|| "bucket index".to_owned())?;
                if *c {
                    return Err(format!("bucket {k} is in two runs"));
                }
                *c = true;
            }
            runs.push((e.first, e.last));
        }
    }
    for (k, (p, c)) in prices.iter().zip(&covered).enumerate() {
        if p.is_some() && !c {
            return Err(format!(
                "bucket {k} is filled but no listed event covers it"
            ));
        }
    }
    runs.sort_unstable();
    Ok(runs)
}

fn state_name(s: WindowState) -> &'static str {
    match s {
        WindowState::Pending => "pending",
        WindowState::Sampling => "sampling",
        WindowState::Finalized => "finalized",
        WindowState::Degraded => "degraded",
    }
}

fn derived_json(d: &Derived) -> Value {
    let compressed: Vec<&(u32, u32)> = d.runs.iter().flatten().filter(|(a, b)| b > a).collect();
    let longest = d.runs.as_ref().and_then(|r| {
        r.iter()
            .map(|(a, b)| b.saturating_sub(*a).saturating_add(1))
            .max()
    });
    json!({
        "state": state_name(d.state),
        "buckets": d.buckets,
        "filled": d.filled,
        "coverage": format!("{}/{}", d.filled, d.buckets),
        "sum_sq_raw": d.sum_sq_raw.to_string(),
        "realized_var_raw": d.realized_var_raw.map(|v| v.to_string()),
        "realized_var": d.realized_var_raw.map(crate::format_var),
        "runs": d.runs.as_ref().map(|r| r.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()),
        "compressed_runs": d.runs.as_ref().map(|_| compressed.len()),
        "longest_run": longest,
        "unproven": d.unproven,
        "label": d.label,
    })
}

fn record_json(r: &TxRecord) -> Value {
    json!({
        "signature": r.signature,
        "slot": r.slot,
        "block_time": r.block_time,
        "success": r.success,
        "creates_window": r.creates_window,
        "samples": r.samples.iter().map(|e| json!({
            "instruction": e.instruction, "first": e.first, "last": e.last,
            "publish_time": e.publish_time, "price": e.price.to_string(), "expo": e.expo,
        })).collect::<Vec<_>>(),
    })
}

fn record_from_json(v: &Value) -> Option<TxRecord> {
    let samples = v
        .get("samples")?
        .as_array()?
        .iter()
        .map(|e| {
            Some(SampleEvent {
                instruction: u32::try_from(e.get("instruction")?.as_u64()?).ok()?,
                first: u32::try_from(e.get("first")?.as_u64()?).ok()?,
                last: u32::try_from(e.get("last")?.as_u64()?).ok()?,
                publish_time: e.get("publish_time")?.as_i64()?,
                price: e.get("price")?.as_str()?.parse().ok()?,
                expo: i32::try_from(e.get("expo")?.as_i64()?).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(TxRecord {
        signature: v.get("signature")?.as_str()?.to_owned(),
        slot: v.get("slot")?.as_u64()?,
        block_time: v.get("block_time").and_then(Value::as_i64),
        success: v.get("success")?.as_bool()?,
        creates_window: v.get("creates_window")?.as_bool()?,
        samples,
    })
}

/// Every transaction of `window`, oldest first, and whether the list reaches its
/// `create_window`.
fn history<H: History>(
    h: &mut H,
    program_id: &Address,
    window: &Address,
) -> Result<(Vec<TxRecord>, bool), CliError> {
    let mut signatures = Vec::new();
    let mut before: Option<String> = None;
    loop {
        let page = h.signatures(window, before.as_deref())?;
        let Some(last) = page.last().cloned() else {
            break;
        };
        signatures.extend(page);
        before = Some(last);
    }
    let mut records = Vec::new();
    let mut missing = false;
    for s in &signatures {
        match h.transaction(s)? {
            Some(v) => records.push(parse_transaction(&v, program_id, window)?),
            None => missing = true,
        }
    }
    records.sort_by(|a, b| {
        a.slot
            .cmp(&b.slot)
            .then_with(|| a.signature.cmp(&b.signature))
    });
    let reaches = !missing && records.iter().any(|r| r.creates_window);
    Ok((records, reaches))
}

/// Builds the record of `window`.
///
/// # Errors
///
/// The cluster cannot answer, the account is missing or not this program's, or a transaction
/// cannot be read.
pub fn export<H: History>(
    h: &mut H,
    program_id: &Address,
    window: &Address,
    cluster: &str,
) -> Result<Value, CliError> {
    let (slot, account) = h.account_at(window)?;
    let account = account.ok_or_else(|| bad("window account missing"))?;
    if account.owner != *program_id {
        return Err(bad("the window is not owned by the program"));
    }
    let (records, reaches) = history(h, program_id, window)?;
    let derived = derive(&account.data, &records, reaches)?;
    Ok(json!({
        "version": RECORD_VERSION,
        "cluster": cluster,
        "program_id": program_id.to_string(),
        "window": window.to_string(),
        "account": { "slot": slot, "owner": account.owner.to_string(), "data_hex": hex(&account.data) },
        "history": { "reaches_create_window": reaches, "transactions": records.iter().map(record_json).collect::<Vec<_>>() },
        "derived": derived_json(&derived),
    }))
}

/// The parts of a bundle.
struct Parsed {
    program_id: Address,
    window: Address,
    data: Vec<u8>,
    records: Vec<TxRecord>,
    reaches: bool,
}

fn parse_bundle(b: &Value) -> Result<Parsed, CliError> {
    if b.get("version").and_then(Value::as_u64) != Some(RECORD_VERSION) {
        return Err(bad("unknown record version"));
    }
    let addr = |p: &str| {
        b.pointer(p)
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<Address>().ok())
            .ok_or_else(|| bad(format!("{p} is not an address")))
    };
    let program_id = addr("/program_id")?;
    let window = addr("/window")?;
    if addr("/account/owner")? != program_id {
        return Err(bad("the account is not owned by the program"));
    }
    let data = b
        .pointer("/account/data_hex")
        .and_then(Value::as_str)
        .and_then(unhex)
        .ok_or_else(|| bad("account data is not hex"))?;
    let records = b
        .pointer("/history/transactions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no transaction list"))?
        .iter()
        .map(|r| record_from_json(r).ok_or_else(|| bad("malformed transaction record")))
        .collect::<Result<Vec<_>, _>>()?;
    let reaches = b
        .pointer("/history/reaches_create_window")
        .and_then(Value::as_bool)
        .ok_or_else(|| bad("history.reaches_create_window missing"))?;
    if reaches && !records.iter().any(|r| r.creates_window) {
        return Err(bad("history claims to reach create_window and lists none"));
    }
    Ok(Parsed {
        program_id,
        window,
        data,
        records,
        reaches,
    })
}

/// Offline check: the bundle is consistent with itself (see the module note: consistency
/// only, no chain authentication). Returns the derived fields.
///
/// # Errors
///
/// `CliError::Record` naming the first inconsistency.
pub fn verify_offline(b: &Value) -> Result<Value, CliError> {
    let p = parse_bundle(b)?;
    let derived = derived_json(&derive(&p.data, &p.records, p.reaches)?);
    let stated = b.get("derived").ok_or_else(|| bad("no derived fields"))?;
    if let (Some(want), Some(got)) = (derived.as_object(), stated.as_object()) {
        for (k, v) in want {
            if got.get(k) != Some(v) {
                return Err(bad(format!(
                    "derived.{k} is {}, the account and events give {v}",
                    got.get(k).unwrap_or(&Value::Null)
                )));
            }
        }
        if got.len() != want.len() {
            return Err(bad("derived has extra fields"));
        }
    }
    Ok(derived)
}

/// Online check: [`verify_offline`], then the chain at finalized commitment: the account bytes
/// are the bundle's, every listed transaction is there with the same record, and the cluster
/// lists no other transaction of the window.
///
/// # Errors
///
/// `CliError::Record` naming the first difference, or the cluster's error.
pub fn verify_rpc<H: History>(h: &mut H, b: &Value) -> Result<Value, CliError> {
    let derived = verify_offline(b)?;
    let p = parse_bundle(b)?;
    let (_, account) = h.account_at(&p.window)?;
    let account = account.ok_or_else(|| bad("the window account is gone"))?;
    if account.owner != p.program_id || account.data != p.data {
        return Err(bad("the account on chain differs from the bundle's bytes"));
    }
    for r in &p.records {
        let v = h
            .transaction(&r.signature)?
            .ok_or_else(|| bad(format!("{}: not on the cluster", r.signature)))?;
        let fresh = parse_transaction(&v, &p.program_id, &p.window)?;
        if &fresh != r {
            return Err(bad(format!("{}: differs from the cluster's", r.signature)));
        }
    }
    let (chain, _) = history(h, &p.program_id, &p.window)?;
    for c in &chain {
        if !p.records.iter().any(|r| r.signature == c.signature) {
            return Err(bad(format!(
                "{}: on the cluster, missing from the bundle",
                c.signature
            )));
        }
    }
    Ok(derived)
}

/// One markdown table row for `docs/windows/README.md`.
///
/// # Errors
///
/// As [`verify_offline`].
pub fn table_row(b: &Value, file: &str) -> Result<String, CliError> {
    let d = verify_offline(b)?;
    let s = |k: &str| {
        d.get(k)
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
    };
    Ok(format!(
        "| [`{}`]({file}) | {} | {} | {} | {} | {} |",
        b.get("window").and_then(Value::as_str).unwrap_or(""),
        s("state").unwrap_or_default(),
        s("coverage").unwrap_or_default(),
        s("realized_var").unwrap_or_default(),
        s("label").unwrap_or_default(),
        s("compressed_runs").unwrap_or_default(),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::disallowed_macros,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn test_base58_and_hex() {
        assert_eq!(base58("4"), Some(vec![3]));
        assert_eq!(base58("1"), Some(vec![0]));
        assert_eq!(base58("5R"), Some(vec![1, 0]));
        assert_eq!(base58("0"), None);
        assert_eq!(unhex(&hex(&[0, 1, 255])), Some(vec![0, 1, 255]));
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn test_parse_event() {
        let e = parse_event("first=3 last=5 publish_time=1700 price=123 expo=-8", 2).unwrap();
        assert_eq!(
            (
                e.instruction,
                e.first,
                e.last,
                e.publish_time,
                e.price,
                e.expo
            ),
            (2, 3, 5, 1700, 123, -8)
        );
        assert!(parse_event("first=3 last=5 price=123 expo=-8", 0).is_none());
        assert!(parse_event("first=3 extra=1 last=5 publish_time=1 price=1 expo=1", 0).is_none());
    }
}
