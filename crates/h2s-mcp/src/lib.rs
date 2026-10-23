//! `h2studio-mcp`: the Harness² Studio MCP server.
//!
//! Every harness2 operation the Studio web UI offers, exposed to AI agents as MCP tools,
//! resources and prompts (`docs/ARCHITECTURE.md` §13). The server is a **thin client** of the
//! Studio server's REST API (`/api/v1`): it holds no state and no extra power. Each call is
//! made with an API token — in stdio mode the one it was started with, over streamable HTTP
//! the caller's own bearer token — so the Studio server's role checks and audit log apply to
//! every client individually.

pub mod cli;
pub mod client;
pub mod http;
pub mod limits;
pub mod server;
pub mod token;

mod descriptions;
mod errors;
mod output;
mod params;
mod prompts;
mod resources;
mod schema;
mod sse;
mod wait;

pub use cli::{Cli, TransportKind, run};
pub use client::{ApiClient, ClientOptions};
pub use limits::WaitLimiter;
pub use server::{Credentials, H2sMcp};
pub use token::Token;
