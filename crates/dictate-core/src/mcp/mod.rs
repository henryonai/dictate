//! The MCP server Dictate offers to AI clients: protocol over stdio, tools supplied by the caller.

pub mod server;

pub use server::{McpServer, Tool, ToolError, ToolFn, LEGACY_VERSIONS, MODERN_VERSIONS};
