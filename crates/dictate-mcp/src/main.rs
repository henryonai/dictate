//! dictate-mcp: lets Claude (Code or Desktop) read the Dictate library over MCP on stdio.
//! `DICTATE_MEETINGS_DIR` points it at another library (see `MeetingLibrary::default_directory`).

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use dictate_core::mcp::{tools, McpServer};
use dictate_core::meetings::MeetingLibrary;

fn main() {
    // Run by hand, the server would sit waiting for JSON-RPC that never comes: say what it is for.
    if io::stdin().is_terminal() {
        // Best effort: a stderr that can't be written to must not keep the server from starting.
        let _ = io::stderr().write_all(help(&executable()).as_bytes());
    }
    let library = MeetingLibrary::new(MeetingLibrary::default_directory());
    McpServer::new(
        "dictate",
        env!("CARGO_PKG_VERSION"),
        tools::INSTRUCTIONS,
        tools::all(library),
    )
    .run();
}

/// What to do with the server, for someone who ran it in a terminal.
fn help(executable: &str) -> String {
    // A path with spaces ("C:\Program Files\...") needs quotes to be pasted into a shell.
    let command = if executable.contains(char::is_whitespace) {
        format!("\"{executable}\"")
    } else {
        executable.to_string()
    };
    format!(
        "dictate-mcp is an MCP server for Claude: it reads JSON-RPC on stdin. Add it with\n  \
         claude mcp add dictate -- {command}\n\
         or in Claude Desktop's config (see the Dictate README).\n"
    )
}

/// The path this program runs from, or the name it was started with.
fn executable() -> String {
    std::env::current_exe()
        .ok()
        .or_else(|| std::env::args_os().next().map(PathBuf::from))
        .map_or_else(
            || "dictate-mcp".to_string(),
            |path| path.display().to_string(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_says_how_to_add_the_server_with_the_executable_path() {
        assert_eq!(
            help("/opt/dictate/dictate-mcp"),
            "dictate-mcp is an MCP server for Claude: it reads JSON-RPC on stdin. Add it with\n  \
             claude mcp add dictate -- /opt/dictate/dictate-mcp\n\
             or in Claude Desktop's config (see the Dictate README).\n"
        );
    }

    #[test]
    fn help_quotes_a_path_with_spaces() {
        let text = help("C:\\Program Files\\Dictate\\dictate-mcp.exe");
        assert!(text.contains("-- \"C:\\Program Files\\Dictate\\dictate-mcp.exe\"\n"));
    }
}
