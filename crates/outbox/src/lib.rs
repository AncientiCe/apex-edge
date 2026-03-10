//! Durable outbox: reliable order submission with retry, backoff, idempotency, DLQ.

pub mod dispatcher;

pub use dispatcher::*;
