mod api;
mod app;
mod collection_api;
mod confirm;
mod drafts;
mod editor;
mod form;
mod formula_builder;
mod history;
mod import;
mod pages;
mod record_cell;
mod records;
mod relationships;
mod summary;

fn main() {
    yew::Renderer::<app::App>::new().render();
}
