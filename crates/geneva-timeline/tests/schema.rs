//! The published JSON Schema must agree with the parser on the examples.

use geneva_timeline::{json_schema, load};

#[test]
fn examples_validate_against_the_json_schema() {
    let schema = json_schema();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    for name in ["solid", "shapes", "overlay"] {
        let text = std::fs::read_to_string(format!("../../examples/{name}.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:#?}");
        assert!(load(&text).is_ok());
    }
}

#[test]
fn schema_rejects_what_the_parser_rejects() {
    let schema = json_schema();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let bad = serde_json::json!({
        "geneva": "0.1",
        "output": { "width": 16, "height": 16, "fps": 30 },
        "layers": [ { "clips": [ { "source": { "kind": "solid", "color": "#fff" }, "opacty": 1 } ] } ]
    });
    assert!(!validator.is_valid(&bad));
    assert!(load(&bad.to_string()).diagnostics[0].code == "E101");
}

#[test]
fn checked_in_schema_is_current() {
    let expected = serde_json::to_string_pretty(&json_schema()).unwrap() + "\n";
    let path = "../../schema/geneva-timeline-0.1.schema.json";
    match std::fs::read_to_string(path) {
        Ok(actual) if actual == expected => {}
        _ => {
            if std::env::var_os("GENEVA_UPDATE_SCHEMA").is_some() {
                std::fs::write(path, expected).unwrap();
            } else {
                panic!(
                    "{path} is out of date; run GENEVA_UPDATE_SCHEMA=1 cargo test -p geneva-timeline"
                );
            }
        }
    }
}
