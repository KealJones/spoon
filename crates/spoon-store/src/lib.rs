//! Persistence for the concept substrate.
//!
//! The store is a flat bag of concepts indexed several ways: by structure (the
//! content id), by head symbol (so "find all friendships" works), and by
//! participant (so "find everything about Greg" works). A compound does not
//! belong to its head or to any of its arguments. It exists on its own and all
//! of them can be found through it.
//!
//! # Ground concepts cost nothing until something is said about them
//!
//! A named concept's meaning lives here: surface forms, realizations,
//! relationships. The name alone tells you nothing, so it always earns a row.
//! A ground concept's identity IS its value, so it is self-describing and
//! needs no row at all. Evaluating `Add<42, 1>` writes zero rows. Asserting
//! `Synonym<"forty-two", 42>` materializes one for 42, and from that moment 42
//! is queryable exactly like `Greg` is. That asymmetry is what keeps every
//! integer in every computation out of the database while still letting `#160`
//! become a real pull request concept.
//!
//! # What each write does
//!
//! - [`Store::put_concept`] records structure. It is the "I have seen this"
//!   path and it skips bare ground atomics.
//! - [`Store::assert_concept`] records belief, with provenance and a
//!   bi-temporal validity window. It forces a row for whatever it targets.
//! - [`Store::put_meta`] records description: names, activation, tier. It also
//!   forces a row.
//!
//! Nothing is ever deleted. Retraction sets `invalidated_at`, because
//! something that was true does not become false merely because it is no
//! longer true now.

mod assertions;
mod concepts;
mod encode;
mod episodes;
mod error;
mod meta;
pub mod pairs;
mod realizations;
mod seed;
mod symbols;

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use assertions::{AssertionId, AssertionRecord};
pub use error::{Result, StoreError};
pub use schema::SCHEMA_VERSION;
pub use seed::{ImportStats, Seed, SeedAssertion, SeedSymbol};

mod schema;

/// A single-file brain.
///
/// The connection sits behind a mutex rather than a pool because sqlite
/// serializes writers anyway, and one lock at the top is far easier to reason
/// about than a connection per caller with WAL snapshots drifting apart. Every
/// public method takes `&self`, so a `Store` can be shared as an `Arc`.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) a brain file and bring its schema up to date.
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        Store::from_connection(conn)
    }

    /// An ephemeral brain. Used by tests and by the doctor when it needs a
    /// scratch database that must not touch the real one.
    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        Store::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> Result<Store> {
        schema::apply_pragmas(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    /// Schema version recorded in the database. Equals [`SCHEMA_VERSION`]
    /// after a successful open; exposed so the doctor can report it.
    pub fn schema_version(&self) -> Result<u32> {
        let conn = self.conn.lock();
        let raw: String = conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [schema::SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )?;
        raw.parse::<u32>()
            .map_err(|_| StoreError::corrupt("meta", format!("schema_version is {raw:?}")))
    }
}
