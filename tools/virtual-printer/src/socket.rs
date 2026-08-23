//! The port 9100 side: raw bytes in, decoded jobs out.
//!
//! Port 9100 has no protocol. A job is "everything the client sends until it hangs up",
//! which is exactly how a networked receipt printer behaves, and why the printing path
//! can be pointed at this tool without changing a line of ApexEdge.

use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

use crate::JobStore;

/// Accepts print jobs forever. A failed accept is logged rather than fatal: the next
/// retry from the POS should still land.
pub async fn serve(listener: TcpListener, jobs: JobStore) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let jobs = jobs.clone();
                tokio::spawn(async move {
                    match receive(stream, jobs).await {
                        Ok(Some(id)) => tracing::info!(job = id, %peer, "print job received"),
                        Ok(None) => tracing::debug!(%peer, "connection carried no bytes"),
                        Err(error) => tracing::warn!(%peer, %error, "print job failed"),
                    }
                });
            }
            Err(error) => tracing::warn!(%error, "accept failed"),
        }
    }
}

/// Reads one job and records it. Returns the job id, or `None` for a connection that
/// carried nothing — printers get probed by monitoring tools, and an empty probe is not
/// a receipt.
pub async fn receive(mut stream: TcpStream, jobs: JobStore) -> std::io::Result<Option<u64>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(jobs.record(&bytes).id))
}
