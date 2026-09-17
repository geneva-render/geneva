use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

use crate::schema::{ACCEPTED_VERSIONS, FORMAT_VERSION, Timeline};

/// Canonical URL of the published schema for the current format version.
pub fn schema_url() -> String {
    format!("https://geneva-render.github.io/schema/geneva-timeline-{FORMAT_VERSION}.schema.json")
}

/// Generates the JSON Schema (draft 2020-12) for the current timeline format.
pub fn json_schema() -> Value {
    let generator = SchemaSettings::draft2020_12().into_generator();
    let mut schema = generator.into_root_schema_for::<Timeline>().to_value();
    let root = schema.as_object_mut().expect("root schema is an object");
    root.insert("$id".to_owned(), Value::String(schema_url()));
    root.insert(
        "description".to_owned(),
        Value::String(format!(
            "Geneva timeline format {FORMAT_VERSION}: a declarative video composition."
        )),
    );
    if let Some(geneva) = root
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|p| p.get_mut("geneva"))
        .and_then(Value::as_object_mut)
    {
        geneva.insert("enum".to_owned(), json!(ACCEPTED_VERSIONS));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_pins_the_version() {
        let s = json_schema();
        // The enum is what the build actually reads, rather than a list
        // written out again here that a format bump would have to keep
        // in step by hand.
        assert_eq!(s["properties"]["geneva"]["enum"], json!(ACCEPTED_VERSIONS));
        // The version this build writes has to be one it reads, and the
        // newest of them.
        assert_eq!(ACCEPTED_VERSIONS.last(), Some(&FORMAT_VERSION));
        assert_eq!(
            s["$schema"],
            json!("https://json-schema.org/draft/2020-12/schema")
        );
        assert!(
            s["$id"]
                .as_str()
                .unwrap()
                .ends_with(&format!("geneva-timeline-{FORMAT_VERSION}.schema.json"))
        );
    }
}
