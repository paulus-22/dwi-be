//! DWI backend: admin API, scanner sync, live updates, photo uploads.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod live;
pub mod redis_ext;
pub mod routes;
pub mod seed;
pub mod state;
pub mod storage;
pub mod views;

pub use config::Config;
pub use state::AppState;
