//! Harness² Workbench server (`h2workbench`): REST API, web UI, job runner, auth, audit.
//!
//! The server is the single owner of jobs, state, authentication and the audit log
//! (ARCHITECTURE.md §3, ADR 0003); the web UI and the MCP server are clients of its REST
//! API. This crate is a library so integration tests can build the real router and server.

#![forbid(unsafe_code)]

pub mod auth;
pub mod cli;
pub mod config;
pub mod db;
pub mod doctor;
pub mod egress;
pub mod error;
pub mod expdirs;
pub mod http;
pub mod jobs;
pub mod metrics;
pub mod ratelimit;
pub mod secrets;
pub mod telemetry;
pub mod util;
