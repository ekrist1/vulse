//! The MCP server at `/mcp`. Tools call the same functions as the REST API,
//! so roles, collection scopes and calculated values behave identically.

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use vulse_model::{CollectionView, DocumentView, FieldType, RecordFilter, RecordQuery, Role};

use crate::auth::{self, Principal, require_collection, require_role};
use crate::content;
use crate::db::Conn;
use crate::error::AppError;
use crate::mutations::{self, RecordChange};
use crate::state::AppState;
use crate::store;

/// Records returned by one `query_records` call unless the caller asks for
/// fewer. The query validator allows at most 100.
const DEFAULT_LIMIT: u32 = 25;
const MAX_COLLECTIONS: u32 = 100;
/// Rows of a `run_sql` result sent back; the rest are counted, not shown.
const MAX_SQL_ROWS: usize = 200;

const INSTRUCTIONS: &str = "Vulse stores records in collections. Call list_collections to see \
them, then describe_collection before querying or writing so you know the field names and \
types. Field names are case-sensitive. Calculated fields are read-only and are filled in on \
every read. Each write returns an undo_id; undo_action reverts that write within 24 hours.";

/// `/mcp`, reachable only with a bearer credential.
pub fn router(state: &AppState) -> Router<AppState> {
    Router::new()
        .nest_service("/mcp", service(state))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            authorize,
        ))
}

fn service(state: &AppState) -> StreamableHttpService<VulseMcp, LocalSessionManager> {
    let mut hosts = vec!["localhost".to_string(), "127.0.0.1".into(), "::1".into()];
    let mut origins = Vec::new();
    if let Some(origin) = &state.auth.public_origin {
        if let Some(host) = origin.split_once("://").map(|(_, rest)| rest) {
            hosts.push(host.to_string());
        }
        origins.push(origin.clone());
    }
    // Sessions are not kept: every request carries its own credential, and
    // any server instance can answer it.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins)
        .enforce_origin_validation();
    let state = state.clone();
    StreamableHttpService::new(
        move || Ok(VulseMcp::new(state.clone())),
        Default::default(),
        config,
    )
}

/// Resolves the bearer credential and hands the principal to the tools.
async fn authorize(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    match auth::bearer_principal(&state, &parts).await {
        Ok(principal) => {
            parts.extensions.insert(principal);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(err) => {
            let mut response = err.into_response();
            if response.status() == axum::http::StatusCode::UNAUTHORIZED {
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, crate::oauth::challenge(&state));
            }
            response
        }
    }
}

#[derive(Clone)]
pub struct VulseMcp {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl VulseMcp {
    fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    async fn conn(&self) -> Result<Conn, AppError> {
        let conn = self.state.db.connect().await?;
        conn.set_source("mcp");
        Ok(conn)
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListCollections {
    /// Collections to skip, from `next_offset` of the previous call.
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DescribeCollection {
    /// Collection id or name.
    pub collection: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRecord {
    /// Collection id or name.
    pub collection: String,
    /// Record id.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryRecords {
    /// Collection id or name.
    pub collection: String,
    /// Free-text search across the record's text.
    #[serde(default)]
    pub search: Option<String>,
    /// Conditions on fields, at most 20.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Field to sort by, or `created_at`, `updated_at` or `id`. Defaults to
    /// `created_at`.
    #[serde(default)]
    pub sort: Option<String>,
    /// Sort from high to low. Defaults to true.
    #[serde(default)]
    pub descending: Option<bool>,
    /// Records to return, 1 to 100. Defaults to 25.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Records to skip, from `next_offset` of the previous call.
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Filter {
    /// Field name, as listed by describe_collection.
    pub field: String,
    pub op: FilterOp,
    /// Compared value. Numbers for number fields, `true`/`false` for boolean
    /// fields, `YYYY-MM-DD` for date fields. Omit for `empty`/`not_empty`.
    #[serde(default)]
    pub value: Value,
    /// Join this filter to the previous one with OR instead of AND. Filters
    /// joined by OR form a group, and every group must match, so
    /// `a, or b, c` means `(a OR b) AND c`.
    #[serde(default)]
    pub or: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateRecords {
    /// Collection id or name.
    pub collection: String,
    /// 1 to 100 records, each an object of field name to value.
    pub records: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRecords {
    /// Collection id or name.
    pub collection: String,
    /// 1 to 100 changes, each to a different record.
    pub updates: Vec<RecordUpdate>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordUpdate {
    /// Record id.
    pub id: String,
    /// The fields to change, as field name to new value. Fields left out keep
    /// their value; `null` clears a field.
    pub fields: Map<String, Value>,
    /// The record's `updated_at` as you last read it. The update is refused
    /// if the record changed since.
    #[serde(default)]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteRecords {
    /// Collection id or name.
    pub collection: String,
    /// 1 to 100 record ids.
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunSql {
    /// One SELECT statement on one collection.
    pub sql: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UndoAction {
    /// Collection id or name.
    pub collection: String,
    /// The `undo_id` returned by the write to revert.
    pub undo_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    /// Text contains the value, ignoring case.
    Contains,
    Eq,
    Neq,
    /// Greater than. Number and date fields only; the same for gte, lt, lte.
    Gt,
    Gte,
    Lt,
    Lte,
    Empty,
    NotEmpty,
}

impl FilterOp {
    fn as_str(self) -> &'static str {
        match self {
            FilterOp::Contains => "contains",
            FilterOp::Eq => "eq",
            FilterOp::Neq => "neq",
            FilterOp::Gt => "gt",
            FilterOp::Gte => "gte",
            FilterOp::Lt => "lt",
            FilterOp::Lte => "lte",
            FilterOp::Empty => "empty",
            FilterOp::NotEmpty => "not_empty",
        }
    }
}

#[tool_router]
impl VulseMcp {
    #[tool(
        description = "List the collections you can read, with their ids and field names. \
        Returns at most 100; pass next_offset to continue.",
        annotations(read_only_hint = true)
    )]
    async fn list_collections(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<ListCollections>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = principal(&parts)?;
            let offset = input.offset.unwrap_or(0);
            let conn = self.conn().await?;
            let collections =
                content::visible_collections(&conn, principal, MAX_COLLECTIONS, offset).await?;
            let next_offset =
                (collections.len() as u32 == MAX_COLLECTIONS).then_some(offset + MAX_COLLECTIONS);
            let items: Vec<Value> = collections
                .iter()
                .map(|collection| {
                    json!({
                        "id": collection.id,
                        "name": collection.name,
                        "fields": collection
                            .blueprint
                            .fields
                            .iter()
                            .map(|field| field.name.as_str())
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({ "collections": items, "next_offset": next_offset }))
        })
        .await
    }

    #[tool(
        description = "Describe a collection's fields: name, type, whether it is required, \
        allowed values, the collection a reference points to, and formulas of calculated \
        (read-only) fields. Call this before querying a collection.",
        annotations(read_only_hint = true)
    )]
    async fn describe_collection(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<DescribeCollection>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = principal(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            Ok(json!({
                "id": collection.id,
                "name": collection.name,
                "title_field": collection.blueprint.title_field,
                "fields": collection.blueprint.fields,
            }))
        })
        .await
    }

    #[tool(
        description = "Find records in a collection with optional search text, field filters, \
        sorting and paging. Returns the matching total and one page of records; pass \
        next_offset to read the next page.",
        annotations(read_only_hint = true)
    )]
    async fn query_records(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<QueryRecords>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = principal(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let defaults = RecordQuery::default();
            let query = RecordQuery {
                search: input.search.unwrap_or_default(),
                filters: input
                    .filters
                    .into_iter()
                    .map(|filter| RecordFilter {
                        field: filter.field,
                        op: filter.op.as_str().into(),
                        value: filter.value,
                        or: filter.or,
                    })
                    .collect(),
                sort_field: input.sort.unwrap_or(defaults.sort_field),
                descending: input.descending.unwrap_or(defaults.descending),
                limit: input.limit.unwrap_or(DEFAULT_LIMIT),
                offset: input.offset.unwrap_or(0),
                ..defaults
            };
            let page = crate::records::run_query(&conn, &collection, &query).await?;
            let end = page.offset as u64 + page.items.len() as u64;
            Ok(json!({
                "total": page.total,
                "records": page.items.iter().map(record).collect::<Vec<_>>(),
                "next_offset": (end < page.total).then_some(end),
            }))
        })
        .await
    }

    #[tool(
        description = "Read one record by id.",
        annotations(read_only_hint = true)
    )]
    async fn get_record(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<GetRecord>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = principal(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let document = content::read_document(&conn, &collection, &input.id)
                .await
                .map_err(|err| match err {
                    AppError::NotFound => AppError::bad(format!(
                        "no record {} in collection {}",
                        input.id, collection.name
                    )),
                    err => err,
                })?;
            Ok(record(&document))
        })
        .await
    }

    #[tool(
        description = "Create 1 to 100 records in a collection. All are created or none: if \
        any record is invalid, nothing is saved and the error names the problem. Values of \
        calculated fields are ignored. Returns the new records and an undo_id.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn create_records(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<CreateRecords>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = editor(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let mut ignored: Vec<&str> = collection
                .blueprint
                .fields
                .iter()
                .filter(|field| field.kind == FieldType::Calculated)
                .map(|field| field.name.as_str())
                .filter(|name| {
                    input
                        .records
                        .iter()
                        .any(|record| record.contains_key(*name))
                })
                .collect();
            ignored.sort();
            let bodies = input.records.iter().cloned().map(Value::Object).collect();
            let result = mutations::run_create(&conn, principal, &collection.id, bodies).await?;
            let mut out = json!({
                "created": result.affected,
                "records": result.documents.iter().map(record).collect::<Vec<_>>(),
                "undo_id": result.undo_id,
            });
            if !ignored.is_empty() {
                out["ignored_calculated_fields"] = json!(ignored);
            }
            Ok(out)
        })
        .await
    }

    #[tool(
        description = "Change fields of 1 to 100 existing records; each update names a record \
        and only the fields to change. All updates are saved or none. Calculated fields cannot \
        be set. Returns the updated records and an undo_id.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn update_records(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<UpdateRecords>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = editor(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let changes = input
                .updates
                .into_iter()
                .map(|update| RecordChange {
                    id: update.id,
                    patch: Value::Object(update.fields),
                    expected: update.expected_updated_at,
                })
                .collect();
            let result =
                mutations::run_changes(&conn, principal, &collection.id, "update", changes)
                    .await
                    .map_err(|err| missing_record(err, &collection))?;
            Ok(json!({
                "updated": result.affected,
                "records": result.documents.iter().map(record).collect::<Vec<_>>(),
                "undo_id": result.undo_id,
            }))
        })
        .await
    }

    #[tool(
        description = "Delete 1 to 100 records. All are deleted or none; records that other \
        records still reference cannot be deleted. Deleted records stay in the collection's \
        trash and can be brought back with undo_action or from the trash in Vulse.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn delete_records(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<DeleteRecords>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = editor(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let changes = input
                .ids
                .into_iter()
                .map(|id| RecordChange {
                    id,
                    patch: Value::Null,
                    expected: None,
                })
                .collect();
            let result =
                mutations::run_changes(&conn, principal, &collection.id, "delete", changes)
                    .await
                    .map_err(|err| missing_record(err, &collection))?;
            Ok(json!({ "deleted": result.affected, "undo_id": result.undo_id }))
        })
        .await
    }

    #[tool(
        description = "Revert a create, update or delete made in the last 24 hours, using the \
        undo_id it returned. Undoing a create deletes those records. Refused if an affected \
        record has changed since.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn undo_action(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<UndoAction>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = editor(&parts)?;
            let conn = self.conn().await?;
            let collection = find_collection(&conn, principal, &input.collection).await?;
            let result = mutations::run_undo(&conn, principal, &collection.id, &input.undo_id)
                .await
                .map_err(|err| match err {
                    AppError::NotFound => AppError::bad(format!(
                        "no undo_id {} in collection {}; it may already be undone",
                        input.undo_id, collection.name
                    )),
                    err => err,
                })?;
            Ok(json!({
                "undone": result.affected,
                "records": result.documents.iter().map(record).collect::<Vec<_>>(),
            }))
        })
        .await
    }

    #[tool(
        description = "Run one read-only Vulse SQL SELECT on one collection, for counts, sums, \
        averages and grouping that query_records cannot do. Syntax: SELECT columns or \
        aggregates (COUNT(*), SUM, AVG, MIN, MAX) FROM <collection> [WHERE ...] [GROUP BY ...] \
        [ORDER BY ...] [LIMIT n] [OFFSET n]. Fields are the collection's field names plus id, \
        created_at and updated_at. Conditions: = <> < <= > >=, AND, OR, NOT, IN (...), \
        BETWEEN, LIKE (case-insensitive, % and _), IS NULL. Text values use single quotes; \
        quote names that are keywords with double quotes. No joins; UPDATE and DELETE are \
        refused. Returns at most 200 rows.",
        annotations(read_only_hint = true)
    )]
    async fn run_sql(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<RunSql>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(async {
            let principal = principal(&parts)?;
            let conn = self.conn().await?;
            let mut result = crate::sql::select_only(&conn, principal, &input.sql).await?;
            let truncated = result.truncated || result.rows.len() > MAX_SQL_ROWS;
            result.rows.truncate(MAX_SQL_ROWS);
            Ok(json!({
                "collection": result.collection,
                "columns": result.columns,
                "rows": result.rows,
                "total": result.total,
                "truncated": truncated,
            }))
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for VulseMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("vulse", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

fn principal(parts: &Parts) -> Result<&Principal, AppError> {
    parts
        .extensions
        .get::<Principal>()
        .ok_or(AppError::Unauthorized)
}

fn editor(parts: &Parts) -> Result<&Principal, AppError> {
    let principal = principal(parts)?;
    require_role(principal, Role::Editor)?;
    Ok(principal)
}

fn missing_record(err: AppError, collection: &CollectionView) -> AppError {
    match err {
        AppError::NotFound => AppError::bad(format!(
            "a record id was not found in collection {}",
            collection.name
        )),
        err => err,
    }
}

/// A collection by id or name, if the principal may read it.
async fn find_collection(
    conn: &Conn,
    principal: &Principal,
    key: &str,
) -> Result<CollectionView, AppError> {
    let key = key.trim();
    let found = match store::find_collection(conn, key).await? {
        Some(collection) => Some(collection),
        None => store::find_collection_by_name(conn, &key.to_ascii_lowercase()).await?,
    };
    let collection = found.ok_or_else(|| {
        AppError::bad(format!(
            "no collection named {key}; call list_collections to see them"
        ))
    })?;
    require_collection(principal, &collection.id)?;
    Ok(collection)
}

fn record(document: &DocumentView) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(document.id));
    out.insert("fields".into(), document.body.clone());
    if !document.calculation_errors.is_empty() {
        out.insert(
            "calculation_errors".into(),
            json!(document.calculation_errors),
        );
    }
    out.insert("created_at".into(), json!(document.created_at));
    out.insert("updated_at".into(), json!(document.updated_at));
    Value::Object(out)
}

/// Problems the model can act on become error results it can read; failures
/// inside the server are logged and reported without detail.
async fn reply(
    result: impl Future<Output = Result<Value, AppError>>,
) -> Result<CallToolResult, ErrorData> {
    match result.await {
        Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(
            value.to_string(),
        )])),
        Err(AppError::Internal(err)) => {
            tracing::error!(error = %err, "mcp tool failed");
            Err(ErrorData::internal_error("internal error", None))
        }
        Err(AppError::Conflict {
            message,
            document_ids,
        }) if !document_ids.is_empty() => Ok(CallToolResult::error(vec![ContentBlock::text(
            format!("{message} Records: {}", document_ids.join(", ")),
        )])),
        Err(err) => Ok(CallToolResult::error(vec![ContentBlock::text(
            err.to_string(),
        )])),
    }
}
