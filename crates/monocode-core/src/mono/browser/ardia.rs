//! Mono Browser on Ardia: the `ardia` command as the gate's upstream. Design:
//! `docs/superpowers/specs/2026-11-27-ardia-browser.md`.
//!
//! * [`runner`]: the one place a process starts, behind [`runner::Runner`] so tests replay the
//!   recorded replies in `fixtures/`.
//! * [`platform`]: finding, starting, quitting and hiding the Ardia application, behind a trait.
//! * [`controller`]: process-wide state (one Ardia per machine): readiness, on-demand launch,
//!   idle quit, who started Ardia, the tabs zptah opened.
//! * [`upstream`]: [`upstream::ArdiaUpstream`], the gate's backend per Mono.
pub mod controller;
pub mod platform;
pub mod runner;
pub mod shell;
pub mod upstream;

pub use controller::{
    controller, ArdiaProfile, Controller, IdleOutcome, LiveInfo, Owner, Readiness, Report,
};
pub use upstream::{end_conversation_everywhere, ArdiaUpstream};

#[cfg(test)]
mod tests;
#[cfg(all(test, unix))]
mod unix_tests;
