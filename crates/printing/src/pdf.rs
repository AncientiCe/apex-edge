//! Fast in-process HTML-to-PDF rendering for local receipt generation.

use std::sync::OnceLock;
use std::time::Instant;

fn finalize_timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("APEX_EDGE_PROFILE_FINALIZE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

fn log_finalize_timing(event: &str, fields: &[(&str, String)]) {
    if !finalize_timing_enabled() {
        return;
    }
    let suffix = if fields.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    eprintln!("[ApexEdge][PDF] {event}{suffix}");
}

fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();

    for ch in html.chars() {
        if in_tag {
            if ch == '>' {
                let raw_tag = tag_buf.as_str();
                let tag = raw_tag
                    .trim()
                    .trim_start_matches('/')
                    .trim_end_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();

                let is_block = matches!(
                    tag.as_str(),
                    "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                );
                let is_break = tag == "br";
                if (is_block || is_break) && !out.ends_with('\n') {
                    out.push('\n');
                }
                in_tag = false;
                tag_buf.clear();
            } else {
                tag_buf.push(ch);
            }
            continue;
        }

        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        out.push(ch);
    }

    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn wrap_lines(text: &str, max_chars: usize) -> Vec<String> {
    let normalized = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let mut lines = Vec::new();

    for line in normalized {
        let mut current = String::new();
        for word in line.split_whitespace() {
            let needs_space = !current.is_empty();
            let projected =
                current.chars().count() + word.chars().count() + usize::from(needs_space);
            if projected > max_chars && !current.is_empty() {
                lines.push(current);
                current = word.to_string();
            } else {
                if needs_space {
                    current.push(' ');
                }
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::from(" "));
    }

    lines
}

fn escape_pdf_text(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn write_obj(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, id: usize, body: &str) {
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
}

/// Render an HTML string to PDF bytes without spawning a browser.
pub fn html_to_pdf(html: &str) -> Result<Vec<u8>, PdfError> {
    let total_started_at = Instant::now();
    let render_started_at = Instant::now();
    let plain_text = html_to_text(html);
    let lines = wrap_lines(&plain_text, 90);
    let mut stream = String::from("BT\n/F1 11 Tf\n14 TL\n50 792 Td\n");
    for (index, line) in lines.iter().enumerate() {
        let escaped = escape_pdf_text(line);
        if index == 0 {
            stream.push_str(&format!("({escaped}) Tj\n"));
        } else {
            stream.push_str(&format!("T*\n({escaped}) Tj\n"));
        }
    }
    stream.push_str("ET\n");
    let stream_bytes = stream.into_bytes();
    log_finalize_timing(
        "render_done",
        &[(
            "elapsed_ms",
            render_started_at.elapsed().as_millis().to_string(),
        )],
    );

    let mut pdf = Vec::with_capacity(1024 + stream_bytes.len());
    pdf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let mut offsets = vec![0usize];
    write_obj(
        &mut pdf,
        &mut offsets,
        1,
        "<< /Type /Catalog /Pages 2 0 R >>",
    );
    write_obj(
        &mut pdf,
        &mut offsets,
        2,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    );
    write_obj(
        &mut pdf,
        &mut offsets,
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    );
    write_obj(
        &mut pdf,
        &mut offsets,
        4,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );

    offsets.push(pdf.len());
    pdf.extend_from_slice(
        format!("5 0 obj\n<< /Length {} >>\nstream\n", stream_bytes.len()).as_bytes(),
    );
    pdf.extend_from_slice(&stream_bytes);
    pdf.extend_from_slice(b"endstream\nendobj\n");

    let xref_start = pdf.len();
    let object_count = offsets.len();
    pdf.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF\n"
        )
        .as_bytes(),
    );

    log_finalize_timing(
        "total_done",
        &[
            (
                "elapsed_ms",
                total_started_at.elapsed().as_millis().to_string(),
            ),
            ("pdf_size", pdf.len().to_string()),
        ],
    );

    Ok(pdf)
}

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("pdf render: {0}")]
    Render(String),
}
