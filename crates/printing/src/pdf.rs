//! HTML to PDF via headless Chrome.

use headless_chrome::Browser;

/// Render HTML string to PDF bytes using headless Chrome.
/// The HTML is loaded via a data URL; no network access is required.
pub fn html_to_pdf(html: &str) -> Result<Vec<u8>, PdfError> {
    let browser = Browser::default().map_err(|e| PdfError::Browser(e.to_string()))?;
    let tab = browser
        .new_tab()
        .map_err(|e| PdfError::Browser(e.to_string()))?;

    let encoded =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, html.as_bytes());
    let data_url = format!("data:text/html;base64,{encoded}");
    tab.navigate_to(&data_url)
        .map_err(|e| PdfError::Browser(e.to_string()))?;

    tab.wait_for_element("body")
        .map_err(|e| PdfError::Browser(e.to_string()))?;

    let pdf_bytes = tab
        .print_to_pdf(None)
        .map_err(|e| PdfError::Browser(e.to_string()))?;

    Ok(pdf_bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("browser: {0}")]
    Browser(String),
}
