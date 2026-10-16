mod api;
mod app;
mod collection_api;
mod confirm;
mod drafts;
mod editor;
mod field_import;
mod form;
mod formula_builder;
mod formula_input;
mod history;
mod import;
mod live;
mod lookup_builder;
mod pages;
mod record_cell;
mod records;
mod relationships;
mod sql_editor;
mod summary;

fn main() {
    yew::Renderer::<app::App>::new().render();
}
