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
    let mut out = template_body.to_string();
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            let placeholder = format!("{{{{{}}}}}", k);
            out = out.replace(&placeholder, v.to_string().trim_matches('"'));
        }
    }
    Ok(out.into_bytes())
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
