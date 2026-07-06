//! Application layer providing the main Knx library interface.
//!
//! This module contains the main Knx struct that serves as the primary
//! entry point for the library, managing connections and telegram
//! processing, and providing a high-level API for KNX/IP communication.

pub mod callbacks;
pub mod knx;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod thread_safety_test;

pub use callbacks::{CallbackHandle, ConnectionState, EventHandler};
pub use knx::{ConnectionControlEvent, Knx, KnxBuilder, KnxState};
