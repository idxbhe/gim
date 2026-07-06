//! `gim` — Game Files Version Control Tool.
//!
//! A CLI tool for versioning game files. Similar to git, but
//! purpose-built for game directories. Uses SQLite for metadata and
//! XXH3 for fast file hashing.
//!
//! This crate is structured as a library with a thin `main.rs` binary
//! that wires CLI parsing to command implementations.

pub mod cli;
pub mod commands;
pub mod config;
pub mod db;
pub mod error;
pub mod hashing;
pub mod ignore_mod;
pub mod locking;
pub mod output;
pub mod parallel;
pub mod path_utils;
pub mod storage;
pub mod walker;
