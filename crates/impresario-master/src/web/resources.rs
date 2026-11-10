//! Documentation exposed by the web console.
//!
//! The same Markdown files are compiled into the MCP server
//! (`impresario://docs/...`), so a browser and an AI agent read the docs of
//! the binary they are connected to.

use serde_json::{json, Value};

pub struct Doc {
    pub slug: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub text: &'static str,
}

const DOCS: &[Doc] = &[
    Doc {
        slug: "web",
        title: "Web console",
        description: "The guided browser console: start-up flow, screens, API envelope, action \
catalogue, security model and the CLI-child architecture.",
        text: include_str!("../../../../docs/WEB.md"),
    },
    Doc {
        slug: "dsl",
        title: "Impresario DSL reference",
        description: "Scene/play syntax, selectors, checks, actions, variables and the \
diagnostic code list — everything needed to author .scene/.play files.",
        text: include_str!("../../../../docs/DSL.md"),
    },
    Doc {
        slug: "parity",
        title: "Platform support matrix",
        description: "Which actions and checks work on each platform (Android, Android TV, \
web, webOS, Tizen, iOS, tvOS), generated from the planner's own tables.",
        text: include_str!("../../../../docs/PARITY.md"),
    },
    Doc {
        slug: "devices",
        title: "Device setup guide",
        description: "Per-platform setup and evidence: Android over adb/Wi-Fi, Android TV, \
web/headless Chrome, webOS/Tizen, emulators, stay-awake and vendor-freeze notes.",
        text: include_str!("../../../../docs/DEVICES.md"),
    },
    Doc {
        slug: "cli",
        title: "CLI and TUI reference",
        description: "Every command, exit code, scriptable output and the TUI client model.",
        text: include_str!("../../../../docs/CLI.md"),
    },
    Doc {
        slug: "config",
        title: "Configuration reference",
        description: "Layered settings, keys, scopes, defaults and validation.",
        text: include_str!("../../../../docs/CONFIG.md"),
    },
    Doc {
        slug: "scheduling",
        title: "Schedules, watchers and resident delivery",
        description: "Resident runner reference: layered schedule/watcher config, local-time \
and cron firing, missed/busy policies, dumps and retention, notifications, quiet hours, rate \
limits and webhooks.",
        text: include_str!("../../../../docs/SCHEDULING.md"),
    },
    Doc {
        slug: "probes",
        title: "Probe evidence and verdict model",
        description: "Probe kinds, evidence sources and confidence, per-platform reachability, \
fallback rungs and the in-app SDK contract.",
        text: include_str!("../../../../docs/PROBES.md"),
    },
    Doc {
        slug: "errors",
        title: "Error and diagnostic codes",
        description: "Diagnostic codes, exit codes and remediation hints.",
        text: include_str!("../../../../docs/ERRORS.md"),
    },
    Doc {
        slug: "mcp",
        title: "MCP server guide",
        description: "The same workspace exposed to AI agents: tools, resources, transport, \
concurrency and security notes.",
        text: include_str!("../../../../docs/MCP.md"),
    },
];

#[must_use]
pub fn list() -> Value {
    Value::Array(
        DOCS.iter()
            .map(|doc| {
                json!({
                    "slug": doc.slug,
                    "title": doc.title,
                    "description": doc.description,
                    "bytes": doc.text.len(),
                    "url": format!("/api/docs/{}", doc.slug),
                    "raw": format!("/api/docs/{}/raw", doc.slug),
                })
            })
            .collect(),
    )
}

#[must_use]
pub fn get(slug: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|doc| doc.slug == slug)
}
