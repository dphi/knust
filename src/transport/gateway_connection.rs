//! A normal-gateway-shaped API that both single- and multi-gateway
//! connections can implement.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::protocol::telegram::Telegram;

/// Send telegrams to, and subscribe to telegrams from, a bus's connection —
/// regardless of whether it's backed by one gateway or several (HA) with
/// failover between them.
#[async_trait]
pub trait GatewayConnection: Send + Sync {
    /// Queue a telegram for outbound delivery.
    async fn send(&self, telegram: Telegram) -> Result<()>;

    /// Subscribe to the merged stream of incoming telegrams.
    fn subscribe(&self) -> broadcast::Receiver<Telegram>;

    /// Signal shutdown.
    fn shutdown(&self);

    /// Check whether shutdown has been requested.
    fn is_shutdown(&self) -> bool;
}
