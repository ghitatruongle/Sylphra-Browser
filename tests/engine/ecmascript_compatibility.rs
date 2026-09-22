use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use sylphra::javascript::{JsvEngine, JsvPromiseState, JsvValue};

#[derive(Debug, Deserialize)]
struct CorpusCase {
    name: String,
    source: String,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    kind: String,
    value: Option<serde_json::Value>,
    contains: Option<String>,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ecmascript")
        .join("ecmascript-corpus.json")
}

#[test]
fn ecmascript_original_compatibility_corpus() {
    let raw = fs::read_to_string(corpus_path()).expect("read ecmascript corpus");
    let cases: Vec<CorpusCase> = serde_json::from_str(&raw).expect("parse ecmascript corpus");
    assert!(
        cases.len() >= 10,
        "compatibility corpus unexpectedly shrank"
    );

    for case in cases {
        let mut engine = JsvEngine::new();
        let outcome = engine.eval(&case.source);
        match case.expected.kind.as_str() {
            "error" => {
                let error = outcome.unwrap_err();
                let fragment = case.expected.contains.expect("error fragment");
                assert!(error.contains(&fragment), "{}: {error}", case.name);
            }
            "number" => assert_eq!(
                outcome.unwrap().as_number(),
                case.expected.value.and_then(|value| value.as_f64()),
                "{}",
                case.name
            ),
            "string" => assert_eq!(
                outcome.unwrap().as_string(),
                case.expected
                    .value
                    .as_ref()
                    .and_then(|value| value.as_str()),
                "{}",
                case.name
            ),
            "boolean" => assert_eq!(
                outcome.unwrap().as_boolean(),
                case.expected.value.and_then(|value| value.as_bool()),
                "{}",
                case.name
            ),
            "promise-number" => {
                let JsvValue::Promise(promise) = outcome.unwrap() else {
                    panic!("{}: expected Promise", case.name);
                };
                let expected = case.expected.value.and_then(|value| value.as_f64());
                assert!(
                    matches!(&*promise.borrow(), JsvPromiseState::Fulfilled(JsvValue::Number(value)) if Some(*value) == expected),
                    "{}: Promise did not contain expected number",
                    case.name
                );
            }
            "promise-string" => {
                let JsvValue::Promise(promise) = outcome.unwrap() else {
                    panic!("{}: expected Promise", case.name);
                };
                let expected = case
                    .expected
                    .value
                    .as_ref()
                    .and_then(|value| value.as_str());
                assert!(
                    matches!(&*promise.borrow(), JsvPromiseState::Fulfilled(JsvValue::String(value)) if Some(value.as_str()) == expected),
                    "{}: Promise did not contain expected string",
                    case.name
                );
            }
            kind => panic!("{}: unsupported expected kind {kind}", case.name),
        }
    }
}
