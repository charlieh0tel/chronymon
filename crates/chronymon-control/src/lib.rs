//! A client for chronyd's control protocol, the one chronyc speaks,
//! over chronyd's Unix socket.  Read-only reports only.
//!
//! The reports chronymon needs most -- `ntpdata`, `sourcestats`,
//! `selectdata` -- are the ones chrony does not promise to keep stable,
//! so each reply type is decoded only when it is one this client knows,
//! and any other is an error rather than a guess.

pub mod client;
pub mod error;
#[cfg(test)]
mod fixture_tests;
mod protocol;
pub mod report;
mod wire;
