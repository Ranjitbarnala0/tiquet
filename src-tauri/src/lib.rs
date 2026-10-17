pub mod api;
pub mod auth;
pub mod backup;
pub mod codepage;
pub mod db;
pub mod error;
pub mod escpos;
pub mod layout;
pub mod money;
pub mod printer;
pub mod products;
pub mod reports;
pub mod settings;
pub mod state;
pub mod tickets;
pub mod users;

mod app;

pub use app::run;
