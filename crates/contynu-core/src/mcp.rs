use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use chrono::{DateTime, Utc};

use crate::blobs::BlobStore;
use crate::checkpoint::CheckpointManager;
use crate::error::Result;
use crate::ids::{MemoryId, ProjectId};
use crate::state::StatePaths;
use crate::store::{
    MemoryObject, MemoryObjectKind, MemoryQuery, MemoryScope, MemorySortBy, MetadataStore,
    PromptRecord,
};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    pub fn method_not_found(id: Option<Value>) -> Self {
        Self::err(id, -32601, "Method not found")
    }

    pub fn invalid_params(id: Option<Value>, msg: impl Into<String>) -> Self {
        Self::err(id, -32602, msg)
    }

    pub fn parse_error(msg: &str) -> Self {
        Self::err(None, -32700, format!("Parse error: {msg}"))
    }
}

// ---------------------------------------------------------------------------
// MCP protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Debug, Serialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl McpToolResult {
    fn text(s: String) -> Self {
        Self {
            content: vec![McpContent { content_type: "text".into(), text: s }],
            is_error: None,
        }
    }

    fn error(msg: String) -> Self {
        Self {
            content: vec![McpContent { content_type: "text".into(), text: msg }],
            is_error: Some(true),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP Dispatcher
// ---------------------------------------------------------------------------

const SERVER_NAME: &str = "contynu";
const PROTOCOL_VERSION: &str = "2025-03-26";

pub struct McpDispatcher {
    store: MetadataStore,
    state: StatePaths,
    blob_store: BlobStore,
    active_project: ProjectId,
}

impl McpDispatcher {
    pub fn new(
        state_dir: &std::path::Path,
        active_project: ProjectId,
    ) -> Result<Self> {
        let state = StatePaths::new(state_dir);
        let store = MetadataStore::open(state.sqlite_db())?;
        let blob_store = BlobStore::new(state.blobs_root());
        Ok(Self { store, state, blob_store, active_project })
    }

    /// For testing: construct from already-opened components.
    pub fn from_parts(
        store: MetadataStore,
        state: StatePaths,
        blob_store: BlobStore,
        active_project: ProjectId,
    ) -> Self {
        Self { store, state, blob_store, active_project }
    }

    pub fn handle_request(&self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        match req.method.as_str() {
            "initialize" => Some(self.handle_initialize(req)),
            "notifications/initialized" => None, // notification, no response
            "tools/list" => Some(self.handle_tools_list(req)),
            "tools/call" => Some(self.handle_tools_call(req)),
            "resources/list" => Some(self.handle_resources_list(req)),
            "resources/read" => Some(self.handle_resources_read(req)),
            _ => Some(JsonRpcResponse::method_not_found(req.id.clone())),
        }
    }

    fn handle_initialize(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::ok(req.id.clone(), json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": env!("CARGO_PKG_VERSION")
            }
        }))
    }

    fn handle_tools_list(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::ok(req.id.clone(), json!({ "tools": self.list_tools() }))
    }

    fn handle_tools_call(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let name = req.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = req.params.get("arguments").cloned().unwrap_or(json!({}));
        match self.call_tool(name, &arguments) {
            Ok(result) => JsonRpcResponse::ok(req.id.clone(), serde_json::to_value(result).unwrap()),
            Err(e) => JsonRpcResponse::ok(
                req.id.clone(),
                serde_json::to_value(McpToolResult::error(e.to_string())).unwrap(),
            ),
        }
    }

    fn handle_resources_list(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::ok(req.id.clone(), json!({ "resources": self.list_resources() }))
    }

    fn handle_resources_read(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let uri = req.params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        match self.read_resource(uri) {
            Ok(content) => JsonRpcResponse::ok(req.id.clone(), json!({
                "contents": [content]
            })),
            Err(e) => JsonRpcResponse::invalid_params(
                req.id.clone(),
                format!("Failed to read resource: {e}"),
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Tools
    // -----------------------------------------------------------------------

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "search_memory".into(),
                description: "Search project memory by text, kind, scope, or time window. Returns facts, decisions, constraints, todos, and other memories matching the filters. Results are paginated (default 20, max 50 per page).".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text to search for in memory content" },
                        "kind": { "type": "string", "description": "Filter by kind: fact, constraint, decision, todo, user_fact, project_knowledge" },
                        "scope": { "type": "string", "description": "Filter by scope: user, project, session" },
                        "after": { "type": "string", "description": "Only memories created after this ISO datetime (e.g. 2026-04-01T00:00:00Z)" },
                        "before": { "type": "string", "description": "Only memories created before this ISO datetime" },
                        "sort_by": { "type": "string", "description": "Sort order: 'importance' (default) or 'recency'" },
                        "limit": { "type": "integer", "description": "Max results to return (default 20, max 50)" },
                        "offset": { "type": "integer", "description": "Skip this many results for pagination (default 0)" }
                    }
                }),
            },
            McpTool {
                name: "list_memories".into(),
                description: "Browse all active project memories with optional filtering and sorting. Use this to explore what the project knows without a specific search term. Results are paginated.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "description": "Filter by kind: fact, constraint, decision, todo, user_fact, project_knowledge" },
                        "scope": { "type": "string", "description": "Filter by scope: user, project, session" },
                        "sort_by": { "type": "string", "description": "Sort order: 'importance' (default) or 'recency'" },
                        "limit": { "type": "integer", "description": "Max results (default 20, max 50)" },
                        "offset": { "type": "integer", "description": "Skip results for pagination (default 0)" }
                    }
                }),
            },
            McpTool {
                name: "write_memory".into(),
                description: "Write a new memory to the project's persistent memory store. Use this to record facts, decisions, constraints, todos, user preferences, or project knowledge that should be recalled in future sessions. Only write memories that are genuinely worth recalling later — skip ephemeral details, debugging steps, and information already in the code.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The memory content — a clear, concise statement of the fact, decision, constraint, or todo" },
                        "kind": { "type": "string", "description": "Memory kind: fact, constraint, decision, todo, user_fact, project_knowledge" },
                        "scope": { "type": "string", "description": "Memory scope: user (persists across all projects), project (this project only), session (this session only). Default: project" },
                        "importance": { "type": "number", "description": "Importance score from 0.0 to 1.0 (default 0.5). Higher = more likely to be included in rehydration." },
                        "reason": { "type": "string", "description": "Why this memory is worth storing — helps with future relevance judgments" }
                    },
                    "required": ["text", "kind"]
                }),
            },
            McpTool {
                name: "update_memory".into(),
                description: "Update an existing memory's text, importance, or reason. Use this to correct or refine memories rather than creating duplicates.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "memory_id": { "type": "string", "description": "The ID of the memory to update (mem_xxx format)" },
                        "text": { "type": "string", "description": "New memory text" },
                        "importance": { "type": "number", "description": "New importance score (0.0-1.0)" },
                        "reason": { "type": "string", "description": "Reason for the update" }
                    },
                    "required": ["memory_id", "text"]
                }),
            },
            McpTool {
                name: "delete_memory".into(),
                description: "Delete a memory that is no longer relevant or was recorded in error.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "memory_id": { "type": "string", "description": "The ID of the memory to delete (mem_xxx format)" }
                    },
                    "required": ["memory_id"]
                }),
            },
            McpTool {
                name: "record_prompt".into(),
                description: "Record the user's prompt verbatim, along with an optional interpretation if the prompt is ambiguous. Call this at every stop point (each time you finish generating and wait for the user's next input).".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "verbatim": { "type": "string", "description": "The exact user prompt text" },
                        "interpretation": { "type": "string", "description": "Your interpretation of the prompt if it is ambiguous or unclear" },
                        "interpretation_confidence": { "type": "number", "description": "Confidence in the interpretation (0.0-1.0)" }
                    },
                    "required": ["verbatim"]
                }),
            },
        ]
    }

    fn call_tool(&self, name: &str, arguments: &Value) -> Result<McpToolResult> {
        match name {
            "search_memory" => self.tool_search_memory(arguments),
            "list_memories" => self.tool_list_memories(arguments),
            "write_memory" => self.tool_write_memory(arguments),
            "update_memory" => self.tool_update_memory(arguments),
            "delete_memory" => self.tool_delete_memory(arguments),
            "record_prompt" => self.tool_record_prompt(arguments),
            _ => Ok(McpToolResult::error(format!("Unknown tool: {name}"))),
        }
    }

    fn tool_search_memory(&self, args: &Value) -> Result<McpToolResult> {
        let query = MemoryQuery {
            session_id: Some(self.active_project.clone()),
            text_query: args.get("query").and_then(|v| v.as_str()).map(String::from),
            kind: args.get("kind").and_then(|v| v.as_str()).and_then(MemoryObjectKind::from_str),
            scope: args.get("scope").and_then(|v| v.as_str()).and_then(MemoryScope::from_str),
            after: parse_datetime_arg(args, "after"),
            before: parse_datetime_arg(args, "before"),
            sort_by: parse_sort_by(args),
            limit: args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize,
            offset: args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        };

        let memories = self.store.query_memories(&query)?;
        let total_hint = if memories.len() == query.limit {
            "more results may be available — increase offset to paginate"
        } else {
            "end of results"
        };

        let results: Vec<Value> = memories.iter().map(|m| json!({
            "memory_id": m.memory_id.as_str(),
            "kind": m.kind.as_str(),
            "scope": m.scope.as_str(),
            "text": m.text,
            "importance": m.importance,
            "source_model": m.source_model,
            "created_at": m.created_at.to_rfc3339(),
        })).collect();

        let output = json!({
            "results": results,
            "count": results.len(),
            "offset": query.offset,
            "pagination": total_hint,
        });
        Ok(McpToolResult::text(serde_json::to_string_pretty(&output)?))
    }

    fn tool_list_memories(&self, args: &Value) -> Result<McpToolResult> {
        let query = MemoryQuery {
            session_id: Some(self.active_project.clone()),
            text_query: None,
            kind: args.get("kind").and_then(|v| v.as_str()).and_then(MemoryObjectKind::from_str),
            scope: args.get("scope").and_then(|v| v.as_str()).and_then(MemoryScope::from_str),
            after: None,
            before: None,
            sort_by: parse_sort_by(args),
            limit: args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize,
            offset: args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        };

        let memories = self.store.query_memories(&query)?;
        let total_hint = if memories.len() == query.limit {
            "more results may be available — increase offset to paginate"
        } else {
            "end of results"
        };

        let results: Vec<Value> = memories.iter().map(|m| json!({
            "memory_id": m.memory_id.as_str(),
            "kind": m.kind.as_str(),
            "scope": m.scope.as_str(),
            "text": m.text,
            "importance": m.importance,
            "created_at": m.created_at.to_rfc3339(),
        })).collect();

        let output = json!({
            "results": results,
            "count": results.len(),
            "offset": query.offset,
            "pagination": total_hint,
        });
        Ok(McpToolResult::text(serde_json::to_string_pretty(&output)?))
    }

    fn tool_write_memory(&self, args: &Value) -> Result<McpToolResult> {
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(McpToolResult::error("text parameter is required".into())),
        };
        let kind = match args.get("kind").and_then(|v| v.as_str()).and_then(MemoryObjectKind::from_str) {
            Some(k) => k,
            None => return Ok(McpToolResult::error(
                "kind parameter is required and must be one of: fact, constraint, decision, todo, user_fact, project_knowledge".into()
            )),
        };
        let scope = args.get("scope")
            .and_then(|v| v.as_str())
            .and_then(MemoryScope::from_str)
            .unwrap_or(MemoryScope::Project);
        let importance = args.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5).clamp(0.0, 1.0);
        let reason = args.get("reason").and_then(|v| v.as_str()).map(String::from);

        // Detect the source model from environment if available
        let source_model = std::env::var("CONTYNU_SOURCE_MODEL").ok();

        let memory_id = MemoryId::new();
        self.store.insert_memory_object(&MemoryObject {
            memory_id: memory_id.clone(),
            session_id: self.active_project.clone(),
            kind,
            scope,
            status: "active".into(),
            text: text.into(),
            importance,
            reason,
            source_model,
            superseded_by: None,
            created_at: Utc::now(),
            updated_at: None,
            access_count: 0,
            last_accessed_at: None,
        })?;

        Ok(McpToolResult::text(json!({
            "status": "ok",
            "memory_id": memory_id.as_str(),
            "message": "Memory stored successfully"
        }).to_string()))
    }

    fn tool_update_memory(&self, args: &Value) -> Result<McpToolResult> {
        let memory_id_str = match args.get("memory_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return Ok(McpToolResult::error("memory_id parameter is required".into())),
        };
        let memory_id = match MemoryId::parse(memory_id_str) {
            Ok(id) => id,
            Err(e) => return Ok(McpToolResult::error(format!("Invalid memory_id: {e}"))),
        };
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(McpToolResult::error("text parameter is required".into())),
        };
        let importance = args.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5).clamp(0.0, 1.0);
        let reason = args.get("reason").and_then(|v| v.as_str());

        self.store.update_memory_text(&memory_id, text, importance, reason)?;

        Ok(McpToolResult::text(json!({
            "status": "ok",
            "memory_id": memory_id.as_str(),
            "message": "Memory updated successfully"
        }).to_string()))
    }

    fn tool_delete_memory(&self, args: &Value) -> Result<McpToolResult> {
        let memory_id_str = match args.get("memory_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return Ok(McpToolResult::error("memory_id parameter is required".into())),
        };
        let memory_id = match MemoryId::parse(memory_id_str) {
            Ok(id) => id,
            Err(e) => return Ok(McpToolResult::error(format!("Invalid memory_id: {e}"))),
        };

        self.store.delete_memory(&memory_id)?;

        Ok(McpToolResult::text(json!({
            "status": "ok",
            "memory_id": memory_id.as_str(),
            "message": "Memory deleted"
        }).to_string()))
    }

    fn tool_record_prompt(&self, args: &Value) -> Result<McpToolResult> {
        let verbatim = match args.get("verbatim").and_then(|v| v.as_str()) {
            Some(v) if !v.is_empty() => v,
            _ => return Ok(McpToolResult::error("verbatim parameter is required".into())),
        };
        let interpretation = args.get("interpretation").and_then(|v| v.as_str()).map(String::from);
        let interpretation_confidence = args.get("interpretation_confidence").and_then(|v| v.as_f64());
        let source_model = std::env::var("CONTYNU_SOURCE_MODEL").ok();

        let prompt_id = format!("pmt_{}", uuid::Uuid::now_v7().simple());
        self.store.insert_prompt(&PromptRecord {
            prompt_id: prompt_id.clone(),
            session_id: self.active_project.clone(),
            verbatim: verbatim.into(),
            interpretation,
            interpretation_confidence,
            source_model,
            created_at: Utc::now(),
        })?;

        Ok(McpToolResult::text(json!({
            "status": "ok",
            "prompt_id": prompt_id,
            "message": "Prompt recorded"
        }).to_string()))
    }

    // -----------------------------------------------------------------------
    // Resources
    // -----------------------------------------------------------------------

    fn list_resources(&self) -> Vec<McpResource> {
        vec![
            McpResource {
                uri: "contynu://project/brief".into(),
                name: "Project Brief".into(),
                description: "Current project rehydration packet with mission, facts, decisions, and recent context.".into(),
                mime_type: "application/json".into(),
            },
        ]
    }

    fn read_resource(&self, uri: &str) -> Result<Value> {
        match uri {
            "contynu://project/brief" => {
                let manager = CheckpointManager::new(&self.state, &self.store, &self.blob_store);
                let packet = manager.build_packet(&self.active_project, None)?;
                Ok(json!({
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&packet)?,
                }))
            }
            _ => Err(crate::error::ContynuError::Validation(
                format!("Unknown resource URI: {uri}"),
            )),
        }
    }
}

fn parse_datetime_arg(args: &Value, key: &str) -> Option<DateTime<Utc>> {
    args.get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|dt| dt.and_utc())
                })
        })
}

fn parse_sort_by(args: &Value) -> MemorySortBy {
    match args.get("sort_by").and_then(|v| v.as_str()) {
        Some("recency") => MemorySortBy::Recency,
        _ => MemorySortBy::Importance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionRecord;

    fn setup_test_dispatcher() -> McpDispatcher {
        let dir = tempfile::tempdir().unwrap();
        let state = StatePaths::new(dir.path());
        state.ensure_layout().unwrap();
        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let blob_store = BlobStore::new(state.blobs_root());
        let project_id = ProjectId::new();

        store.register_session(&SessionRecord {
            session_id: project_id.clone(),
            project_id: None,
            status: "active".into(),
            cli_name: Some("claude_cli".into()),
            cli_version: None,
            model_name: None,
            cwd: Some("/tmp/test".into()),
            repo_root: None,
            host_fingerprint: None,
            started_at: chrono::Utc::now(),
            ended_at: None,
        }).unwrap();
        store.set_primary_project_id(&project_id).unwrap();

        // Insert test memories
        for (kind, scope, text, importance) in [
            (MemoryObjectKind::Fact, MemoryScope::Project, "The API uses JWT authentication", 0.8),
            (MemoryObjectKind::Fact, MemoryScope::Project, "Database is PostgreSQL 15", 0.7),
            (MemoryObjectKind::Decision, MemoryScope::Project, "Use HMAC-SHA256 for token signing", 0.85),
            (MemoryObjectKind::Constraint, MemoryScope::Project, "Must support backward compatibility", 0.9),
            (MemoryObjectKind::Todo, MemoryScope::Project, "Implement token refresh endpoint", 0.75),
            (MemoryObjectKind::Todo, MemoryScope::Project, "Add rate limiting", 0.75),
        ] {
            store.insert_memory_object(&MemoryObject {
                memory_id: crate::ids::MemoryId::new(),
                session_id: project_id.clone(),
                kind,
                scope,
                status: "active".into(),
                text: text.into(),
                importance,
                reason: None,
                source_model: None,
                superseded_by: None,
                created_at: chrono::Utc::now(),
                updated_at: None,
                access_count: 0,
                last_accessed_at: None,
            }).unwrap();
        }

        McpDispatcher::from_parts(store, state, blob_store, project_id)
    }

    #[test]
    fn initialize_returns_capabilities() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "initialize".into(),
            params: json!({}),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
    }

    #[test]
    fn tools_list_returns_all_tools() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/list".into(),
            params: json!({}),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 6);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_memory"));
        assert!(names.contains(&"list_memories"));
        assert!(names.contains(&"write_memory"));
        assert!(names.contains(&"update_memory"));
        assert!(names.contains(&"delete_memory"));
        assert!(names.contains(&"record_prompt"));
    }

    #[test]
    fn search_memory_finds_results() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "tools/call".into(),
            params: json!({"name": "search_memory", "arguments": {"query": "JWT"}}),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("JWT authentication"));
    }

    #[test]
    fn list_memories_by_kind_filters_correctly() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(4)),
            method: "tools/call".into(),
            params: json!({"name": "list_memories", "arguments": {"kind": "decision"}}),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let text = resp.result.unwrap()["content"][0]["text"].as_str().unwrap().to_string();
        assert!(text.contains("HMAC-SHA256"));
        assert!(!text.contains("JWT authentication"));
    }

    #[test]
    fn write_memory_creates_new_memory() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(5)),
            method: "tools/call".into(),
            params: json!({
                "name": "write_memory",
                "arguments": {
                    "text": "The deploy pipeline uses GitHub Actions",
                    "kind": "fact",
                    "importance": 0.8,
                    "reason": "Observed during CI review"
                }
            }),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("ok"));

        // Verify it's searchable
        let search_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(6)),
            method: "tools/call".into(),
            params: json!({"name": "search_memory", "arguments": {"query": "GitHub Actions"}}),
        };
        let search_resp = dispatcher.handle_request(&search_req).unwrap();
        let search_text = search_resp.result.unwrap()["content"][0]["text"].as_str().unwrap().to_string();
        assert!(search_text.contains("GitHub Actions"));
    }

    #[test]
    fn resources_list_returns_resources() {
        let dispatcher = setup_test_dispatcher();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(7)),
            method: "resources/list".into(),
            params: json!({}),
        };
        let resp = dispatcher.handle_request(&req).unwrap();
        let resources = resp.result.unwrap()["resources"].as_array().unwrap().clone();
        assert_eq!(resources.len(), 1);
    }
}
