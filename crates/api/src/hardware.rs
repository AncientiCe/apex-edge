//! The hub's configured receipt printer and cash drawer.
//!
//! Printing from the hub is optional and off by default: most deployments have the POS
//! fetch the document and print it itself. When a printer *is* configured, every
//! operation is instrumented, because "the receipt did not come out" is otherwise
//! unanswerable after the fact.

use std::sync::Arc;
use std::time::Instant;

use apex_edge_adapters_hardware::{
    CaptureSink, EscPosEncoder, HardwareError, ReceiptDevice, StarLineModeEncoder,
    Tcp9100Transport, TransportPrinter,
};
use apex_edge_metrics::{
    HARDWARE_OPERATIONS_TOTAL, HARDWARE_OPERATION_DURATION_SECONDS, OUTCOME_ERROR, OUTCOME_SUCCESS,
};
use serde_json::Value;

/// Character columns on the paper. 32 is 58mm paper in the standard font, which is the
/// narrowest common receipt roll — anything wider still fits.
const DEFAULT_PAPER_WIDTH: usize = 32;

/// When the drawer should spring open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawerKickPolicy {
    /// Cash sales only. Opening the till on a card sale is a security problem and trains
    /// staff to ignore the drawer.
    #[default]
    CashSales,
    Always,
    Never,
}

impl DrawerKickPolicy {
    fn from_env_value(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "always" => DrawerKickPolicy::Always,
            "never" | "off" | "0" => DrawerKickPolicy::Never,
            _ => DrawerKickPolicy::CashSales,
        }
    }
}

#[derive(Clone, Default)]
pub struct HardwareSettings {
    /// `None` means no printer is wired to this hub, which is the default.
    pub device: Option<Arc<dyn ReceiptDevice>>,
    pub paper_width_chars: usize,
    pub drawer: DrawerKickPolicy,
}

impl std::fmt::Debug for HardwareSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareSettings")
            .field("device", &self.device.as_ref().map(|d| d.encoder_name()))
            .field("paper_width_chars", &self.paper_width())
            .field("drawer", &self.drawer)
            .finish()
    }
}

impl HardwareSettings {
    /// Reads the printer configuration.
    ///
    /// - `APEX_EDGE_PRINTER=tcp` plus `APEX_EDGE_PRINTER_ADDRESS=host:9100` for a
    ///   networked printer. Point it at `tools/virtual-printer` to see receipts without
    ///   hardware.
    /// - `APEX_EDGE_PRINTER_DIALECT=star` for Star Line Mode; ESC/POS otherwise.
    /// - `APEX_EDGE_PRINTER_WIDTH` character columns on the paper.
    /// - `APEX_EDGE_DRAWER_KICK=always|never` overrides the cash-only default.
    pub fn from_env() -> Self {
        let width = std::env::var("APEX_EDGE_PRINTER_WIDTH")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v >= 20)
            .unwrap_or(DEFAULT_PAPER_WIDTH);
        let drawer = std::env::var("APEX_EDGE_DRAWER_KICK")
            .map(|raw| DrawerKickPolicy::from_env_value(&raw))
            .unwrap_or_default();

        let device = match std::env::var("APEX_EDGE_PRINTER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "tcp" | "tcp9100" | "network" => {
                let address = std::env::var("APEX_EDGE_PRINTER_ADDRESS")
                    .unwrap_or_else(|_| "127.0.0.1:9100".into());
                Some(tcp_device(&address, star_dialect()))
            }
            "raw" | "port" => match std::env::var("APEX_EDGE_PRINTER_PORT_PATH") {
                Ok(path) if !path.trim().is_empty() => Some(raw_port_device(&path, star_dialect())),
                _ => {
                    tracing::error!(
                        "APEX_EDGE_PRINTER=raw needs APEX_EDGE_PRINTER_PORT_PATH; printing disabled"
                    );
                    None
                }
            },
            "" | "none" | "off" => None,
            other => {
                tracing::error!(
                    printer = other,
                    "unknown printer transport; printing disabled"
                );
                None
            }
        };

        Self {
            device,
            paper_width_chars: width,
            drawer,
        }
    }

    /// A printer that records what it was told to print instead of printing it. This is
    /// the CI path and the one POS-level tests assert against.
    pub fn capturing(paper_width_chars: usize) -> (Self, CaptureSink) {
        let sink = CaptureSink::default();
        let settings = Self {
            device: Some(Arc::new(TransportPrinter::new(EscPosEncoder, sink.clone()))),
            paper_width_chars,
            drawer: DrawerKickPolicy::default(),
        };
        (settings, sink)
    }

    /// The same, speaking Star Line Mode.
    pub fn capturing_star(paper_width_chars: usize) -> (Self, CaptureSink) {
        let sink = CaptureSink::default();
        let settings = Self {
            device: Some(Arc::new(TransportPrinter::new(
                StarLineModeEncoder,
                sink.clone(),
            ))),
            paper_width_chars,
            drawer: DrawerKickPolicy::default(),
        };
        (settings, sink)
    }

    pub fn paper_width(&self) -> usize {
        if self.paper_width_chars == 0 {
            DEFAULT_PAPER_WIDTH
        } else {
            self.paper_width_chars
        }
    }

    pub fn encoder_name(&self) -> Option<&'static str> {
        self.device.as_ref().map(|device| device.encoder_name())
    }

    /// Prints a stored receipt payload. `Ok(None)` means no printer is configured, which
    /// is a normal deployment rather than a failure.
    pub fn print_receipt(&self, payload: &Value) -> Result<Option<&'static str>, HardwareError> {
        let Some(device) = self.device.as_ref() else {
            return Ok(None);
        };
        let document = crate::receipt_layout::receipt_document(payload, self.paper_width());
        // "The receipt was wrong" is otherwise unanswerable: the bytes are gone and the
        // paper is with the customer.
        tracing::debug!(receipt = %document.plain_text(), "printing receipt");
        observe(device.encoder_name(), "print_receipt", || {
            device.print_document(&document)
        })?;
        Ok(Some(device.encoder_name()))
    }

    /// Whether a sale should open the drawer.
    pub fn should_kick_drawer(&self, sale_took_cash: bool) -> bool {
        match self.drawer {
            DrawerKickPolicy::Always => true,
            DrawerKickPolicy::CashSales => sale_took_cash,
            DrawerKickPolicy::Never => false,
        }
    }

    /// Opens the drawer. `Ok(false)` means there was nothing to open, either because no
    /// printer is configured or because policy forbids it.
    pub fn kick_drawer(&self) -> Result<bool, HardwareError> {
        if self.drawer == DrawerKickPolicy::Never {
            return Ok(false);
        }
        let Some(device) = self.device.as_ref() else {
            return Ok(false);
        };
        observe(device.encoder_name(), "kick_drawer", || {
            device.kick_drawer()
        })?;
        Ok(true)
    }
}

fn star_dialect() -> bool {
    std::env::var("APEX_EDGE_PRINTER_DIALECT")
        .map(|v| v.trim().eq_ignore_ascii_case("star"))
        .unwrap_or(false)
}

fn tcp_device(address: &str, star: bool) -> Arc<dyn ReceiptDevice> {
    let transport = Tcp9100Transport::new(address);
    if star {
        Arc::new(TransportPrinter::new(StarLineModeEncoder, transport))
    } else {
        Arc::new(TransportPrinter::new(EscPosEncoder, transport))
    }
}

fn raw_port_device(path: &str, star: bool) -> Arc<dyn ReceiptDevice> {
    let transport = apex_edge_adapters_hardware::RawPortTransport::new(path);
    if star {
        Arc::new(TransportPrinter::new(StarLineModeEncoder, transport))
    } else {
        Arc::new(TransportPrinter::new(EscPosEncoder, transport))
    }
}

/// Every hardware operation is counted and timed under one `device`/`operation` pair, so
/// a printer that has quietly stopped working is visible on a dashboard.
fn observe<T>(
    device: &'static str,
    operation: &'static str,
    call: impl FnOnce() -> Result<T, HardwareError>,
) -> Result<T, HardwareError> {
    let start = Instant::now();
    let result = call();
    let outcome = if result.is_ok() {
        OUTCOME_SUCCESS
    } else {
        OUTCOME_ERROR
    };
    metrics::counter!(HARDWARE_OPERATIONS_TOTAL, "device" => device, "operation" => operation, "outcome" => outcome)
        .increment(1);
    metrics::histogram!(HARDWARE_OPERATION_DURATION_SECONDS, "device" => device, "operation" => operation)
        .record(start.elapsed().as_secs_f64());
    if let Err(ref error) = result {
        tracing::error!(device, operation, %error, "hardware operation failed");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_with_no_printer_reports_nothing_printed_rather_than_an_error() {
        let settings = HardwareSettings::default();

        assert_eq!(
            settings
                .print_receipt(&serde_json::json!({}))
                .expect("print"),
            None
        );
        assert!(!settings.kick_drawer().expect("kick"));
    }

    #[test]
    fn the_drawer_policy_decides_cash_only_by_default() {
        let (cash_only, _) = HardwareSettings::capturing(32);
        assert!(cash_only.should_kick_drawer(true));
        assert!(!cash_only.should_kick_drawer(false));

        let always = HardwareSettings {
            drawer: DrawerKickPolicy::Always,
            ..HardwareSettings::capturing(32).0
        };
        assert!(always.should_kick_drawer(false));

        let never = HardwareSettings {
            drawer: DrawerKickPolicy::Never,
            ..HardwareSettings::capturing(32).0
        };
        assert!(!never.should_kick_drawer(true));
        assert!(!never.kick_drawer().expect("kick"), "policy must win");
    }

    #[test]
    fn an_unreadable_paper_width_falls_back_to_the_narrowest_roll() {
        let settings = HardwareSettings {
            paper_width_chars: 0,
            ..HardwareSettings::default()
        };

        assert_eq!(settings.paper_width(), DEFAULT_PAPER_WIDTH);
    }

    #[test]
    fn the_drawer_setting_is_read_from_its_documented_spellings() {
        assert_eq!(
            DrawerKickPolicy::from_env_value("Always"),
            DrawerKickPolicy::Always
        );
        assert_eq!(
            DrawerKickPolicy::from_env_value("never"),
            DrawerKickPolicy::Never
        );
        assert_eq!(
            DrawerKickPolicy::from_env_value("cash"),
            DrawerKickPolicy::CashSales
        );
    }
}
