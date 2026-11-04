//! `fixtures/golden.json` is the contract the TypeScript port is tested
//! against. It must keep matching this crate; regenerate it with
//! `examples/export_golden.rs` only for an intended motion change.

#![cfg(feature = "serde")]

#[path = "common/golden_cases.rs"]
mod golden_cases;

use serde_json::Value;

fn close(path: &str, a: &Value, b: &Value, worst: &mut f64) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let d = (x.as_f64().unwrap() - y.as_f64().unwrap()).abs();
            *worst = worst.max(d);
            assert!(d <= 1e-9, "{path}: {x} vs {y}");
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: length");
            for (i, (x, y)) in x.iter().zip(y).enumerate() {
                close(&format!("{path}[{i}]"), x, y, worst);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: keys");
            for (k, v) in x {
                close(&format!("{path}.{k}"), v, &y[k], worst);
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}

#[test]
fn golden_fixture_matches_the_crate() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/golden.json")).expect("golden json");
    let mut worst = 0.0;
    close("golden", &golden_cases::golden(), &fixture, &mut worst);
}
