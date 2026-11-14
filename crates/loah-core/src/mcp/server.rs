//! A small MCP server over stdio (one JSON-RPC message per line), offering tools only.
//!
//! It serves both protocol eras, so any current Claude client can use it:
//! - modern (2026-07-28): every request names its version in `_meta`; `server/discover` describes the server;
//! - legacy (2025-11-25 and earlier): the client starts with an `initialize` handshake.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde_json::{json, Map, Value};

/// What a tool does with the arguments of a call: the text for the model, or why it can't.
pub type ToolFn = Box<dyn Fn(&Map<String, Value>) -> Result<String, ToolError> + Send + Sync>;

pub struct Tool {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub run: ToolFn,
}

impl Tool {
    pub fn new(
        name: &str,
        title: &str,
        description: &str,
        input_schema: Value,
        run: impl Fn(&Map<String, Value>) -> Result<String, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            input_schema,
            run: Box::new(run),
        }
    }
}

/// A tool call that can't be carried out: the message goes back to the model as the tool's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

pub const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
pub const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The version offered to a legacy client that asks for one we don't know.
const NEWEST_LEGACY: &str = LEGACY_VERSIONS[0];

const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

const INTERNAL_ERROR_REPLY: &str =
    r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"Internal error"}}"#;

pub struct McpServer {
    name: String,
    version: String,
    instructions: String,
    tools: Vec<Tool>,
}

impl McpServer {
    pub fn new(name: &str, version: &str, instructions: &str, tools: Vec<Tool>) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            instructions: instructions.to_string(),
            tools,
        }
    }

    /// Serves stdin until the client closes it.
    pub fn run(&self) {
        self.serve(io::stdin().lock(), io::stdout().lock());
    }

    /// Serves `input` line by line until it ends or fails, writing each reply and a newline to `output`
    /// and flushing after it. Stops when the client is gone (a failed write) instead of panicking.
    pub fn serve<R: BufRead, W: Write>(&self, mut input: R, mut output: W) {
        let mut raw = Vec::new();
        loop {
            raw.clear();
            match input.read_until(b'\n', &mut raw) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            // Bytes that aren't UTF-8 become U+FFFD, so one bad line can't end the session.
            let line = String::from_utf8_lossy(&raw);
            let Some(mut reply) = self.handle_line(&line) else {
                continue;
            };
            reply.push('\n');
            if output
                .write_all(reply.as_bytes())
                .and_then(|()| output.flush())
                .is_err()
            {
                return;
            }
        }
    }

    /// One incoming message (a line) -> the reply line; None for notifications and responses.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        // Like the macOS server, only an object or an array is a message: a bare scalar is a parse error.
        let parsed = match serde_json::from_str::<Value>(line) {
            Ok(value @ (Value::Object(_) | Value::Array(_))) => value,
            _ => return Some(encode(&error(Value::Null, -32700, "Parse error", None))),
        };
        match parsed {
            // 2025-03-26 allowed batches.
            Value::Array(batch) => {
                let replies: Vec<Value> = batch
                    .iter()
                    .filter_map(|m| self.handle_message(m))
                    .collect();
                if replies.is_empty() {
                    None
                } else {
                    Some(encode(&Value::Array(replies)))
                }
            }
            message => self.handle_message(&message).map(|reply| encode(&reply)),
        }
    }

    fn handle_message(&self, message: &Value) -> Option<Value> {
        let Some(m) = message.as_object() else {
            return Some(error(Value::Null, -32600, "Invalid request", None));
        };
        let Some(method) = m.get("method").and_then(Value::as_str) else {
            // A response to a request we never send, or junk.
            if m.contains_key("result") || m.contains_key("error") {
                return None;
            }
            let id = m.get("id").cloned().unwrap_or(Value::Null);
            return Some(error(id, -32600, "Invalid request", None));
        };
        let id = match m.get("id") {
            None | Some(Value::Null) => return None, // notification
            Some(id) => id.clone(),
        };
        let no_params = Map::new();
        let params = m
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or(&no_params);

        if method == "initialize" {
            return Some(reply(id, self.initialize(params)));
        }

        // Modern requests carry their version; without one, the client is legacy and past `initialize`.
        let meta = params.get("_meta").and_then(Value::as_object);
        let requested = meta
            .and_then(|meta| meta.get(VERSION_KEY))
            .and_then(Value::as_str);
        let modern = requested.is_some() || method == "server/discover";
        if modern {
            let Some(requested) = requested else {
                let message = format!("Missing _meta {VERSION_KEY}");
                return Some(error(id, -32602, &message, None));
            };
            if !MODERN_VERSIONS.contains(&requested) {
                let data = json!({"supported": supported_versions(), "requested": requested});
                return Some(error(
                    id,
                    -32022,
                    "Unsupported protocol version",
                    Some(data),
                ));
            }
            if !meta.is_some_and(|meta| meta.get(CAPABILITIES_KEY).is_some_and(Value::is_object)) {
                let message = format!("Missing _meta {CAPABILITIES_KEY}");
                return Some(error(id, -32602, &message, None));
            }
        }

        let mut result = match method {
            "server/discover" => json!({
                "supportedVersions": supported_versions(),
                "capabilities": {"tools": {}},
                "instructions": self.instructions,
            }),
            "tools/list" => json!({"tools": self.tools.iter().map(describe).collect::<Vec<_>>()}),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str);
                let Some(tool) = name.and_then(|name| self.tools.iter().find(|t| t.name == name))
                else {
                    let shown = match params.get("name") {
                        Some(Value::String(name)) => name.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    return Some(error(id, -32602, &format!("Unknown tool: {shown}"), None));
                };
                let no_arguments = Map::new();
                let arguments = params
                    .get("arguments")
                    .and_then(Value::as_object)
                    .unwrap_or(&no_arguments);
                call(tool, arguments)
            }
            "ping" => json!({}),
            _ => {
                return Some(error(
                    id,
                    -32601,
                    &format!("Method not found: {method}"),
                    None,
                ))
            }
        };
        if modern {
            if let Value::Object(fields) = &mut result {
                fields.insert("resultType".to_string(), json!("complete"));
                fields.insert(
                    "_meta".to_string(),
                    json!({ SERVER_INFO_KEY: self.server_info() }),
                );
            }
        }
        Some(reply(id, result))
    }

    fn server_info(&self) -> Value {
        json!({"name": self.name, "version": self.version})
    }

    /// Legacy handshake: agree on the client's version when it's one we know, else offer the newest legacy one.
    fn initialize(&self, params: &Map<String, Value>) -> Value {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("");
        let agreed = if LEGACY_VERSIONS.contains(&asked) {
            asked
        } else {
            NEWEST_LEGACY
        };
        json!({
            "protocolVersion": agreed,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": self.server_info(),
            "instructions": self.instructions,
        })
    }
}

fn supported_versions() -> Vec<&'static str> {
    MODERN_VERSIONS
        .iter()
        .chain(LEGACY_VERSIONS)
        .copied()
        .collect()
}

fn describe(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": tool.input_schema,
        "annotations": {
            "title": tool.title,
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

fn call(tool: &Tool, arguments: &Map<String, Value>) -> Value {
    let (text, failed) = match (tool.run)(arguments) {
        Ok(text) => (text, false),
        Err(e) => (e.message, true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": failed})
}

fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i32, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({"code": code, "message": message});
    if let (Some(data), Value::Object(fields)) = (data, &mut e) {
        fields.insert("data".to_string(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": e})
}

/// Compact JSON never contains a raw newline: line breaks inside strings are escaped.
fn encode(message: &Value) -> String {
    serde_json::to_string(message).unwrap_or_else(|_| INTERNAL_ERROR_REPLY.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufReader, Cursor, Read};

    use serde_json::json;

    use super::*;

    /// A server with two stand-in tools: `echo` returns its `text` argument, `fail` always fails.
    fn test_server() -> McpServer {
        let tools = vec![
            Tool::new(
                "echo",
                "Echo",
                "Returns its text argument.",
                json!({"type": "object", "properties": {"text": {"type": "string"}}}),
                |args| {
                    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
                    Ok(text.to_string())
                },
            ),
            Tool::new(
                "fail",
                "Fail",
                "Always fails.",
                json!({"type": "object"}),
                |_| Err(ToolError::new("nothing to see")),
            ),
        ];
        McpServer::new("dictate", "test", "Use the test tools.", tools)
    }

    /// One JSON-RPC message in, the parsed one-line reply out.
    fn send(server: &McpServer, message: Value) -> Option<Value> {
        let reply = server.handle_line(&message.to_string())?;
        assert!(!reply.contains('\n'), "stdio messages must be one line");
        Some(serde_json::from_str(&reply).expect("a reply is JSON"))
    }

    /// A modern (2026-07-28) request: version and capabilities in `_meta`.
    fn modern_as(server: &McpServer, method: &str, params: Value, version: &str) -> Value {
        let mut params = params;
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        let message = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        send(server, message).expect("a request gets a reply")
    }

    fn modern(server: &McpServer, method: &str, params: Value) -> Value {
        modern_as(server, method, params, "2026-07-28")
    }

    fn legacy(server: &McpServer, method: &str, params: Value) -> Value {
        let message = json!({"jsonrpc": "2.0", "id": "a", "method": method, "params": params});
        send(server, message).expect("a request gets a reply")
    }

    /// The text a tool returned, and whether it was an error.
    fn call(server: &McpServer, tool: &str, args: Value) -> (String, bool) {
        let reply = legacy(
            server,
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        let text = reply["result"]["content"][0]["text"].as_str().unwrap_or("");
        (
            text.to_string(),
            reply["result"]["isError"].as_bool().unwrap_or(true),
        )
    }

    #[test]
    fn legacy_initialize_agrees_on_a_known_version() {
        let s = test_server();
        let params = json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}});
        let r = legacy(&s, "initialize", params);
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], "a");
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            r["result"]["serverInfo"],
            json!({"name": "dictate", "version": "test"})
        );
        assert_eq!(
            r["result"]["capabilities"],
            json!({"tools": {"listChanged": false}})
        );
        assert_eq!(r["result"]["instructions"], "Use the test tools.");
    }

    #[test]
    fn legacy_initialize_agrees_on_each_legacy_version() {
        let s = test_server();
        for version in LEGACY_VERSIONS {
            let r = legacy(&s, "initialize", json!({"protocolVersion": version}));
            assert_eq!(r["result"]["protocolVersion"], *version);
        }
    }

    #[test]
    fn legacy_initialize_offers_the_newest_legacy_version_otherwise() {
        let s = test_server();
        let newest = "2025-11-25";
        let unknown = legacy(&s, "initialize", json!({"protocolVersion": "2099-01-01"}));
        assert_eq!(unknown["result"]["protocolVersion"], newest);
        // The modern version is negotiated per request, never through `initialize`.
        let modern_asked = legacy(&s, "initialize", json!({"protocolVersion": "2026-07-28"}));
        assert_eq!(modern_asked["result"]["protocolVersion"], newest);
        assert_eq!(
            legacy(&s, "initialize", json!({}))["result"]["protocolVersion"],
            newest
        );
        let not_a_string = legacy(&s, "initialize", json!({"protocolVersion": 20250618}));
        assert_eq!(not_a_string["result"]["protocolVersion"], newest);
    }

    #[test]
    fn notifications_get_no_reply() {
        let s = test_server();
        assert!(send(
            &s,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_none());
        assert!(send(&s, json!({"jsonrpc": "2.0", "id": null, "method": "ping"})).is_none());
    }

    #[test]
    fn legacy_results_stay_as_they_were() {
        let s = test_server();
        let list = legacy(&s, "tools/list", json!({}));
        assert!(list["result"].get("resultType").is_none());
        assert!(list["result"].get("_meta").is_none());
        assert_eq!(legacy(&s, "ping", json!({}))["result"], json!({}));
        // Without params at all, the client is legacy too.
        let bare = send(&s, json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}));
        assert_eq!(bare, Some(json!({"jsonrpc": "2.0", "id": 5, "result": {}})));
    }

    #[test]
    fn modern_discover_describes_the_server() {
        let s = test_server();
        let r = modern(&s, "server/discover", json!({}));
        let d = &r["result"];
        assert_eq!(d["resultType"], "complete");
        assert_eq!(
            d["supportedVersions"],
            json!([
                "2026-07-28",
                "2025-11-25",
                "2025-06-18",
                "2025-03-26",
                "2024-11-05"
            ])
        );
        assert_eq!(d["capabilities"], json!({"tools": {}}));
        assert_eq!(d["instructions"], "Use the test tools.");
        let info = &d["_meta"]["io.modelcontextprotocol/serverInfo"];
        assert_eq!(*info, json!({"name": "dictate", "version": "test"}));
    }

    #[test]
    fn modern_results_carry_type_and_server_info() {
        let s = test_server();
        let ping = modern(&s, "ping", json!({}));
        let info =
            json!({"io.modelcontextprotocol/serverInfo": {"name": "dictate", "version": "test"}});
        assert_eq!(
            ping["result"],
            json!({"resultType": "complete", "_meta": info})
        );
        let call = modern(
            &s,
            "tools/call",
            json!({"name": "echo", "arguments": {"text": "hi"}}),
        );
        assert_eq!(call["result"]["resultType"], "complete");
        assert_eq!(call["result"]["content"][0]["text"], "hi");
    }

    #[test]
    fn tools_list_describes_each_tool_as_read_only() {
        let s = test_server();
        let r = modern(&s, "tools/list", json!({}));
        assert_eq!(r["result"]["resultType"], "complete");
        let tools = r["result"]["tools"].as_array().expect("a tools array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["echo", "fail"]);
        assert_eq!(
            tools[0],
            json!({
                "name": "echo",
                "title": "Echo",
                "description": "Returns its text argument.",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
                "annotations": {
                    "title": "Echo",
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            })
        );
    }

    #[test]
    fn modern_rejects_an_unsupported_version() {
        let s = test_server();
        let r = modern_as(&s, "tools/list", json!({}), "2099-01-01");
        assert_eq!(r["id"], 1);
        assert_eq!(r["error"]["code"], -32022);
        assert_eq!(r["error"]["message"], "Unsupported protocol version");
        let data = &r["error"]["data"];
        assert_eq!(data["requested"], "2099-01-01");
        assert_eq!(
            data["supported"],
            json!([
                "2026-07-28",
                "2025-11-25",
                "2025-06-18",
                "2025-03-26",
                "2024-11-05"
            ])
        );
        // A legacy version is not a modern one: per-request `_meta` only carries 2026-07-28.
        assert_eq!(
            modern_as(&s, "ping", json!({}), "2025-11-25")["error"]["code"],
            -32022
        );
    }

    #[test]
    fn modern_requires_its_meta_fields() {
        let s = test_server();
        let no_capabilities =
            json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}});
        let r = send(
            &s,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": no_capabilities}),
        );
        let r = r.expect("a request gets a reply");
        assert_eq!(r["id"], 2);
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(
            r["error"]["message"],
            "Missing _meta io.modelcontextprotocol/clientCapabilities"
        );
        // server/discover is modern by definition, so it needs the version too.
        let r = send(
            &s,
            json!({"jsonrpc": "2.0", "id": 3, "method": "server/discover"}),
        );
        let r = r.expect("a request gets a reply");
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(
            r["error"]["message"],
            "Missing _meta io.modelcontextprotocol/protocolVersion"
        );
        // Capabilities must be an object.
        let params = json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": "none"
        }});
        let r = send(
            &s,
            json!({"jsonrpc": "2.0", "id": 4, "method": "ping", "params": params}),
        );
        assert_eq!(r.expect("a request gets a reply")["error"]["code"], -32602);
    }

    #[test]
    fn a_tool_call_returns_the_tools_text() {
        let s = test_server();
        let (text, is_error) = call(&s, "echo", json!({"text": "Nhóm thống nhất tách repo"}));
        assert!(!is_error);
        assert_eq!(text, "Nhóm thống nhất tách repo");
        // Without arguments the tool still runs, with none.
        let reply = legacy(&s, "tools/call", json!({"name": "echo"}));
        assert_eq!(
            reply["result"],
            json!({"content": [{"type": "text", "text": ""}], "isError": false})
        );
        // Arguments that are not an object count as none.
        let reply = legacy(
            &s,
            "tools/call",
            json!({"name": "echo", "arguments": ["x"]}),
        );
        assert_eq!(reply["result"]["content"][0]["text"], "");
    }

    #[test]
    fn a_tool_error_comes_back_as_an_error_result() {
        let s = test_server();
        let reply = legacy(&s, "tools/call", json!({"name": "fail", "arguments": {}}));
        assert!(
            reply.get("error").is_none(),
            "a failing tool is not a protocol error"
        );
        assert_eq!(
            reply["result"],
            json!({"content": [{"type": "text", "text": "nothing to see"}], "isError": true})
        );
        let modern_reply = modern(&s, "tools/call", json!({"name": "fail"}));
        assert_eq!(modern_reply["result"]["isError"], true);
        assert_eq!(modern_reply["result"]["resultType"], "complete");
    }

    #[test]
    fn calling_an_unknown_tool_is_invalid_params() {
        let s = test_server();
        let r = legacy(&s, "tools/call", json!({"name": "nope"}));
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(r["error"]["message"], "Unknown tool: nope");
        let r = legacy(&s, "tools/call", json!({}));
        assert_eq!(r["error"]["code"], -32602);
        assert_eq!(r["error"]["message"], "Unknown tool: ");
    }

    #[test]
    fn protocol_errors() {
        let s = test_server();
        let r = legacy(&s, "resources/list", json!({}));
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["error"]["message"], "Method not found: resources/list");
        assert_eq!(r["id"], "a");
        let parse: Value =
            serde_json::from_str(&s.handle_line("{not json").expect("a reply")).expect("JSON");
        assert_eq!(parse["error"]["code"], -32700);
        assert_eq!(parse["error"]["message"], "Parse error");
        assert_eq!(parse["id"], Value::Null);
        assert!(s.handle_line("").is_none());
        assert!(s.handle_line("  \t ").is_none());
        // A response from the client, to a request we never send.
        assert!(send(&s, json!({"jsonrpc": "2.0", "id": 9, "result": {}})).is_none());
        assert!(send(
            &s,
            json!({"jsonrpc": "2.0", "id": 9, "error": {"code": 1, "message": "x"}})
        )
        .is_none());
    }

    #[test]
    fn a_top_level_scalar_is_a_parse_error() {
        let s = test_server();
        for line in ["42", "\"ping\"", "null", "true"] {
            let r: Value =
                serde_json::from_str(&s.handle_line(line).expect("a reply")).expect("JSON");
            assert_eq!(r["error"]["code"], -32700, "{line}");
        }
    }

    #[test]
    fn messages_that_are_not_requests_are_invalid() {
        let s = test_server();
        // No method, and not a response: junk that still names an id.
        let r = send(&s, json!({"jsonrpc": "2.0", "id": 7})).expect("a reply");
        assert_eq!(r["error"]["code"], -32600);
        assert_eq!(r["error"]["message"], "Invalid request");
        assert_eq!(r["id"], 7);
        // A method that is not a string is no method.
        let r = send(&s, json!({"jsonrpc": "2.0", "id": 8, "method": 5})).expect("a reply");
        assert_eq!(r["error"]["code"], -32600);
        assert_eq!(r["id"], 8);
        // A batch entry that is not an object.
        let r: Value = serde_json::from_str(&s.handle_line("[1]").expect("a reply")).expect("JSON");
        assert_eq!(r[0]["error"]["code"], -32600);
        assert_eq!(r[0]["id"], Value::Null);
    }

    #[test]
    fn ids_come_back_as_they_came() {
        let s = test_server();
        for id in [json!(0), json!(7), json!("abc"), json!(1.5), json!(true)] {
            let r =
                send(&s, json!({"jsonrpc": "2.0", "id": id, "method": "ping"})).expect("a reply");
            assert_eq!(r["id"], id);
        }
    }

    #[test]
    fn malformed_lines_keep_serving() {
        let s = test_server();
        let r: Value = serde_json::from_str(&s.handle_line("{nope").unwrap()).unwrap();
        assert_eq!(r["error"]["code"], -32700);
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        let b: Value = serde_json::from_str(&s.handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#).unwrap()).unwrap();
        assert_eq!(b.as_array().unwrap().len(), 2);
        assert_eq!(b[1]["error"]["code"], -32601);
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#)
            .is_some());
    }

    #[test]
    fn a_batch_answers_only_its_requests() {
        let s = test_server();
        let line = r#"[{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
        let r: Value = serde_json::from_str(&s.handle_line(line).expect("a reply")).expect("JSON");
        assert_eq!(r, json!([{"jsonrpc": "2.0", "id": 1, "result": {}}]));
        // Nothing to answer, nothing to send.
        let notifications = r#"[{"jsonrpc":"2.0","method":"a"},{"jsonrpc":"2.0","method":"b"}]"#;
        assert!(s.handle_line(notifications).is_none());
        assert!(s.handle_line("[]").is_none());
    }

    fn ping(id: u32) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#)
    }

    fn replies(output: Vec<u8>) -> Vec<Value> {
        let text = String::from_utf8(output).expect("replies are UTF-8");
        assert!(
            text.is_empty() || text.ends_with('\n'),
            "every reply ends with a newline"
        );
        text.lines()
            .map(|l| serde_json::from_str(l).expect("a reply is JSON"))
            .collect()
    }

    #[test]
    fn serve_answers_each_line_until_the_input_ends() {
        let s = test_server();
        let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        // Blank lines, a notification and a CRLF ending are skipped or tolerated; the last line has no newline.
        let input = format!("{}\r\n\n{notification}\n{{nope\n{}", ping(1), ping(2));
        let mut output = Vec::new();
        s.serve(Cursor::new(input), &mut output);
        let r = replies(output);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0]["id"], 1);
        assert_eq!(r[1]["error"]["code"], -32700);
        assert_eq!(r[2]["id"], 2);
    }

    #[test]
    fn serve_repairs_invalid_utf8_and_keeps_serving() {
        let s = test_server();
        let mut input = b"\xff\xfe\n".to_vec();
        input.extend_from_slice(ping(4).as_bytes());
        input.push(b'\n');
        let mut output = Vec::new();
        s.serve(Cursor::new(input), &mut output);
        let r = replies(output);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["error"]["code"], -32700);
        assert_eq!(r[1]["id"], 4);
    }

    /// Counts what the server writes and how often it flushes.
    #[derive(Default)]
    struct Recorder {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn serve_flushes_after_every_reply() {
        let s = test_server();
        let mut out = Recorder::default();
        s.serve(Cursor::new(format!("{}\n{}\n", ping(1), ping(2))), &mut out);
        assert_eq!(out.flushes, 2);
        assert_eq!(replies(out.bytes).len(), 2);
    }

    /// A client that went away: every write fails.
    #[derive(Default)]
    struct Closed {
        writes: usize,
    }

    impl Write for Closed {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn serve_stops_when_the_client_is_gone() {
        let s = test_server();
        let mut out = Closed::default();
        s.serve(
            Cursor::new(format!("{}\n{}\n{}\n", ping(1), ping(2), ping(3))),
            &mut out,
        );
        assert_eq!(out.writes, 1);
    }

    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("input failed"))
        }
    }

    #[test]
    fn serve_stops_when_the_input_fails() {
        let s = test_server();
        let mut output = Vec::new();
        s.serve(BufReader::new(Broken), &mut output);
        assert!(output.is_empty());
    }
}
