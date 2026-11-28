//! Workflows: a graph of trigger, session, command, If, Switch and For each nodes.
//! The definition, lenient parsing and validation are pure; files and running come later.
pub mod active;
#[cfg(test)]
mod active_tests;
pub mod board_edit;
#[cfg(test)]
mod board_edit_tests;
pub mod board_model;
#[cfg(test)]
mod board_model_tests;
pub mod cli;
#[cfg(test)]
mod cli_tests;
#[cfg(test)]
mod cloud_target_tests;
pub mod command;
pub mod condition;
#[cfg(test)]
mod data_passing_tests;
pub mod engine;
#[cfg(test)]
mod engine_tests;
pub mod event;
#[cfg(test)]
mod event_node_tests;
pub mod event_run;
#[cfg(test)]
mod event_run_tests;
#[cfg(test)]
mod event_tests;
#[cfg(test)]
mod fixtures;
pub mod global;
#[cfg(test)]
mod global_tests;
pub mod guide;
pub mod mention;
#[cfg(test)]
mod mention_tests;
pub mod model;
pub mod models;
pub mod ordered;
pub mod output;
pub mod parse;
pub mod preflight;
#[cfg(test)]
mod preflight_tests;
pub mod queue;
#[cfg(test)]
mod queue_tests;
pub mod report;
#[cfg(test)]
mod report_tests;
pub mod run_graph_model;
#[cfg(test)]
mod run_graph_model_tests;
pub mod run_worktree;
#[cfg(test)]
mod run_worktree_tests;
pub mod schedule;
pub mod session_facts;
#[cfg(test)]
mod session_facts_tests;
pub mod store;
#[cfg(test)]
mod store_tests;
pub mod template;
pub mod templates;
#[cfg(test)]
mod templates_tests;
#[cfg(test)]
mod tests;
pub mod tick;
#[cfg(test)]
mod tick_tests;
#[cfg(test)]
mod ui_fixtures;
pub mod validate;
pub mod variables;
#[cfg(test)]
mod variables_tests;
#[cfg(test)]
mod workspace_tests;
pub mod yaml;
pub(crate) mod yaml_emit;
pub(crate) mod yaml_read;
#[cfg(test)]
mod yaml_tests;
