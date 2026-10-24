//! The tools the MCP server offers. Names, descriptions and argument schemas are the pack's own, so an agent that knows the pack's tools knows these.
//! They are read from `catalog.json`, which `tools/export_mcp_catalog.py` writes from the pack's MCP server and the endpoints this app serves.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

const CATALOG: &str = include_str!("catalog.json");

/// Where a generated tool goes: its method and path, and which argument fills each `{parameter}`.
#[derive(Debug, Deserialize)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub args: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<Value>,
    /// `named` tools have their own recipe in `exec.rs`. `endpoint` tools are one call to the route.
    pub kind: String,
    #[serde(default)]
    pub route: Option<Route>,
}

#[derive(Debug, Deserialize)]
pub struct Endpoint {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub tools: Vec<Tool>,
    pub endpoints: Vec<Endpoint>,
}

pub fn catalog() -> &'static Catalog {
    static LOADED: OnceLock<Catalog> = OnceLock::new();
    LOADED.get_or_init(|| serde_json::from_str(CATALOG).expect("catalog.json is valid"))
}

impl Catalog {
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Whether `method path` is an endpoint of the API. A `{parameter}` matches any one segment.
    pub fn serves(&self, method: &str, path: &str) -> bool {
        let wanted: Vec<&str> = path.split('?').next().unwrap_or("").split('/').filter(|s| !s.is_empty()).collect();
        self.endpoints.iter().any(|e| {
            if !e.method.eq_ignore_ascii_case(method) {
                return false;
            }
            let pattern: Vec<&str> = e.path.split('/').filter(|s| !s.is_empty()).collect();
            pattern.len() == wanted.len() && pattern.iter().zip(&wanted).all(|(p, w)| (p.starts_with('{') && p.ends_with('}')) || p == w)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_named_and_endpoint_tools() {
        let all = catalog();
        assert!(all.tools.iter().any(|t| t.name == "project_create" && t.kind == "named"));
        assert!(all.tools.iter().filter(|t| t.kind == "endpoint").all(|t| t.route.is_some()));
        let mut names: Vec<&str> = all.tools.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.tools.len(), "tool names are unique");
    }

    #[test]
    fn serves_matches_paths_with_parameters() {
        let all = catalog();
        assert!(all.serves("GET", "/projects/Demo%20Song/scenes/seg_1"));
        assert!(all.serves("post", "/projects/x/scenes/y/video/render?x=1"));
        assert!(!all.serves("GET", "/setup/config"));
        assert!(!all.serves("DELETE", "/health"));
    }
}
