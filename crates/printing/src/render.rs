//! Template rendering: contracted data -> template -> output (ESC/POS, PDF, etc.).

use serde_json::Value;

/// Render a template by replacing `{{key}}` placeholders with payload values.
///
/// # Examples
///
/// ```
/// use apex_edge_printing::render;
///
/// let out = render("Hello {{name}}", &serde_json::json!({"name": "Apex"})).unwrap();
/// assert_eq!(String::from_utf8_lossy(&out), "Hello Apex");
/// ```
pub fn render(template_body: &str, payload: &Value) -> Result<Vec<u8>, RenderError> {
    let s = render_html(template_body, payload)?;
    Ok(s.into_bytes())
}

/// Render template to a string, supporting `{{key}}` and `{{#each key}}...{{/each}}`.
/// Used for HTML templates (e.g. receipt PDF).
fn value_to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => v.to_string().trim_matches('"').to_string(),
    }
}

/// Render HTML template with `{{key}}` and `{{#each key}}...{{/each}}`. Returns HTML string.
pub fn render_html(template_body: &str, payload: &Value) -> Result<String, RenderError> {
    let mut out = template_body.to_string();

    // Expand {{#each key}}...{{/each}} first
    const EACH_START: &str = "{{#each ";
    const EACH_END: &str = "{{/each}}";
    while let Some(start) = out.find(EACH_START) {
        let key_start = start + EACH_START.len();
        let key_end = out[key_start..]
            .find("}}")
            .ok_or_else(|| RenderError::Template("malformed {{#each}}".into()))?;
        let key = out[key_start..key_start + key_end].trim();
        let block_start = key_start + key_end + 2;
        let block_end = out[block_start..]
            .find(EACH_END)
            .ok_or_else(|| RenderError::Template("missing {{/each}}".into()))?;
        let block = out[block_start..block_start + block_end].to_string();
        let rest_start = block_start + block_end + EACH_END.len();
        let arr = payload
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| RenderError::Template(format!("{{#each {key}}}: not an array")))?;
        let mut replacement = String::new();
        for item in arr {
            let mut block_out = block.clone();
            if let Some(obj) = item.as_object() {
                for (k, v) in obj {
                    let placeholder = format!("{{{{{}}}}}", k);
                    let s = value_to_str(v);
                    block_out = block_out.replace(&placeholder, &s);
                }
            }
            replacement.push_str(&block_out);
        }
        out = format!("{}{}{}", &out[..start], replacement, &out[rest_start..]);
    }

    // Top-level {{key}} replacement
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            let placeholder = format!("{{{{{}}}}}", k);
            let s = value_to_str(v);
            out = out.replace(&placeholder, &s);
        }
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("template error: {0}")]
    Template(String),
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn render_replaces_known_placeholders_and_keeps_unknown() {
        let out = render(
            "Hi {{name}} {{unknown}}",
            &serde_json::json!({"name":"Edge"}),
        )
        .expect("render should succeed");
        assert_eq!(String::from_utf8_lossy(&out), "Hi Edge {{unknown}}");
    }
}
