//! Tests of the Hermes price source with fixtures only (no live calls): parsing a response in
//! the shape of Hermes' `/v2/updates/price/{t}` (`apps/hermes/server/src/api/types.rs`), the
//! request it sends, status handling, the key redaction, and that over a synthetic tape served
//! with Hermes' "first update at or after t" semantics it answers exactly like the reference
//! `TapeSource`. Ported from metalbook-keeper (#17), with its fixture.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::disallowed_macros,
    missing_docs
)]

use std::cell::RefCell;

use varswap_keeper::{
    parse_update, HermesConfig, HermesSource, HttpGet, HttpResponse, KeeperError, PriceSource,
    PrintMsg, TapeSource, UreqHttp, HERMES_API_KEY_VAR, HERMES_URL_VAR,
};

const FEED: [u8; 32] = [
    0x76, 0x5d, 0x2b, 0xa9, 0x06, 0xdb, 0xc3, 0x2c, 0xa1, 0x7c, 0xc1, 0x1f, 0x53, 0x10, 0xa8, 0x9e,
    0x9e, 0xe1, 0xf6, 0x42, 0x05, 0x08, 0xc6, 0x38, 0x61, 0xf2, 0xf8, 0xba, 0x4e, 0xe3, 0x4b, 0xb2,
];

fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn config() -> HermesConfig {
    HermesConfig::from_lookup(|n| (n == HERMES_API_KEY_VAR).then(|| "test-key".to_string()))
        .unwrap()
}

/// A recorded request: URL and headers.
type Request = (String, Vec<(String, String)>);

/// A fake Hermes over a tape: for `/v2/updates/price/{t}` it returns the first message (in
/// publication order) with `publish_time >= t`, as Hermes' `RequestTime::FirstAfter` does.
struct FakeHermes {
    tape: Vec<PrintMsg>,
    now: i64,
    status: Option<u16>,
    seen: RefCell<Vec<Request>>,
}

impl FakeHermes {
    fn new(tape: Vec<PrintMsg>) -> Self {
        Self {
            tape,
            now: i64::MAX,
            status: None,
            seen: RefCell::new(Vec::new()),
        }
    }
}

fn body(m: &PrintMsg) -> String {
    format!(
        r#"{{"binary":{{"encoding":"base64","data":["AAEC"]}},"parsed":[{{"id":"{}","price":{{"price":"{}","conf":"{}","expo":{},"publish_time":{}}},"ema_price":{{"price":"{}","conf":"{}","expo":{},"publish_time":{}}},"metadata":{{"slot":1,"proof_available_time":{},"prev_publish_time":{}}}}}]}}"#,
        hex(&FEED),
        m.price,
        m.conf,
        m.expo,
        m.publish_time,
        m.price,
        m.conf,
        m.expo,
        m.publish_time,
        m.publish_time,
        m.prev_publish_time
    )
}

impl HttpGet for FakeHermes {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
        self.seen
            .borrow_mut()
            .push((url.to_string(), headers.to_vec()));
        if let Some(status) = self.status {
            return Ok(HttpResponse {
                status,
                body: String::new(),
            });
        }
        // Like Hermes, only what is already published (<= now) is served.
        let path = url
            .split("/v2/updates/price/")
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        let found = if path == "latest" {
            self.tape.iter().rev().find(|m| m.publish_time <= self.now)
        } else {
            let t: i64 = path.parse().unwrap();
            self.tape
                .iter()
                .find(|m| m.publish_time >= t && m.publish_time <= self.now)
        };
        Ok(match found {
            Some(m) => HttpResponse {
                status: 200,
                body: body(m),
            },
            None => HttpResponse {
                status: 404,
                body: r#"{"error":"not found"}"#.into(),
            },
        })
    }
}

#[test]
fn test_parse_recorded_shape_fixture() {
    let json = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hermes_xau.json"
    ))
    .unwrap();
    let u = parse_update(&json, &FEED).unwrap();
    assert_eq!(
        u.print,
        PrintMsg {
            price: 4_023_655,
            conf: 125,
            expo: -3,
            publish_time: 1_793_289_597,
            prev_publish_time: 1_793_289_596
        }
    );
    assert_eq!(
        u.update_data,
        b"PNAU\x01\x00\x00\x00\x03\xb8\x01\x00\x00\x00\x04\x0d".to_vec()
    );
    // Wrong feed, bad JSON, wrong encoding, non-integer price, missing prev_publish_time.
    assert!(matches!(
        parse_update(&json, &[0; 32]),
        Err(KeeperError::MalformedResponse { .. })
    ));
    assert!(matches!(
        parse_update("{", &FEED),
        Err(KeeperError::MalformedResponse { .. })
    ));
    assert!(matches!(
        parse_update(&json.replace("base64", "hex"), &FEED),
        Err(KeeperError::MalformedResponse { .. })
    ));
    assert!(matches!(
        parse_update(&json.replace("\"4023655\"", "\"4.0\""), &FEED),
        Err(KeeperError::MalformedResponse { .. })
    ));
    let no_prev = json.replace(", \"prev_publish_time\": 1793289596", "");
    assert_eq!(
        parse_update(&no_prev, &FEED)
            .unwrap()
            .print
            .prev_publish_time,
        0
    );
}

#[test]
fn test_request_shape_and_auth_header() {
    let fake = FakeHermes::new(vec![PrintMsg {
        price: 1,
        conf: 0,
        expo: -3,
        publish_time: 100,
        prev_publish_time: 99,
    }]);
    let source = HermesSource::new(config(), fake);
    source.first_at_or_after(&FEED, 100).unwrap().unwrap();
    // The source owns the fake; inspect through a second instance sharing nothing: rebuild.
    let fake = FakeHermes::new(vec![]);
    let source2 = HermesSource::new(config(), &fake);
    assert_eq!(source2.first_at_or_after(&FEED, 5).unwrap(), None);
    let (url, headers) = fake.seen.borrow()[0].clone();
    assert_eq!(url, format!("https://hermes.pyth.network/v2/updates/price/5?ids%5B%5D={}&encoding=base64&parsed=true", hex(&FEED)));
    assert_eq!(
        headers,
        vec![("Authorization".to_string(), "Bearer test-key".to_string())]
    );
    drop(source);
}

#[test]
fn test_status_handling() {
    for (status, ok) in [(401u16, false), (403, false), (500, false), (404, true)] {
        let mut fake = FakeHermes::new(vec![]);
        fake.status = Some(status);
        let r = HermesSource::new(config(), fake).first_at_or_after(&FEED, 1);
        assert_eq!(r.is_ok(), ok, "{status}: {r:?}");
        if status == 401 {
            assert!(format!("{r:?}").contains("unverified"));
        }
    }
}

/// The tape of the keeper tests' shape: a print every 1 to 3 seconds, a same-second repeat
/// every 7th step, failed aggregations, and a halt.
fn tape() -> Vec<PrintMsg> {
    let mut out = Vec::new();
    let (mut t, mut last_ok, mut px, mut step) = (10_000i64, 9_999i64, 4_000_000i64, 0u64);
    while t < 14_000 {
        step += 1;
        if (11_000..11_400).contains(&t) {
            t = 11_400;
            continue;
        }
        px += ((step * 7_919) % 41) as i64 - 20;
        out.push(PrintMsg {
            price: px,
            conf: 100,
            expo: -3,
            publish_time: t,
            prev_publish_time: last_ok,
        });
        if step % 7 == 0 {
            out.push(PrintMsg {
                price: px + 500,
                conf: 100,
                expo: -3,
                publish_time: t,
                prev_publish_time: t,
            });
        }
        if step % 11 == 0 {
            out.push(PrintMsg {
                price: px - 300,
                conf: 100,
                expo: -3,
                publish_time: last_ok,
                prev_publish_time: last_ok,
            });
        }
        last_ok = t;
        t += 1 + (step % 3) as i64;
    }
    out
}

impl HttpGet for &FakeHermes {
    fn get(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
        (*self).get(url, headers)
    }
}

#[test]
fn test_hermes_source_answers_like_the_tape_reference() {
    let t = tape();
    let reference = TapeSource::new(FEED, t.clone());
    let now = 13_500;
    let mut fake = FakeHermes::new(t);
    fake.now = now;
    let hermes = HermesSource::new(config(), &fake);
    let mut checked = 0;
    let mut found = 0;
    for q in (9_990..13_600).step_by(7) {
        for tol in [0, 3, 10, 600] {
            let want = reference.canonical_at(&FEED, q, tol, now).unwrap();
            assert_eq!(
                hermes.canonical_at(&FEED, q, tol, now).unwrap(),
                want,
                "canonical_at {q} tolerance {tol}"
            );
            found += usize::from(want.is_some());
            checked += 1;
        }
    }
    assert!(checked > 2_000 && found > 1_000, "{checked} {found}");
}

/// A malformed response that echoes the key in mistyped or unexpected fields never puts the
/// key, or any response text, into the error (codex, #17).
#[test]
fn test_errors_never_echo_the_key_or_the_response() {
    let key = "test-key";
    let bodies = [
        format!(
            r#"{{"binary":{{"encoding":"base64","data":["AA=="]}},"parsed":[{{"id":"{0}","price":{{"price":"1","conf":"1","expo":"{key}","publish_time":1}},"metadata":{{}}}}]}}"#,
            hex(&FEED)
        ),
        format!(r#"{{"binary":{{"encoding":"{key}","data":["AA=="]}},"parsed":[]}}"#),
        format!(
            r#"{{"binary":{{"encoding":"base64","data":["{key}!"]}},"parsed":[{{"id":"{0}","price":{{"price":"1","conf":"1","expo":-3,"publish_time":1}},"metadata":{{"prev_publish_time":0}}}}]}}"#,
            hex(&FEED)
        ),
        format!("{key} is not json"),
    ];
    for body in bodies {
        let err = parse_update(&body, &FEED).unwrap_err();
        let shown = format!("{err:?} {err}");
        assert!(!shown.contains(key), "{shown}");
        // And through the source, with the key-redaction backstop.
        struct Echo(String);
        impl HttpGet for Echo {
            fn get(&self, _: &str, _: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
                Ok(HttpResponse {
                    status: 200,
                    body: self.0.clone(),
                })
            }
        }
        let err = HermesSource::new(config(), Echo(body.clone()))
            .first_at_or_after(&FEED, 1)
            .unwrap_err();
        assert!(!format!("{err:?} {err}").contains(key));
    }
}

/// Codex's repro on #17: the key is "test-key" and Hermes answers 200 with the JSON string
/// "test-key". The error is a fixed category; neither its Display, its Debug, nor the line the
/// `hermes` example prints (`hermes: {e}`) contains the key.
#[test]
fn test_reflected_key_never_reaches_display_debug_or_the_example_output() {
    struct Echo;
    impl HttpGet for Echo {
        fn get(&self, _: &str, _: &[(String, String)]) -> Result<HttpResponse, KeeperError> {
            Ok(HttpResponse {
                status: 200,
                body: "\"test-key\"".to_string(),
            })
        }
    }
    let err = HermesSource::new(config(), Echo)
        .first_at_or_after(&FEED, 1)
        .unwrap_err();
    assert_eq!(
        err,
        KeeperError::MalformedResponse {
            status: 200,
            byte_len: 10,
            what: "JSON of the wrong shape"
        }
    );
    for shown in [
        format!("{err}"),
        format!("{err:?}"),
        format!("hermes: {err}"),
    ] {
        assert!(!shown.contains("test-key"), "{shown}");
    }
}

/// Codex's repro on #16: the key is also a path segment of the base URL. Neither the config's
/// nor the source's `Debug` shows it, nor the URL, and a real transport error (nothing listens
/// on port 9) quotes neither the URL, its host, nor the key.
#[test]
fn test_key_in_the_base_url_never_reaches_debug_or_transport_errors() {
    let key = "SYNTHETIC-HERMES-KEY";
    let lookup = |url: &'static str| {
        move |n: &str| match n {
            HERMES_API_KEY_VAR => Some(key.to_string()),
            HERMES_URL_VAR => Some(url.to_string()),
            _ => None,
        }
    };
    let config =
        HermesConfig::from_lookup(lookup("https://gateway.example/SYNTHETIC-HERMES-KEY")).unwrap();
    let source = HermesSource::new(config.clone(), UreqHttp::new());
    for shown in [format!("{config:?}"), format!("{source:?}")] {
        assert!(!shown.contains(key), "{shown}");
        assert!(!shown.contains("gateway.example"), "{shown}");
    }

    let config =
        HermesConfig::from_lookup(lookup("http://127.0.0.1:9/SYNTHETIC-HERMES-KEY/")).unwrap();
    let err = HermesSource::new(config, UreqHttp::new())
        .first_at_or_after(&FEED, 1)
        .unwrap_err();
    assert!(matches!(err, KeeperError::Source(_)), "{err:?}");
    let shown = format!("{err:?} {err}");
    for leak in [key, "127.0.0.1", "http://"] {
        assert!(!shown.contains(leak), "{shown}");
    }
}

/// History is immutable and the key's rate limit is shared: a second query for the same
/// `(feed, t)`, and the update query at a canonical print's own publish time, are answered from
/// the cache. A 429 is `RateLimited`, and nothing is cached for it.
#[test]
fn test_answers_are_cached_and_429_is_rate_limited() {
    use varswap_keeper::UpdateSource;
    let tape = tape();
    let mut fake = FakeHermes::new(tape.clone());
    fake.now = 20_000;
    let hermes = HermesSource::new(config(), &fake);
    let b = 10_500;
    let first = hermes.first_at_or_after(&FEED, b).unwrap().unwrap();
    assert_eq!(hermes.first_at_or_after(&FEED, b).unwrap().unwrap(), first);
    hermes.update(&FEED, first.print.publish_time).unwrap();
    assert_eq!(fake.seen.borrow().len(), 1);

    let mut limited = FakeHermes::new(tape);
    limited.status = Some(429);
    let hermes = HermesSource::new(config(), &limited);
    assert_eq!(
        hermes.first_at_or_after(&FEED, b),
        Err(KeeperError::RateLimited)
    );
    assert_eq!(
        hermes.first_at_or_after(&FEED, b),
        Err(KeeperError::RateLimited)
    );
    assert_eq!(limited.seen.borrow().len(), 2);
}
