#![allow(clippy::missing_errors_doc)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::usage::{self, Origin, Parsed, Window};

pub const TOOL_PREFIX: &str = "mcp__";
pub const SEPARATOR: &str = "__";
pub const USER_CONFIG_RELATIVE: &str = ".claude.json";
const MAX_NAME_CHARS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct McpName(String);

impl McpName {
    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

impl TryFrom<String> for McpName {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let ok = !s.is_empty()
            && s.chars().count() <= MAX_NAME_CHARS
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if ok { Ok(Self(s)) } else { Err(format!("'{s}' is not an MCP server or tool name")) }
    }
}

impl From<McpName> for String {
    fn from(n: McpName) -> Self { n.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTrigger {
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpRecord {
    pub ts: String,
    pub event: String,
    pub trigger: McpTrigger,
    pub server: McpName,
    pub tool: McpName,
    #[serde(flatten)]
    pub origin: Origin,
}

#[must_use]
pub fn split_tool_name(rest: &str) -> Option<(McpName, McpName)> {
    let (server, tool) = rest.split_once(SEPARATOR)?;
    Some((McpName::try_from(server.to_owned()).ok()?, McpName::try_from(tool.to_owned()).ok()?))
}

#[must_use]
pub fn record(rest: &str, event: &Value, event_name: Option<String>, now: i64) -> Option<McpRecord> {
    let (server, tool) = split_tool_name(rest)?;
    Some(McpRecord {
        ts: usage::format_rfc3339(now),
        event: event_name.unwrap_or_else(|| "PreToolUse".to_owned()),
        trigger: McpTrigger::Mcp,
        server,
        tool,
        origin: Origin::of(event),
    })
}

#[must_use]
pub fn normalise_server_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect()
}

#[must_use]
pub fn default_config_path(home: Option<OsString>) -> Option<PathBuf> {
    home.map(|h| PathBuf::from(h).join(USER_CONFIG_RELATIVE))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigSource {
    pub path: String,
    pub present: bool,
    pub servers: Vec<String>,
}

pub fn read_config(path: &Path, required: bool) -> anyhow::Result<ConfigSource> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigSource { path: path.display().to_string(), present: false, servers: Vec::new() });
        }
        Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
    };
    let doc: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let servers = match doc.get("mcpServers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        Some(_) => anyhow::bail!("{}: mcpServers is not an object", path.display()),
    };
    Ok(ConfigSource { path: path.display().to_string(), present: true, servers })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolList {
    pub path: String,
    pub tools: usize,
    pub servers: usize,
    #[serde(skip)]
    pub per_server: BTreeMap<String, usize>,
}

pub fn read_tools(path: &Path) -> anyhow::Result<ToolList> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let names: BTreeSet<(McpName, McpName)> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '"' | '[' | ']'))
        .filter_map(|t| t.strip_prefix(TOOL_PREFIX))
        .filter_map(split_tool_name)
        .collect();
    let mut per_server: BTreeMap<String, usize> = BTreeMap::new();
    for (server, _) in &names {
        *per_server.entry(server.as_str().to_owned()).or_default() += 1;
    }
    Ok(ToolList { path: path.display().to_string(), tools: names.len(), servers: per_server.len(), per_server })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerUse {
    pub server: String,
    pub calls: usize,
    pub distinct_tools: usize,
    pub tools_used: Vec<String>,
    pub last_used: String,
    pub config_name: Option<String>,
    pub tool_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnusedServer {
    pub server: String,
    pub config_name: String,
    pub tool_count: Option<usize>,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpReport {
    pub log: String,
    pub log_present: bool,
    pub now: String,
    pub window: String,
    pub since: Option<String>,
    pub lines_read: usize,
    pub malformed_lines: usize,
    pub skill_lines: usize,
    pub events_in_window: usize,
    pub configs: Vec<ConfigSource>,
    pub configured: usize,
    pub tools_file: Option<ToolList>,
    pub used: Vec<ServerUse>,
    pub unused: Vec<UnusedServer>,
    pub unused_tool_names: usize,
}

pub struct McpReportInput<'a> {
    pub log: &'a Path,
    pub now: i64,
    pub window: Window,
    pub window_label: &'a str,
    pub configs: Vec<ConfigSource>,
    pub tools: Option<ToolList>,
}

#[derive(Default)]
struct Tally {
    calls: usize,
    tools: BTreeSet<String>,
    last_used: i64,
}

pub fn report(input: McpReportInput<'_>) -> anyhow::Result<McpReport> {
    let since = input.window.lower_bound(input.now);
    let (text, log_present) = usage::read_log(input.log)?;

    let mut lines_read = 0;
    let mut malformed_lines = 0;
    let mut skill_lines = 0;
    let mut events_in_window = 0;
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut last_seen: BTreeMap<String, i64> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines_read += 1;
        let (rec, ts) = match usage::parse_line(line) {
            Parsed::Mcp(rec, ts) => (rec, ts),
            Parsed::Skill(..) => {
                skill_lines += 1;
                continue;
            }
            Parsed::Malformed => {
                malformed_lines += 1;
                continue;
            }
        };
        let server = rec.server.as_str().to_owned();
        let seen = last_seen.entry(server.clone()).or_insert(ts);
        *seen = (*seen).max(ts);
        if since.is_some_and(|s| ts < s) || ts > input.now {
            continue;
        }
        events_in_window += 1;
        let t = tallies.entry(server).or_default();
        t.calls += 1;
        t.tools.insert(rec.tool.as_str().to_owned());
        t.last_used = t.last_used.max(ts);
    }

    let mut configured: BTreeMap<String, String> = BTreeMap::new();
    for name in input.configs.iter().flat_map(|c| &c.servers) {
        configured.entry(normalise_server_name(name)).or_insert_with(|| name.clone());
    }
    let known_count = |server: &str| input.tools.as_ref().and_then(|t| t.per_server.get(server).copied());

    let mut used: Vec<ServerUse> = tallies
        .iter()
        .map(|(server, t)| ServerUse {
            server: server.clone(),
            calls: t.calls,
            distinct_tools: t.tools.len(),
            tools_used: t.tools.iter().cloned().collect(),
            last_used: usage::format_rfc3339(t.last_used),
            config_name: configured.get(server).cloned(),
            tool_count: known_count(server),
        })
        .collect();
    used.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.server.cmp(&b.server)));

    let mut unused: Vec<UnusedServer> = configured
        .iter()
        .filter(|(server, _)| !tallies.contains_key(*server))
        .map(|(server, config_name)| UnusedServer {
            server: server.clone(),
            config_name: config_name.clone(),
            tool_count: known_count(server),
            last_seen: last_seen.get(server).copied().map(usage::format_rfc3339),
        })
        .collect();
    unused.sort_by(|a, b| b.tool_count.cmp(&a.tool_count).then_with(|| a.server.cmp(&b.server)));
    let unused_tool_names = unused.iter().filter_map(|u| u.tool_count).sum();

    Ok(McpReport {
        log: input.log.display().to_string(),
        log_present,
        now: usage::format_rfc3339(input.now),
        window: input.window_label.to_owned(),
        since: since.map(usage::format_rfc3339),
        lines_read,
        malformed_lines,
        skill_lines,
        events_in_window,
        configured: configured.len(),
        configs: input.configs,
        tools_file: input.tools,
        used,
        unused,
        unused_tool_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_name_splits_at_the_first_double_underscore() {
        let (s, t) = split_tool_name("hosted_Docs__read_page").unwrap();
        assert_eq!((s.as_str(), t.as_str()), ("hosted_Docs", "read_page"));
        assert!(split_tool_name("only-server").is_none());
        assert!(split_tool_name("__tool").is_none());
        assert!(split_tool_name("server__").is_none());
        assert!(split_tool_name("server__bad tool").is_none());
    }

    #[test]
    fn server_names_normalise_the_way_tool_prefixes_do() {
        assert_eq!(normalise_server_name("dotted.srv"), "dotted_srv");
        assert_eq!(normalise_server_name("hosted Docs"), "hosted_Docs");
        assert_eq!(normalise_server_name("plain-srv_1"), "plain-srv_1");
    }

    #[test]
    fn an_empty_name_never_deserialises() {
        assert!(serde_json::from_str::<McpName>("\"\"").is_err());
        assert!(serde_json::from_str::<McpName>("\"ok-name\"").is_ok());
    }
}
