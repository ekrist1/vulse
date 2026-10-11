mod blueprint;
mod query;
pub use query::{
    RecordFilter, RecordPage, RecordQuery, SavedView, SavedViewWrite, display_value,
    validate_query, validate_view,
};
mod formula;
pub use formula::{
    Expr as FormulaExpression, FUNCTIONS, calculate_document, formula_dependencies,
    inspect_formula, source_document, text_value, validate_formulas,
};
mod identity;
mod portable_text;

pub use blueprint::{
    ArrayItems, Blueprint, CheckedDocument, DocumentRef, FieldDef, FieldType, record_title,
    reference_collections, validate_blueprint, validate_document,
};
pub use identity::{
    check_collection_name, check_email, check_handle, check_key_name, check_password, is_uuid,
    page_bounds,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub handle: String,
    pub password: String,
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
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiKey {
    pub name: String,
    #[serde(default)]
    pub collection_id: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentRevision {
    pub id: String,
    pub document_id: String,
    pub action: String,
    pub body: serde_json::Value,
    pub blueprint: Blueprint,
    pub record_created_at: String,
    pub created_at: String,
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
}
