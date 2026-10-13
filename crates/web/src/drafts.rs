use serde::{Serialize, de::DeserializeOwned};
fn storage() -> Result<web_sys::Storage, String> {
    web_sys::window()
        .ok_or("Browser storage unavailable")?
        .local_storage()
        .map_err(|_| "Browser storage unavailable")?
        .ok_or_else(|| "Browser storage unavailable".into())
}
pub fn read<T: DeserializeOwned>(key: &str) -> Result<Option<T>, String> {
    storage()?
        .get_item(key)
        .map_err(|_| "Could not read the saved draft".to_string())?
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|_| "This draft cannot be read. Discard it to start again.".into())
        })
        .transpose()
}
pub fn write<T: Serialize>(key: &str, value: &T) -> Result<(), String> {
    storage()?
        .set_item(
            key,
            &serde_json::to_string(value).map_err(|e| e.to_string())?,
        )
        .map_err(|_| {
            "Could not save a draft on this device. Keep this page open until you save.".into()
        })
}
pub fn clear(key: &str) -> Result<(), String> {
    storage()?
        .remove_item(key)
        .map_err(|_| "Could not remove the saved draft".into())
}
/// Remove a reverted edit without destroying an unreadable recovery draft.
pub fn clear_readable<T: DeserializeOwned>(key: &str) -> Result<(), String> {
    if read::<T>(key)?.is_some() {
        clear(key)?;
    }
    Ok(())
}
#[derive(Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct RecordDraft {
    pub blueprint: vulse_model::Blueprint,
    pub inputs: Vec<crate::form::ValueInput>,
    pub baseline: Vec<crate::form::ValueInput>,
    pub version: Option<String>,
}
#[derive(Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct SchemaDraft {
    pub name: String,
    pub visibility: vulse_model::Visibility,
    pub blueprint: vulse_model::Blueprint,
    pub version: String,
}
