//! Shared logic for Dictate: no UI and no OS-specific code.

pub mod mcp;
pub mod meetings;
pub mod text;

#[cfg(test)]
mod test_fixture;
