//! Template rendering: contracted data -> template -> output (ESC/POS, PDF, etc.).

use serde_json::Value;

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
