//! Tool input schemas and strict argument parsing.
//!
//! Schemas are derived with `schemars` from the input types (which embed the `h2s_api` wire
//! types), with every subschema inlined (no `$ref`, for clients that do not resolve
//! references) and `additionalProperties: false` on every object. Arguments are checked
//! against the schema before deserializing: an unknown or misspelled argument is an error,
//! never silently ignored — a typo in `budget_usd` or `max_tasks` must not start an uncapped
//! or full-size run.

use std::{
    any::TypeId,
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use rmcp::{
    ErrorData,
    handler::server::{common::FromContextPart, tool::ToolCallContext},
    model::JsonObject,
};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// rmcp renders `invalid_params` errors that start with this prefix as tool errors the model
/// can read (instead of an opaque protocol error).
const ARGUMENT_ERROR_PREFIX: &str = "failed to deserialize parameters:";

/// A tool's input type.
pub(crate) trait ToolInput: DeserializeOwned + JsonSchema + Send + 'static {
    /// Adjust the generated schema: bounds the server enforces, descriptions for fields the
    /// wire types leave undocumented.
    fn patch(_schema: &mut Value) {}
}

/// The input schema of `T`, generated once per type.
pub(crate) fn input_schema<T: ToolInput>() -> Arc<JsonObject> {
    static CACHE: OnceLock<Mutex<HashMap<TypeId, Arc<JsonObject>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut map = cache.lock().unwrap_or_else(PoisonError::into_inner);
    map.entry(TypeId::of::<T>())
        .or_insert_with(|| Arc::new(generate::<T>()))
        .clone()
}

fn generate<T: ToolInput>() -> JsonObject {
    let settings = SchemaSettings::draft2020_12().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    });
    let root = settings.into_generator().into_root_schema_for::<T>();
    let mut value = serde_json::to_value(root).unwrap_or_else(|_| json!({}));
    T::patch(&mut value);
    close_objects(&mut value);
    let mut object = match value {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    };
    // The wrapper type's own name and doc are noise to a model; MCP wants an object root.
    object.remove("title");
    object.remove("description");
    object.insert("type".into(), json!("object"));
    object.entry("properties").or_insert_with(|| json!({}));
    object
        .entry("additionalProperties")
        .or_insert(Value::Bool(false));
    object
}

/// Set `additionalProperties: false` on every object schema that lists its properties.
fn close_objects(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let mut has_properties = false;
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        has_properties = true;
        for property in properties.values_mut() {
            close_objects(property);
        }
    }
    if has_properties {
        object
            .entry("additionalProperties")
            .or_insert(Value::Bool(false));
    }
    for key in ["items", "not"] {
        if let Some(sub) = object.get_mut(key) {
            close_objects(sub);
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(Value::Array(list)) = object.get_mut(key) {
            for sub in list {
                close_objects(sub);
            }
        }
    }
}

/// The schema node of the property at `path` (through `properties`, and `items` for `[]`).
pub(crate) fn node<'a>(schema: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    let mut current = schema;
    for step in path {
        current = if *step == "[]" {
            current.get_mut("items")?
        } else {
            current.get_mut("properties")?.get_mut(*step)?
        };
    }
    Some(current)
}

/// Set inclusive integer bounds on the property at `path`.
pub(crate) fn bounds(schema: &mut Value, path: &[&str], min: u64, max: u64) {
    if let Some(Value::Object(object)) = node(schema, path) {
        object.insert("minimum".into(), json!(min));
        object.insert("maximum".into(), json!(max));
    }
}

/// Describe the property at `path` unless the wire type already does.
pub(crate) fn describe(schema: &mut Value, path: &[&str], text: &str) {
    if let Some(Value::Object(object)) = node(schema, path) {
        object
            .entry("description")
            .or_insert_with(|| Value::String(text.to_owned()));
    }
}

/// An argument that is not in the schema.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Unknown {
    /// Dotted path, e.g. `options.max_task`.
    pub(crate) path: String,
    /// The names allowed at that level, sorted.
    pub(crate) allowed: Vec<String>,
}

/// Every argument in `value` that `schema` does not declare.
pub(crate) fn unknown_arguments(schema: &JsonObject, value: &Value) -> Vec<Unknown> {
    let mut out = Vec::new();
    walk(schema, value, "", &mut out);
    out
}

fn walk(schema: &JsonObject, value: &Value, path: &str, out: &mut Vec<Unknown>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = schema.get("properties") {
                for (key, sub_value) in map {
                    let sub_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    match properties.get(key).and_then(Value::as_object) {
                        Some(sub_schema) => walk(sub_schema, sub_value, &sub_path, out),
                        None => {
                            let mut allowed: Vec<String> = properties.keys().cloned().collect();
                            allowed.sort();
                            out.push(Unknown {
                                path: sub_path,
                                allowed,
                            });
                        }
                    }
                }
                return;
            }
            // An optional object or a choice of object shapes: accept the best match.
            for key in ["anyOf", "oneOf"] {
                let Some(Value::Array(alternatives)) = schema.get(key) else {
                    continue;
                };
                let mut best: Option<Vec<Unknown>> = None;
                for alternative in alternatives.iter().filter_map(Value::as_object) {
                    if !alternative.contains_key("properties") {
                        continue;
                    }
                    let mut found = Vec::new();
                    walk(alternative, value, path, &mut found);
                    if found.is_empty() {
                        return;
                    }
                    if best.as_ref().is_none_or(|b| found.len() < b.len()) {
                        best = Some(found);
                    }
                }
                if let Some(best) = best {
                    out.extend(best);
                }
                return;
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items").and_then(Value::as_object) {
                for (i, item) in items.iter().enumerate() {
                    walk(item_schema, item, &format!("{path}[{i}]"), out);
                }
            }
        }
        _ => {}
    }
}

fn unknown_message(unknown: &[Unknown]) -> String {
    let mut msg = String::from("unknown argument");
    if unknown.len() > 1 {
        msg.push('s');
    }
    let names: Vec<String> = unknown.iter().map(|u| format!("`{}`", u.path)).collect();
    msg.push(' ');
    msg.push_str(&names.join(", "));
    if let Some(first) = unknown.first() {
        msg.push_str(&format!(
            " (allowed here: {}). Check the spelling against the tool's input schema.",
            first.allowed.join(", ")
        ));
    }
    msg
}

/// Tool arguments, checked against the input schema of `T` and deserialized.
pub(crate) struct Args<T>(pub(crate) T);

impl<S, T: ToolInput> FromContextPart<ToolCallContext<'_, S>> for Args<T> {
    fn from_context_part(context: &mut ToolCallContext<'_, S>) -> Result<Self, ErrorData> {
        let value = Value::Object(context.arguments.take().unwrap_or_default());
        let schema = input_schema::<T>();
        let unknown = unknown_arguments(&schema, &value);
        if !unknown.is_empty() {
            return Err(ErrorData::invalid_params(
                format!("{ARGUMENT_ERROR_PREFIX} {}", unknown_message(&unknown)),
                None,
            ));
        }
        serde_json::from_value(value)
            .map(Args)
            .map_err(|e| ErrorData::invalid_params(format!("{ARGUMENT_ERROR_PREFIX} {e}"), None))
    }
}

#[cfg(test)]
mod tests {
    use schemars::JsonSchema;
    use serde::Deserialize;

    use super::*;

    // Only their schemas are used.
    #[allow(dead_code)]
    #[derive(Deserialize, JsonSchema)]
    struct Inner {
        /// A count.
        count: u32,
    }

    #[allow(dead_code)]
    #[derive(Deserialize, JsonSchema)]
    struct Probe {
        #[serde(flatten)]
        cell: h2s_api::CheckParams,
        inner: Option<Inner>,
        list: Vec<Inner>,
        #[serde(default)]
        wait_secs: u32,
    }

    impl ToolInput for Probe {
        fn patch(schema: &mut Value) {
            bounds(schema, &["wait_secs"], 0, 600);
            describe(schema, &["wait_secs"], "Seconds.");
            describe(
                schema,
                &["profile"],
                "never used: the wire type documents it",
            );
        }
    }

    #[test]
    fn schemas_are_inlined_closed_and_patched() {
        let schema = input_schema::<Probe>();
        let text = serde_json::to_string(&*schema).unwrap();
        assert!(!text.contains("$ref"), "{text}");
        assert!(!text.contains("$defs"), "{text}");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["cell"]["additionalProperties"], false,
            "nested objects are closed too"
        );
        assert_eq!(schema["properties"]["wait_secs"]["maximum"], 600);
        assert_eq!(schema["properties"]["wait_secs"]["description"], "Seconds.");
        assert_ne!(
            schema["properties"]["profile"]["description"],
            "never used: the wire type documents it"
        );
        assert!(schema.get("title").is_none());
        // Cached: the same Arc comes back.
        assert!(Arc::ptr_eq(&schema, &input_schema::<Probe>()));
    }

    #[test]
    fn unknown_arguments_are_found_at_any_depth() {
        let schema = input_schema::<Probe>();
        let args = json!({
            "cell": {"bench": "lab", "subset_typo": "x"},
            "inner": {"count": 1, "cuont": 2},
            "list": [{"count": 1}, {"extra": true}],
            "wait": 5
        });
        let found: Vec<String> = unknown_arguments(&schema, &args)
            .into_iter()
            .map(|u| u.path)
            .collect();
        assert_eq!(
            found,
            vec!["cell.subset_typo", "inner.cuont", "list[1].extra", "wait"]
        );
        let ok = json!({"cell": {"bench": "lab"}, "inner": null, "list": [], "wait_secs": 1});
        assert!(unknown_arguments(&schema, &ok).is_empty());
    }

    #[test]
    fn unknown_message_lists_allowed_names() {
        let schema = input_schema::<Probe>();
        let found = unknown_arguments(&schema, &json!({"wait": 1}));
        let msg = unknown_message(&found);
        assert!(
            msg.starts_with(
                "unknown argument `wait` (allowed here: cell, inner, list, profile, wait_secs)"
            ),
            "{msg}"
        );
    }
}
