use crate::InputSignalsInfo;

pub(super) fn format_input_mapping_names(inputs_info: &InputSignalsInfo) -> String {
    let mut names: Vec<&str> = inputs_info.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

pub(super) fn missing_input_mapping_names(
    inputs_info: &InputSignalsInfo,
    inputs_set: &[bool],
) -> Vec<String> {
    let mut names: Vec<String> = inputs_info
        .iter()
        .filter_map(|(name, &(offset, len))| {
            let is_missing =
                (offset..offset + len).any(|idx| !inputs_set.get(idx).copied().unwrap_or(false));
            is_missing.then(|| name.clone())
        })
        .collect();
    names.sort_unstable();
    names
}

pub(super) fn format_missing_input_mapping_message(names: &[String]) -> String {
    if names.len() == 1 {
        format!("missing input signal {}", names[0])
    } else {
        format!("missing input signals {}", names.join(", "))
    }
}

pub(super) fn json_value_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
