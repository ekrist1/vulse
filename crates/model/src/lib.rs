mod blueprint;
mod query;
pub use query::{
    RecordFilter, RecordPage, RecordQuery, SavedView, SavedViewWrite, display_value, remove_filter,
    validate_query, validate_view,
};
mod formula;
pub use formula::{
    Expr as FormulaExpression, FUNCTIONS, call_function, LookupColumn, Lookups, calculate_document,
    calculate_document_with, formula_dependencies, formula_lookups, inspect_formula,
    lookup_references, rename_formula_fields, rename_formula_references, source_document,
    text_value, validate_formulas,
};
mod identity;
mod portable_text;
mod summary;
mod vsql;
pub use vsql::{
    MAX_SQL_CHANGES, MAX_SQL_ROWS, MAX_SQL_SAMPLE, SQL_AGGREGATES, SQL_KEYWORDS, SQL_RECORD_COLUMNS, Select as SqlSelect, SqlChange,
    SqlColumn, SqlError, SqlExpr, SqlRequest, SqlResult, SqlRow, Statement as SqlStatement,
    parse_sql, run_select, sql_assignments, sql_matches,
};
pub use summary::{
    ChartKind, DateBucket, MAX_CHART_SERIES, MAX_GROUP_FIELDS, MAX_MEASURES, MAX_SUMMARY_GROUPS,
    SummaryChart, SummaryGroup, SummaryMeasure, SummaryOp, SummaryRequest, SummaryResult,
    SummaryRow, SummarySpec, apply_summary_formulas, lookup_message, rename_summary_fields, validate_summary,
};

pub use blueprint::{
    ArrayItems, Blueprint, CheckedDocument, DocumentRef, FieldDef, FieldType, MAX_FIELD_OPTIONS,
    MAX_FORMAT_DECIMALS, MAX_OPTION_LENGTH, NumberFormat, match_headers, record_title,
    reference_collections, take_field_renames,
    take_option_renames, validate_blueprint, validate_document,
};
pub use identity::{
    check_collection_name, check_email, check_handle, check_key_name, check_password,
    field_name_from, is_date, is_field_name, is_uuid, page_bounds,
};
pub use portable_text::{
    BlockStyle, EditorBlock, ListItem, Mark, MarkRange, clamp_marks, editor_blocks_from_json,
    editor_blocks_to_json, normalize_marks, toggle_mark, utf16_to_char_index,
    validate_portable_text,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
}

/// Workspace roles, ordered from least to most access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Reads collections, records and assets.
    Viewer,
    /// Also writes records, assets, imports and saved views.
    Editor,
    /// Also changes collection schemas and manages users.
    Admin,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Editor, Role::Admin];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Viewer => "Viewer",
            Role::Editor => "Editor",
            Role::Admin => "Admin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub handle: String,
    pub role: Role,
}

/// A workspace account as seen by admins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedUser {
    pub id: String,
    pub email: String,
    pub handle: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateUser {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupStatus {
    pub needs_setup: bool,
}

/// Creates the first admin. Only accepted while the workspace has no accounts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupRequest {
    pub token: String,
    pub email: String,
    pub handle: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkPurpose {
    Invite,
    Reset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvite {
    pub email: String,
    pub role: Role,
}

/// A pending single-use invite or password reset link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InviteView {
    pub id: String,
    pub purpose: LinkPurpose,
    pub email: String,
    pub role: Role,
    pub created_at: String,
    pub expires_at: String,
}

/// The raw token is returned once. The admin shares `path` with the recipient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedInvite {
    #[serde(flatten)]
    pub invite: InviteView,
    pub token: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkTokenRequest {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub purpose: LinkPurpose,
    pub email: String,
}

/// Completes an invite (handle required) or a password reset (handle ignored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptLink {
    pub token: String,
    #[serde(default)]
    pub handle: Option<String>,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionView {
    pub id: String,
    pub name: String,
    pub visibility: Visibility,
    pub blueprint: Blueprint,
    pub created_at: String,
    pub updated_at: String,
    /// How many records the collection holds, when listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCollection {
    pub name: String,
    pub visibility: Visibility,
    pub blueprint: Blueprint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCollection {
    #[serde(default)]
    pub expected_updated_at: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
    #[serde(default)]
    pub blueprint: Option<Blueprint>,
    /// Delete the stored values of fields the new blueprint no longer has.
    /// Without it, removing a field that holds values is refused.
    #[serde(default)]
    pub remove_field_data: bool,
    /// Convert stored values of fields whose type changes, such as text
    /// holding "12" becoming the number 12. Values that cannot be converted
    /// refuse the whole change. Without it, any stored value of the old type
    /// refuses the change.
    #[serde(default)]
    pub convert_field_values: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentView {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub calculation_errors: std::collections::BTreeMap<String, String>,
    pub id: String,
    pub body: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentWrite {
    pub body: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPatch {
    pub patch: serde_json::Value,
    #[serde(default)]
    pub expected_updated_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkWrite {
    pub action: String,
    pub ids: Vec<String>,
    #[serde(default)]
    pub patch: serde_json::Value,
    #[serde(default)]
    pub expected: std::collections::BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResult {
    pub documents: Vec<DocumentView>,
    pub affected: usize,
    pub undo_id: String,
}

/// A backup archive in the server's backup directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupView {
    pub name: String,
    pub bytes: u64,
    /// When the backup was made, as RFC 3339 UTC.
    pub created_at: String,
}

/// The backup page: where backups go, the schedule, and the archives there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupOverview {
    pub dir: String,
    /// Hours between scheduled backups; `None` when scheduling is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_hours: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
    /// Newest first.
    pub backups: Vec<BackupView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetView {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub byte_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    pub role: Role,
    pub created_at: String,
}

/// What the consent page shows about a pending OAuth sign-in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthRequestView {
    pub client_name: String,
    /// For apps identified by a metadata document URL, that URL's host,
    /// which vouches for the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_domain: Option<String>,
    /// Where the browser returns after consent, such as `claude.ai`.
    pub redirect_host: String,
    pub write_requested: bool,
    /// False when the signed-in user is a viewer.
    pub write_allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthConsent {
    pub request: String,
    pub approve: bool,
    /// Grant record writes, when requested and allowed.
    #[serde(default)]
    pub write: bool,
    /// Limit the grant to one collection.
    #[serde(default)]
    pub collection_id: Option<String>,
}

/// Whether any MCP client may connect, or only ones an admin approved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthSettings {
    pub open_registration: bool,
}

/// An MCP client known to Vulse, for admins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientView {
    /// A generated id, or the URL of the client's metadata document.
    pub id: String,
    pub name: String,
    /// Hosts the client returns users to after sign-in.
    pub redirect_hosts: Vec<String>,
    pub approved: bool,
    /// Identified by a metadata document URL rather than registered.
    pub metadata_document: bool,
    /// Users currently connected through it.
    pub connections: u64,
    pub created_at: String,
}

/// Where the browser goes next to finish an OAuth sign-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthRedirect {
    pub redirect: String,
}

/// An app connected through OAuth, such as Claude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthGrantView {
    pub id: String,
    pub client_name: String,
    pub write: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
    pub created_at: String,
    /// When the app last signed in or renewed its access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// The owner, in an admin's workspace-wide list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKey {
    pub name: String,
    #[serde(default)]
    pub collection_id: Option<String>,
    /// Defaults to the creator's role and may not exceed it.
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedApiKey {
    #[serde(flatten)]
    pub key: ApiKeyView,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportIssue {
    pub row: usize,
    pub field: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportResult {
    #[serde(default)]
    pub created: usize,
    #[serde(default)]
    pub updated: usize,
    #[serde(default)]
    pub unchanged: usize,
    #[serde(default)]
    pub preview_actions: Vec<String>,
    pub headers: Vec<String>,
    pub sheets: Vec<String>,
    pub sheet: String,
    pub mapping: Vec<Option<String>>,
    pub row_count: usize,
    pub preview: Vec<serde_json::Value>,
    pub errors: Vec<ImportIssue>,
    pub imported: usize,
}

/// Fields suggested from a spreadsheet's columns, for review before they
/// are added to a collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSuggestions {
    pub sheets: Vec<String>,
    pub sheet: String,
    pub row_count: usize,
    pub columns: Vec<SuggestedField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestedField {
    /// The column header as written in the file.
    pub header: String,
    /// A valid field name made from the header, unique among the columns.
    pub name: String,
    /// The type every nonempty cell in the column can be imported as.
    #[serde(rename = "type")]
    pub kind: FieldType,
    /// Up to three distinct values from the column.
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRevision {
    pub id: String,
    pub document_id: String,
    pub action: String,
    pub body: serde_json::Value,
    pub blueprint: Blueprint,
    pub record_created_at: String,
    pub created_at: String,
    /// Set when the change did not come from the web app or REST API, such as
    /// `mcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreRevision {
    #[serde(default)]
    pub expected_updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceView {
    pub collection_id: String,
    pub collection_name: String,
    pub document_id: String,
    pub title: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    #[serde(default)]
    pub query: RecordQuery,
    pub format: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub ids: Vec<String>,
    /// Columns to include from records that reference fields point to.
    #[serde(default)]
    pub related: Vec<RelatedColumns>,
}

/// For one reference field (or list of references), the fields of the
/// referenced records to export. CSV adds `field.column` columns; JSON adds
/// a `related` object keyed by field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelatedColumns {
    pub field: String,
    pub columns: Vec<String>,
}
