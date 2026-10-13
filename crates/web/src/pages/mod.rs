mod account;
pub(crate) mod auth;
mod collection;
mod collections;
mod document;
mod keys;
mod people;
mod sql;

pub use account::AccountPage;
pub use auth::{AcceptPage, LoginPage, SetupPage};
pub use collection::CollectionPage;
pub use collections::CollectionsPage;
pub use document::DocumentPage;
pub use keys::KeysPage;
pub use people::PeoplePage;
pub use sql::SqlPage;
