//! Embedded MCP server (`impresario mcp`): exposes the workspace, the
//! configured devices and the run pipeline as Model Context Protocol tools
//! and resources, so any MCP-capable AI agent can author, run and debug
//! plays without shelling out (decision I-80, not in the spec CLI list —
//! like `mock-agent`, I-77). See `docs/MCP.md`.
//!
//! Transport: stdio, newline-delimited JSON-RPC 2.0 (the MCP stdio
//! binding). Stdout carries protocol messages only — every log, diagnostic
//! and progress note goes to stderr (tracing is already wired there).
//!
//! Era support is **dual**:
//! - a 2026-07-28 client discovers the server with `server/discover` and
//!   carries `_meta.io.modelcontextprotocol/protocolVersion` on every
//!   request (stateless, `resultType: "complete"` results);
//! - older clients negotiate with `initialize` and get legacy semantics
//!   for the life of the process (2025-11-25 and back, no `resultType`).
//!
//! Long tool calls (`tools/call`) run on their own thread so pings and
//! further requests keep flowing; responses may arrive out of order and are
//! correlated by JSON-RPC id, as the spec allows. Device-touching tools
//! serialize on a process-wide lock (see `handlers`).

mod handlers;
mod protocol;
mod resources;
mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::Value;

use crate::cli::EXIT_OK;

pub(crate) use protocol::Server;

/// Serve MCP on stdio until stdin closes (the client hung up); returns the
/// process exit code.
#[must_use]
pub fn run_stdio(base: PathBuf, config_path: PathBuf, devices_path: PathBuf) -> i32 {
    let server = Server {
        base,
        config_path,
        devices_path,
    };
    serve(server, std::io::stdin().lock(), std::io::stdout())
}

/// The read loop: parse one JSON-RPC message per line, answer cheap methods
/// inline and give `tools/call` a worker thread of its own.
fn serve<R: BufRead>(server: Server, reader: R, writer: impl Write + Send + 'static) -> i32 {
    let writer = Arc::new(Mutex::new(writer));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                tracing::debug!("mcp: stdin closed with an error: {err}");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => {
                write_message(
                    &writer,
                    &protocol::error_response(
                        Value::Null,
                        protocol::PARSE_ERROR,
                        &format!("invalid JSON: {err}"),
                        None,
                    ),
                );
                continue;
            }
        };
        // Only a `tools/call` with an id can block for minutes (a run or a
        // device check); JSON-RPC notifications are never answered, so they
        // stay inline.
        let needs_worker = message.get("method").and_then(Value::as_str) == Some("tools/call")
            && message.get("id").is_some();
        if !needs_worker {
            if let Some(response) = protocol::dispatch(&server, &message) {
                write_message(&writer, &response);
            }
            continue;
        }
        workers.retain(|worker| !worker.is_finished());
        let server = server.clone();
        let writer = Arc::clone(&writer);
        let spawned = std::thread::Builder::new()
            .name("impresario-mcp-tool".to_string())
            .spawn(move || {
                if let Some(response) = protocol::dispatch(&server, &message) {
                    write_message(&writer, &response);
                }
            });
        match spawned {
            Ok(worker) => workers.push(worker),
            Err(err) => eprintln!("mcp: cannot start a tool worker: {err}"),
        }
    }
    EXIT_OK
}

/// One serialized JSON-RPC message per line, flushed immediately; stdout is
/// shared by the read loop and every worker thread.
fn write_message<W: Write>(writer: &Mutex<W>, message: &Value) {
    let text = match serde_json::to_string(message) {
        Ok(text) => text,
        Err(err) => {
            // Cannot happen for values built here, but never panic on it.
            eprintln!("mcp: cannot serialize a response: {err}");
            return;
        }
    };
    let mut guard = match writer.lock() {
        Ok(guard) => guard,
        Err(poison) => poison.into_inner(),
    };
    let written = guard
        .write_all(text.as_bytes())
        .and_then(|()| guard.write_all(b"\n"))
        .and_then(|()| guard.flush());
    if let Err(err) = written {
        eprintln!("mcp: cannot write a response: {err}");
    }
}
