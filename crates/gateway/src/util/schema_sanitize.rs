//! JSON Schema sanitizer for tool-use schemas.
//!
//! Some models (notably OpenAI's o3 family) reject tool/function definitions
//! whose `array`-typed parameters lack an `items` field. The JSON Schema
//! spec technically allows `{"type": "array"}` without `items` (any array),
//! but strict validators require it. This module walks a schema and
//! injects a permissive `items` clause wherever one is missing.
//!
//! Pattern ported from BlockRunAI/Franklin `src/mcp/client.ts:53-80`:
//!
//! ```text
//! function sanitizeArrayItems(schema) {
//!   if (schema.type === 'array' && !schema.items) {
//!     schema.items = {};
//!   }
//!   for nested objects/arrays/anyOf/oneOf/allOf, recurse.
//! }
//! ```

use serde_json::{Map, Value};

/// Recursively walk a JSON Schema in-place, ensuring every `array`-typed
/// schema has an `items` field. Missing `items` are replaced with the
/// permissive `{}` (= "any value").
///
/// Recurses into:
/// - `properties.<key>` — object property schemas
/// - `items` — array element schema
/// - `anyOf`, `oneOf`, `allOf` — schema arrays
/// - `$defs`, `definitions` — schema dictionaries
/// - `additionalProperties` — when a schema (not a bool)
pub fn sanitize_array_items(schema: &mut Value) {
    match schema {
        Value::Object(map) => sanitize_object(map),
        Value::Array(items) => {
            for item in items {
                sanitize_array_items(item);
            }
        }
        _ => {}
    }
}

fn sanitize_object(map: &mut Map<String, Value>) {
    // 1. If this object declares `type: "array"` with no `items`, inject `{}`.
    if matches!(map.get("type"), Some(Value::String(t)) if t == "array")
        && !map.contains_key("items")
    {
        map.insert("items".to_string(), Value::Object(Map::new()));
    }

    // 2. Recurse into nested schema-bearing fields.
    for key in [
        "items",
        "additionalProperties",
        "if",
        "then",
        "else",
        "not",
        "contains",
        "propertyNames",
    ] {
        if let Some(child) = map.get_mut(key) {
            sanitize_array_items(child);
        }
    }

    // 3. Recurse into dictionary schemas (`properties`, `$defs`, `definitions`).
    for key in ["properties", "$defs", "definitions", "patternProperties"] {
        if let Some(Value::Object(props)) = map.get_mut(key) {
            for (_, prop_schema) in props.iter_mut() {
                sanitize_array_items(prop_schema);
            }
        }
    }

    // 4. Recurse into schema arrays (`anyOf`, `oneOf`, `allOf`, `prefixItems`).
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(branches)) = map.get_mut(key) {
            for branch in branches.iter_mut() {
                sanitize_array_items(branch);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn array_with_items_unchanged() {
        let mut schema = json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let before = schema.clone();
        sanitize_array_items(&mut schema);
        assert_eq!(schema, before);
    }

    #[test]
    fn array_without_items_gets_empty_items() {
        let mut schema = json!({ "type": "array" });
        sanitize_array_items(&mut schema);
        assert_eq!(schema, json!({ "type": "array", "items": {} }));
    }

    #[test]
    fn nested_object_property_array_sanitized() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "names": { "type": "array" },
                "count": { "type": "number" }
            }
        });
        sanitize_array_items(&mut schema);
        assert_eq!(
            schema["properties"]["names"],
            json!({ "type": "array", "items": {} })
        );
        // Untouched fields are preserved.
        assert_eq!(schema["properties"]["count"], json!({ "type": "number" }));
    }

    #[test]
    fn deeply_nested_array_sanitized() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner_list": { "type": "array" }
                    }
                }
            }
        });
        sanitize_array_items(&mut schema);
        assert_eq!(
            schema["properties"]["outer"]["properties"]["inner_list"],
            json!({ "type": "array", "items": {} })
        );
    }

    #[test]
    fn anyof_branches_sanitized() {
        let mut schema = json!({
            "anyOf": [
                { "type": "array" },
                { "type": "string" }
            ]
        });
        sanitize_array_items(&mut schema);
        assert_eq!(schema["anyOf"][0], json!({ "type": "array", "items": {} }));
        assert_eq!(schema["anyOf"][1], json!({ "type": "string" }));
    }

    #[test]
    fn oneof_branches_sanitized() {
        let mut schema = json!({
            "oneOf": [
                { "type": "array" },
                {
                    "type": "object",
                    "properties": {
                        "tags": { "type": "array" }
                    }
                }
            ]
        });
        sanitize_array_items(&mut schema);
        assert_eq!(schema["oneOf"][0], json!({ "type": "array", "items": {} }));
        assert_eq!(
            schema["oneOf"][1]["properties"]["tags"],
            json!({ "type": "array", "items": {} })
        );
    }

    #[test]
    fn allof_branches_sanitized() {
        let mut schema = json!({
            "allOf": [
                { "type": "array" }
            ]
        });
        sanitize_array_items(&mut schema);
        assert_eq!(schema["allOf"][0], json!({ "type": "array", "items": {} }));
    }

    #[test]
    fn nested_array_of_arrays_sanitized() {
        // An array whose `items` is itself an array-without-items.
        let mut schema = json!({
            "type": "array",
            "items": { "type": "array" }
        });
        sanitize_array_items(&mut schema);
        assert_eq!(schema["items"], json!({ "type": "array", "items": {} }));
    }

    #[test]
    fn defs_dictionary_recursed() {
        let mut schema = json!({
            "$defs": {
                "Tag": { "type": "array" }
            },
            "type": "object"
        });
        sanitize_array_items(&mut schema);
        assert_eq!(
            schema["$defs"]["Tag"],
            json!({ "type": "array", "items": {} })
        );
    }

    #[test]
    fn non_object_root_is_noop() {
        let mut value = json!("not a schema");
        let before = value.clone();
        sanitize_array_items(&mut value);
        assert_eq!(value, before);

        let mut number = json!(42);
        let before_num = number.clone();
        sanitize_array_items(&mut number);
        assert_eq!(number, before_num);
    }

    #[test]
    fn additional_properties_schema_recursed() {
        let mut schema = json!({
            "type": "object",
            "additionalProperties": { "type": "array" }
        });
        sanitize_array_items(&mut schema);
        assert_eq!(
            schema["additionalProperties"],
            json!({ "type": "array", "items": {} })
        );
    }
}
