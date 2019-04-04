extern crate owning_ref;
extern crate sodiumoxide;
extern crate r2d2;
extern crate r2d2_postgres;
// TODO remove log when done
extern crate log;

use postgres;
use self::r2d2_postgres::{TlsMode, PostgresConnectionManager};
use serde_json;

use self::owning_ref::OwningHandle;
use std::rc::Rc;
use std::time::Duration;

use errors::wallet::WalletStorageError;
use errors::common::CommonError;
use wql::language;
use wql::query;
use wql::transaction;

use wql::storage::{StorageIterator, WalletStorage, StorageRecord, EncryptedValue, Tag, TagName};

fn default_true() -> bool { true }

fn default_false() -> bool { false }

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordOptions {
    #[serde(default = "default_false")]
    retrieve_type: bool,
    #[serde(default = "default_true")]
    retrieve_value: bool,
    #[serde(default = "default_false")]
    retrieve_tags: bool
}

impl RecordOptions {
    pub fn id() -> String {
        let options = RecordOptions {
            retrieve_type: false,
            retrieve_value: false,
            retrieve_tags: false
        };

        serde_json::to_string(&options).unwrap()
    }

    pub fn id_value() -> String {
        let options = RecordOptions {
            retrieve_type: false,
            retrieve_value: true,
            retrieve_tags: false
        };

        serde_json::to_string(&options).unwrap()
    }
}

impl Default for RecordOptions {
    fn default() -> RecordOptions {
        RecordOptions {
            retrieve_type: false,
            retrieve_value: true,
            retrieve_tags: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default = "default_true")]
    retrieve_records: bool,
    #[serde(default = "default_false")]
    retrieve_total_count: bool,
    #[serde(default = "default_false")]
    retrieve_type: bool,
    #[serde(default = "default_true")]
    retrieve_value: bool,
    #[serde(default = "default_false")]
    retrieve_tags: bool
}

impl SearchOptions {
    pub fn id_value() -> String {
        let options = SearchOptions {
            retrieve_records: true,
            retrieve_total_count: true,
            retrieve_type: true,
            retrieve_value: true,
            retrieve_tags: false
        };

        serde_json::to_string(&options).unwrap()
    }
}

impl Default for SearchOptions {
    fn default() -> SearchOptions {
        SearchOptions {
            retrieve_records: true,
            retrieve_total_count: false,
            retrieve_type: false,
            retrieve_value: true,
            retrieve_tags: false,
        }
    }
}


const _POSTGRES_DB: &str = "postgres";
const _WALLETS_DB: &str = "wallets";
const _PLAIN_TAGS_QUERY: &str = "SELECT name, value from tags_plaintext where item_id = $1";
const _ENCRYPTED_TAGS_QUERY: &str = "SELECT name, value from tags_encrypted where item_id = $1";
const _CREATE_WALLETS_DATABASE: &str = "CREATE DATABASE wallets";
// Note: wallet id length was constrained before by postgres database name length to 64 characters, keeping the same restrictions
const _CREATE_SCHEMA: [&str; 14] = [
    "CREATE TABLE IF NOT EXISTS metadata (
        wallet_id VARCHAR(64) NOT NULL,
        id BIGSERIAL NOT NULL,
        value BYTEA NOT NULL,
        PRIMARY KEY(wallet_id, id)
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_metadata_wallet_id_id ON metadata(wallet_id, id)",
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_metadata_values ON metadata(wallet_id, value)",
    "CREATE TABLE IF NOT EXISTS items(
        wallet_id VARCHAR(64) NOT NULL,
        id BIGSERIAL NOT NULL,
        type BYTEA NOT NULL,
        name BYTEA NOT NULL,
        value BYTEA NOT NULL,
        key BYTEA NOT NULL,
        PRIMARY KEY(wallet_id, id)
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_items_wallet_id_id ON items(wallet_id, id)",
    "CREATE UNIQUE INDEX IF NOT EXISTS ux_items_type_name ON items(wallet_id, type, name)",
    "CREATE TABLE IF NOT EXISTS tags_encrypted(
        wallet_id VARCHAR(64) NOT NULL,
        name BYTEA NOT NULL,
        value BYTEA NOT NULL,
        item_id BIGINT NOT NULL,
        PRIMARY KEY(wallet_id, name, item_id),
        FOREIGN KEY(wallet_id, item_id)
            REFERENCES items(wallet_id, id)
            ON DELETE CASCADE
            ON UPDATE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS ix_tags_encrypted_name ON tags_encrypted(wallet_id, name)",
    "CREATE INDEX IF NOT EXISTS ix_tags_encrypted_value ON tags_encrypted(wallet_id, value)",
    "CREATE INDEX IF NOT EXISTS ix_tags_encrypted_wallet_id_item_id ON tags_encrypted(wallet_id, item_id)",
    "CREATE TABLE IF NOT EXISTS tags_plaintext(
        wallet_id VARCHAR(64) NOT NULL,
        name BYTEA NOT NULL,
        value TEXT NOT NULL,
        item_id BIGINT NOT NULL,
        PRIMARY KEY(wallet_id, name, item_id),
        FOREIGN KEY(wallet_id, item_id)
            REFERENCES items(wallet_id, id)
            ON DELETE CASCADE
            ON UPDATE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS ix_tags_plaintext_name ON tags_plaintext(wallet_id, name)",
    "CREATE INDEX IF NOT EXISTS ix_tags_plaintext_value ON tags_plaintext(wallet_id, value)",
    "CREATE INDEX IF NOT EXISTS ix_tags_plaintext_wallet_id_item_id ON tags_plaintext(wallet_id, item_id)"
    ];
const _DELETE_WALLET: [&str; 4] = [
    "DELETE FROM tags_plaintext WHERE wallet_id = $1",
    "DELETE FROM tags_encrypted WHERE wallet_id = $1",
    "DELETE FROM items WHERE wallet_id = $1",
    "DELETE FROM metadata WHERE wallet_id = $1"
    ];
const _DROP_WALLET_DATABASE: &str = "DROP DATABASE wallets";

#[derive(Debug)]
struct TagRetriever<'a> {
    plain_tags_stmt: postgres::stmt::Statement<'a>,
    encrypted_tags_stmt: postgres::stmt::Statement<'a>,
}

type TagRetrieverOwned = OwningHandle<Rc<r2d2::PooledConnection<PostgresConnectionManager>>, Box<TagRetriever<'static>>>;

impl<'a> TagRetriever<'a> {
    fn new_owned(conn: Rc<r2d2::PooledConnection<PostgresConnectionManager>>) -> Result<TagRetrieverOwned, WalletStorageError> {
        OwningHandle::try_new(conn.clone(), |conn| -> Result<_, postgres::Error> {
            let (plain_tags_stmt, encrypted_tags_stmt) = unsafe {
                ((*conn).prepare(_PLAIN_TAGS_QUERY)?,
                 (*conn).prepare(_ENCRYPTED_TAGS_QUERY)?)
            };
            let tr = TagRetriever {
                plain_tags_stmt,
                encrypted_tags_stmt,
            };
            Ok(Box::new(tr))
        }).map_err(WalletStorageError::from)
    }

    fn retrieve(&mut self, id: i64) -> Result<Vec<Tag>, WalletStorageError> {
        let mut tags = Vec::new();

        let plain_results = self.plain_tags_stmt.query(&[&id])?;
        let mut iter_plain = plain_results.iter();
        while let Some(res) = iter_plain.next() {
            let row = res;
            tags.push(Tag::PlainText(row.get(0), row.get(1)));
        }

        let encrypted_results = self.encrypted_tags_stmt.query(&[&id])?;
        let mut iter_encrypted = encrypted_results.iter();
        while let Some(res) = iter_encrypted.next() {
            let row = res;
            tags.push(Tag::Encrypted(row.get(0), row.get(1)));
        }

        Ok(tags)
    }
}

struct PostgresStorageIterator {
    rows: Option<
            OwningHandle<
                OwningHandle<
                    Rc<r2d2::PooledConnection<PostgresConnectionManager>>,
                    Box<postgres::stmt::Statement<'static>>>,
                Box<postgres::rows::Rows<>>>>,
    tag_retriever: Option<TagRetrieverOwned>,
    options: RecordOptions,
    total_count: Option<usize>,
    iter_count: usize,
}

impl PostgresStorageIterator {
    fn new(stmt: Option<OwningHandle<Rc<r2d2::PooledConnection<PostgresConnectionManager>>, Box<postgres::stmt::Statement<'static>>>>,
           args: &[&postgres::types::ToSql],
           options: RecordOptions,
           tag_retriever: Option<TagRetrieverOwned>,
           total_count: Option<usize>) -> Result<PostgresStorageIterator, WalletStorageError> {
        let mut iter = PostgresStorageIterator {
            rows: None,
            tag_retriever,
            options,
            total_count,
            iter_count: 0
        };

        if let Some(stmt) = stmt {
            iter.rows = Some(OwningHandle::try_new(
                stmt, |stmt|
                    unsafe {
                        (*(stmt as *mut postgres::stmt::Statement)).query(args).map(Box::new)
                    },
            )?);
        }

        Ok(iter)
    }
}

impl StorageIterator for PostgresStorageIterator {
    fn next(&mut self) -> Result<Option<StorageRecord>, WalletStorageError> {
        // if records are not requested.
        if self.rows.is_none() {
            return Ok(None);
        }

        // TODO not sure if iter().nth() is the most efficient way to iterate through the result set
        // TODO investigate if the Iter object can be cached between calls to next()
        match self.rows.as_mut().unwrap().iter().nth(self.iter_count) {
            Some(row) => {
                self.iter_count = self.iter_count + 1;
                let name = row.get(1);
                let value = if self.options.retrieve_value {
                    Some(EncryptedValue::new(row.get(2), row.get(3)))
                } else {
                    None
                };
                let tags = if self.options.retrieve_tags {
                    match self.tag_retriever {
                        Some(ref mut tag_retriever) => Some(tag_retriever.retrieve(row.get(0))?),
                        None => return Err(WalletStorageError::CommonError(
                            CommonError::InvalidState("Fetch tags option set and tag retriever is None".to_string())
                        ))
                    }
                } else {
                    None
                };
                let type_ = if self.options.retrieve_type {
                    Some(row.get(4))
                } else {
                    None
                };
                Ok(Some(StorageRecord::new(name, value, type_, tags)))
            }
            //Some(Err(err)) => Err(WalletStorageError::from(err)),
            None => Ok(None)
        }
    }

    fn get_total_count(&self) -> Result<Option<usize>, WalletStorageError> {
        Ok(self.total_count)
    }
}

#[derive(Deserialize, Debug)]
pub struct PostgresConfig {
    url: String,
}

#[derive(Deserialize, Debug)]
pub struct PostgresCredentials {
    account: String,
    password: String,
    admin_account: Option<String>,
    admin_password: Option<String>,
}

#[derive(Debug)]
pub struct PostgresStorage {
    pool: r2d2::Pool<PostgresConnectionManager>,
    wallet_id: String
}

pub trait WalletStorageType {
    fn init_storage(&self, config: Option<&str>, credentials: Option<&str>) -> Result<(), WalletStorageError>;
    fn create_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>, metadata: &[u8]) -> Result<(), WalletStorageError>;
    fn open_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>) -> Result<Box<PostgresStorage>, WalletStorageError>;
    fn delete_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>) -> Result<(), WalletStorageError>;
}

pub struct PostgresStorageType {}


impl PostgresStorageType {
    pub fn new() -> PostgresStorageType {
        PostgresStorageType {}
    }

    fn _admin_postgres_url(config: &PostgresConfig, credentials: &PostgresCredentials) -> String {
        let mut url_base = "postgresql://".to_owned();
        match credentials.admin_account {
            Some(ref account) => url_base.push_str(&account[..]),
            None => ()
        }
        url_base.push_str(":");
        match credentials.admin_password {
            Some(ref password) => url_base.push_str(&password[..]),
            None => ()
        }
        url_base.push_str("@");
        url_base.push_str(&config.url[..]);
        url_base
    }

    fn _base_postgres_url(config: &PostgresConfig, credentials: &PostgresCredentials) -> String {
        let mut url_base = "postgresql://".to_owned();
        url_base.push_str(&credentials.account[..]);
        url_base.push_str(":");
        url_base.push_str(&credentials.password[..]);
        url_base.push_str("@");
        url_base.push_str(&config.url[..]);
        url_base
    }

    fn _postgres_url(id: &str, config: &PostgresConfig, credentials: &PostgresCredentials) -> String {
        let mut url_base = PostgresStorageType::_base_postgres_url(config, credentials);
        url_base.push_str("/");
        url_base.push_str(id);
        url_base
    }
}


impl WalletStorage for PostgresStorage {
    ///
    /// Tries to fetch values and/or tags from the storage.
    /// Returns Result with StorageEntity object which holds requested data in case of success or
    /// Result with WalletStorageError in case of failure.
    ///
    ///
    /// # Arguments
    ///
    ///  * `type_` - type_ of the item in storage
    ///  * `id` - id of the item in storage
    ///  * `options` - JSon containing what needs to be fetched.
    ///  Example: {"retrieveValue": true, "retrieveTags": true}
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `StorageEntity` - Contains name, optional value and optional tags
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::Closed` - Storage is closed
    ///  * `WalletStorageError::ItemNotFound` - Item is not found in database
    ///  * `IOError("IO error during storage operation:...")` - Failed connection or SQL query
    ///
    fn get(&self, type_: &[u8], id: &[u8], options: &str) -> Result<StorageRecord, WalletStorageError> {
        let options: RecordOptions = if options == "{}" { // FIXME:
            RecordOptions::default()
        } else {
            serde_json::from_str(options)?
        };
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let res: Result<(i64, Vec<u8>, Vec<u8>), WalletStorageError> = {
            let mut rows = conn.query(
                "SELECT id, value, key FROM items where wallet_id = $1 AND type = $2 AND name = $3",
                &[&self.wallet_id, &type_.to_vec(), &id.to_vec()]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => Ok((row.get(0), row.get(1), row.get(2))),
                None => Err(WalletStorageError::ItemNotFound)
            }
        };
        let item = match res {
            Ok(entity) => entity,
            Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
            Err(err) => return Err(WalletStorageError::from(err))
        };
        let value = if options.retrieve_value
            { Some(EncryptedValue::new(item.1, item.2)) } else { None };
        let type_ = if options.retrieve_type { Some(type_.clone()) } else { None };
        let tags = if options.retrieve_tags {
            let mut tags = Vec::new();

            // get all encrypted.
            let mut stmt = conn.prepare_cached("SELECT name, value FROM tags_encrypted WHERE wallet_id = $1 AND item_id = $2")?;
            let mut rows = stmt.query(&[&self.wallet_id, &item.0])?;

            let mut iter = rows.iter();
            while let Some(res) = iter.next() {
                let row = res;
                //let tag_name: Vec<u8> = row.get(0);
                //let tag_value: Vec<u8> = row.get(1);
                tags.push(Tag::Encrypted(row.get(0), row.get(1)));
            }

            // get all plain
            let mut stmt = conn.prepare_cached("SELECT name, value FROM tags_plaintext WHERE wallet_id = $1 AND item_id = $2")?;
            let mut rows = stmt.query(&[&self.wallet_id, &item.0])?;

            let mut iter = rows.iter();
            while let Some(res) = iter.next() {
                let row = res;
                //let tag_name: Vec<u8> = row.get(0);
                //let tag_value: String = row.get(1);
                tags.push(Tag::PlainText(row.get(0), row.get(1)));
            }
            Some(tags)
        } else { None };

        Ok(StorageRecord::new(id.to_vec(), value, type_.map(|val| val.to_vec()), tags))
    }

    ///
    /// inserts value and tags into storage.
    /// Returns Result with () on success or
    /// Result with WalletStorageError in case of failure.
    ///
    ///
    /// # Arguments
    ///
    ///  * `type_` - type of the item in storage
    ///  * `id` - id of the item in storage
    ///  * `value` - value of the item in storage
    ///  * `value_key` - key used to encrypt the value
    ///  * `tags` - tags assigned to the value
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `()`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` class of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::Closed` - Storage is closed
    ///  * `WalletStorageError::ItemAlreadyExists` - Item is already present in database
    ///  * `IOError("IO error during storage operation:...")` - Failed connection or SQL query
    ///
    fn add(&self, type_: &[u8], id: &[u8], value: &EncryptedValue, tags: &[Tag]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let tx: transaction::Transaction = transaction::Transaction::new(&conn)?;
        let res = tx.prepare_cached("INSERT INTO items (wallet_id, type, name, value, key) VALUES ($1, $2, $3, $4, $5) RETURNING id")?
            .query(&[&self.wallet_id, &type_.to_vec(), &id.to_vec(), &value.data, &value.key]);

        let item_id = match res {
            Ok(rows) => {
                let res = match rows.iter().next() {
                    Some(row) => Ok(row.get(0)),
                    None => Err(WalletStorageError::ItemNotFound)
                };
                let item_id: i64 = match res {
                    Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
                    Err(err) => return Err(WalletStorageError::from(err)),
                    Ok(id) => id
                };
                item_id
            },
            Err(err) => {
                if err.code() == Some(&postgres::error::UNIQUE_VIOLATION) ||
                   err.code() == Some(&postgres::error::INTEGRITY_CONSTRAINT_VIOLATION) {
                    return Err(WalletStorageError::ItemAlreadyExists);
                } else {
                    return Err(WalletStorageError::from(err));
                }
            }
        };

        let item_id = item_id as i64;

        if !tags.is_empty() {
            let stmt_e = tx.prepare_cached("INSERT INTO tags_encrypted (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)")?;
            let stmt_p = tx.prepare_cached("INSERT INTO tags_plaintext (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)")?;

            for tag in tags {
                match tag {
                    &Tag::Encrypted(ref tag_name, ref tag_data) => {
                        match stmt_e.execute(&[&self.wallet_id, &item_id, tag_name, tag_data]) {
                            Ok(_) => (),
                            Err(err) => {
                                if err.code() == Some(&postgres::error::UNIQUE_VIOLATION) ||
                                   err.code() == Some(&postgres::error::INTEGRITY_CONSTRAINT_VIOLATION) {
                                    return Err(WalletStorageError::ItemAlreadyExists);
                                } else {
                                    return Err(WalletStorageError::from(err));
                                }
                            }
                        }
                    },
                    &Tag::PlainText(ref tag_name, ref tag_data) => {
                        match stmt_p.execute(&[&self.wallet_id, &item_id, tag_name, tag_data]) {
                            Ok(_) => (),
                            Err(err) => {
                                if err.code() == Some(&postgres::error::UNIQUE_VIOLATION) ||
                                   err.code() == Some(&postgres::error::INTEGRITY_CONSTRAINT_VIOLATION) {
                                    return Err(WalletStorageError::ItemAlreadyExists);
                                } else {
                                    return Err(WalletStorageError::from(err));
                                }
                            }
                        }
                    }
                };
            }
        }

        tx.commit()?;

        Ok(())
    }

    fn update(&self, type_: &[u8], id: &[u8], value: &EncryptedValue) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let res = conn.prepare_cached("UPDATE items SET value = $1, key = $2 WHERE wallet_id = $3 AND type = $4 AND name = $5")?
            .execute(&[&value.data, &value.key, &self.wallet_id, &type_.to_vec(), &id.to_vec()]);

        match res {
            Ok(1) => Ok(()),
            Ok(0) => Err(WalletStorageError::ItemNotFound),
            Ok(count) => Err(WalletStorageError::CommonError(CommonError::InvalidState(format!("Postgres returned update row count: {}", count)))),
            Err(err) => Err(WalletStorageError::from(err)),
        }
    }

    fn add_tags(&self, type_: &[u8], id: &[u8], tags: &[Tag]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let tx: transaction::Transaction = transaction::Transaction::new(&conn)?;

        let res = {
            let mut rows = tx.prepare_cached("SELECT id FROM items WHERE wallet_id = $1 AND type = $2 AND name = $3")?
                .query(&[&self.wallet_id, &type_.to_vec(), &id.to_vec()]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => Ok(row.get(0)),
                None => Err(WalletStorageError::ItemNotFound)
            }
        };

        let item_id: i64 = match res {
            Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
            Err(err) => return Err(WalletStorageError::from(err)),
            Ok(id) => id
        };

        if !tags.is_empty() {
            let enc_tag_insert_stmt = tx.prepare_cached("INSERT INTO tags_encrypted (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)
                                                        ON CONFLICT (wallet_id, name, item_id) DO UPDATE SET value = excluded.value")?;
            let plain_tag_insert_stmt = tx.prepare_cached("INSERT INTO tags_plaintext (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)
                                                        ON CONFLICT (wallet_id, name, item_id) DO UPDATE SET value = excluded.value")?;

            for tag in tags {
                match tag {
                    &Tag::Encrypted(ref tag_name, ref tag_data) => {
                        match enc_tag_insert_stmt.execute(&[&self.wallet_id, &item_id, tag_name, tag_data]) {
                            Ok(_) => (),
                            Err(err) => {
                                if err.code() == Some(&postgres::error::UNIQUE_VIOLATION) ||
                                   err.code() == Some(&postgres::error::INTEGRITY_CONSTRAINT_VIOLATION) {
                                    return Err(WalletStorageError::ItemAlreadyExists);
                                } else {
                                    return Err(WalletStorageError::from(err));
                                }
                            }
                        }
                    },
                    &Tag::PlainText(ref tag_name, ref tag_data) => {
                        match plain_tag_insert_stmt.execute(&[&self.wallet_id, &item_id, tag_name, tag_data]) {
                            Ok(_) => (),
                            Err(err) => {
                                if err.code() == Some(&postgres::error::UNIQUE_VIOLATION) ||
                                   err.code() == Some(&postgres::error::INTEGRITY_CONSTRAINT_VIOLATION) {
                                    return Err(WalletStorageError::ItemAlreadyExists);
                                } else {
                                    return Err(WalletStorageError::from(err));
                                }
                            }
                        }
                    }
                };
            }
        }
        tx.commit()?;

        Ok(())
    }

    fn update_tags(&self, type_: &[u8], id: &[u8], tags: &[Tag]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let tx: transaction::Transaction = transaction::Transaction::new(&conn)?;

        let res = {
            let mut rows = tx.prepare_cached("SELECT id FROM items WHERE wallet_id = $1 AND type = $2 AND name = $3")?
                .query(&[&self.wallet_id, &type_.to_vec(), &id.to_vec()]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => Ok(row.get(0)),
                None => Err(WalletStorageError::ItemNotFound)
            }
        };

        let item_id: i64 = match res {
            Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
            Err(err) => return Err(WalletStorageError::from(err)),
            Ok(id) => id
        };

        tx.execute("DELETE FROM tags_encrypted WHERE wallet_id = $1 AND item_id = $2", &[&self.wallet_id, &item_id])?;
        tx.execute("DELETE FROM tags_plaintext WHERE wallet_id = $1 AND item_id = $2", &[&self.wallet_id, &item_id])?;

        if !tags.is_empty() {
            let enc_tag_insert_stmt = tx.prepare_cached("INSERT INTO tags_encrypted (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)")?;
            let plain_tag_insert_stmt = tx.prepare_cached("INSERT INTO tags_plaintext (wallet_id, item_id, name, value) VALUES ($1, $2, $3, $4)")?;

            for tag in tags {
                match tag {
                    &Tag::Encrypted(ref tag_name, ref tag_data) => enc_tag_insert_stmt.execute(&[&self.wallet_id, &item_id, tag_name, tag_data])?,
                    &Tag::PlainText(ref tag_name, ref tag_data) => plain_tag_insert_stmt.execute(&[&self.wallet_id, &item_id, tag_name, tag_data])?
                };
            }
        }
        tx.commit()?;

        Ok(())
    }

    fn delete_tags(&self, type_: &[u8], id: &[u8], tag_names: &[TagName]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let res = {
            let mut rows = conn.prepare_cached("SELECT id FROM items WHERE wallet_id = $1 AND type = $2 AND name = $3")?
                .query(&[&self.wallet_id, &type_.to_vec(), &id.to_vec()]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => Ok(row.get(0)),
                None => Err(WalletStorageError::ItemNotFound)
            }
        };

        let item_id: i64 = match res {
            Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
            Err(err) => return Err(WalletStorageError::from(err)),
            Ok(id) => id
        };

        let tx: transaction::Transaction = transaction::Transaction::new(&conn)?;
        {
            let enc_tag_delete_stmt = tx.prepare_cached("DELETE FROM tags_encrypted WHERE wallet_id = $1 AND item_id = $2 AND name = $3")?;
            let plain_tag_delete_stmt = tx.prepare_cached("DELETE FROM tags_plaintext WHERE wallet_id = $1 AND item_id = $2 AND name = $3")?;

            for tag_name in tag_names {
                match tag_name {
                    &TagName::OfEncrypted(ref tag_name) => enc_tag_delete_stmt.execute(&[&self.wallet_id, &item_id, tag_name])?,
                    &TagName::OfPlain(ref tag_name) => plain_tag_delete_stmt.execute(&[&self.wallet_id, &item_id, tag_name])?,
                };
            }
        }
        tx.commit()?;

        Ok(())
    }

    ///
    /// deletes value and tags into storage.
    /// Returns Result with () on success or
    /// Result with WalletStorageError in case of failure.
    ///
    ///
    /// # Arguments
    ///
    ///  * `type_` - type of the item in storage
    ///  * `id` - id of the item in storage
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `()`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::Closed` - Storage is closed
    ///  * `WalletStorageError::ItemNotFound` - Item is not found in database
    ///  * `IOError("IO error during storage operation:...")` - Failed connection or SQL query
    ///
    fn delete(&self, type_: &[u8], id: &[u8]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let row_count = conn.execute(
            "DELETE FROM items where wallet_id = $1 AND type = $2 AND name = $3",
            &[&self.wallet_id, &type_.to_vec(), &id.to_vec()]
        )?;
        if row_count == 1 {
            Ok(())
        } else {
            Err(WalletStorageError::ItemNotFound)
        }
    }

    fn get_storage_metadata(&self) -> Result<Vec<u8>, WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let res: Result<Vec<u8>, WalletStorageError> = {
            let mut rows = conn.query(
                "SELECT value FROM metadata WHERE wallet_id = $1",
                &[&self.wallet_id]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => Ok(row.get(0)),
                None => Err(WalletStorageError::ItemNotFound)
            }
        };

        match res {
            Ok(entity) => Ok(entity),
            Err(WalletStorageError::ItemNotFound) => return Err(WalletStorageError::ItemNotFound),
            Err(err) => return Err(WalletStorageError::from(err))
        }
    }

    fn set_storage_metadata(&self, metadata: &[u8]) -> Result<(), WalletStorageError> {
        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        match conn.execute("UPDATE metadata SET value = $1 WHERE wallet_id = $2", &[&metadata.to_vec(), &self.wallet_id, ]) {
            Ok(_) => Ok(()),
            Err(error) => {
                Err(WalletStorageError::IOError(format!("Error occurred while inserting the keys: {}", error)))
            }
        }
    }

    fn get_all(&self) -> Result<Box<StorageIterator>, WalletStorageError> {
        let statement = self._prepare_statement("SELECT id, name, value, key, type FROM items WHERE wallet_id = $1")?;
        let fetch_options = RecordOptions {
            retrieve_type: true,
            retrieve_value: true,
            retrieve_tags: true,
        };
        let pool = self.pool.clone();
        let tag_retriever = Some(TagRetriever::new_owned(Rc::new(pool.get().unwrap()).clone())?);

        let storage_iterator = PostgresStorageIterator::new(Some(statement), &[&self.wallet_id], fetch_options, tag_retriever, None)?;
        Ok(Box::new(storage_iterator))
    }

    // TODO add wallet_id limitation to search
    fn search(&self, type_: &[u8], query: &language::Operator, options: Option<&str>) -> Result<Box<StorageIterator>, WalletStorageError> {
        let type_ = type_.to_vec(); // FIXME

        let search_options = match options {
            None => SearchOptions::default(),
            Some(option_str) => serde_json::from_str(option_str)?
        };

        let pool = self.pool.clone();
        let conn = pool.get().unwrap();
        let total_count: Option<usize> = if search_options.retrieve_total_count {
            let (query_string, query_arguments) = query::wql_to_sql_count(&type_, query)?;

            let mut rows = conn.query(
                &query_string,
                &query_arguments[..]);
            match rows.as_mut().unwrap().iter().next() {
                Some(row) => {
                    let x: i64 = row.get(0);
                    Some(x as usize)
                },
                None => None
            }
        } else { None };

        if search_options.retrieve_records {
            let fetch_options = RecordOptions {
                retrieve_value: search_options.retrieve_value,
                retrieve_tags: search_options.retrieve_tags,
                retrieve_type: search_options.retrieve_type,
            };

            let (query_string, query_arguments) = query::wql_to_sql(&type_, query, options)?;

            let statement = self._prepare_statement(&query_string)?;
            let tag_retriever = if fetch_options.retrieve_tags {
                let pool = self.pool.clone();
                Some(TagRetriever::new_owned(Rc::new(pool.get().unwrap()).clone())?)
            } else {
                None
            };
            let storage_iterator = PostgresStorageIterator::new(Some(statement), &query_arguments[..], fetch_options, tag_retriever, total_count)?;
            Ok(Box::new(storage_iterator))
        } else {
            let storage_iterator = PostgresStorageIterator::new(None, &[], RecordOptions::default(), None, total_count)?;
            Ok(Box::new(storage_iterator))
        }
    }

    fn close(&mut self) -> Result<(), WalletStorageError> {
        //let pool = self.pool.clone();
        //let conn = pool.get().unwrap();
        //let _ret = conn.finish();
        Ok(())
    }
}

impl PostgresStorage {
    fn _prepare_statement(&self, sql: &str) -> Result<
        OwningHandle<Rc<r2d2::PooledConnection<PostgresConnectionManager>>, Box<postgres::stmt::Statement<'static>>>,
        WalletStorageError> {
            let pool = self.pool.clone();
            OwningHandle::try_new(Rc::new(pool.get().unwrap()).clone(), |conn| {
                unsafe { (*conn).prepare(sql) }.map(Box::new).map_err(WalletStorageError::from)
        })
    }
}


impl WalletStorageType for PostgresStorageType {
    ///
    /// Deletes the Postgres database records associated with the provided wallet id from the path specified in the
    /// config file.
    ///
    /// # Arguments
    ///
    ///  * `id` - the wallet id
    ///  * `storage_config` - Postgres DB connection config
    ///  * `storage_credentials` - DB credentials
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `()`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::NotFound` - File with the provided id not found
    ///  * `IOError(..)` - Deletion of the file form the file-system failed
    ///
    fn delete_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>) -> Result<(), WalletStorageError> {
        let config = config
            .map(serde_json::from_str::<PostgresConfig>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize config: {:?}", err)))?;
        let credentials = credentials
            .map(serde_json::from_str::<PostgresCredentials>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize credentials: {:?}", err)))?;

        let config = match config {
            Some(config) => config,
            None => return Err(WalletStorageError::ConfigError)
        };
        let credentials = match credentials {
            Some(credentials) => credentials,
            None => return Err(WalletStorageError::ConfigError)
        };

        // if admin user and password aren't provided then bail
        if credentials.admin_account == None || credentials.admin_password == None {
            return Ok(())
        }

        let url = PostgresStorageType::_postgres_url(&_WALLETS_DB, &config, &credentials);

        let conn = match postgres::Connection::connect(&url[..], postgres::TlsMode::None) {
            Ok(conn) => conn,
            Err(error) => {
                return Err(WalletStorageError::IOError(format!("Error occurred while connecting to wallet schema: {}", error)));
            }
        };

        let mut ret = Ok(());
        for sql in &_DELETE_WALLET {
            ret = match conn.execute(sql, &[&id]) {
                Ok(row_count) => {
                    if row_count == 0 {
                        Err(WalletStorageError::NotFound)
                    } else {
                        Ok(())
                    }
                },
                Err(error) => {
                    Err(WalletStorageError::IOError(format!("Error occurred while deleting wallet: {}", error)))
                }
            }
        };
        conn.finish()?;
        return ret
    }

    ///
    /// Initializes the wallets database and creates the necessary tables for all wallets
    /// This only needs to be called once at the very beginning
    ///
    /// # Arguments
    ///
    ///  * `storage_config` - config containing the location of Postgres DB files
    ///  * `storage_credentials` - DB credentials
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `()`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::NotFound` - File with the provided id not found
    ///  * `IOError(..)` - Deletion of the file form the file-system failed
    ///
    fn init_storage(&self, config: Option<&str>, credentials: Option<&str>) -> Result<(), WalletStorageError> {
        let config = config
            .map(serde_json::from_str::<PostgresConfig>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize config: {:?}", err)))?;
        let credentials = credentials
            .map(serde_json::from_str::<PostgresCredentials>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize credentials: {:?}", err)))?;

        let config = match config {
            Some(config) => config,
            None => return Err(WalletStorageError::ConfigError)
        };
        let credentials = match credentials {
            Some(credentials) => credentials,
            None => return Err(WalletStorageError::ConfigError)
        };

        // if admin user and password aren't provided then bail
        if credentials.admin_account == None || credentials.admin_password == None {
            return Ok(())
        }

        let url_base = PostgresStorageType::_admin_postgres_url(&config, &credentials);
        let url = PostgresStorageType::_postgres_url(_WALLETS_DB, &config, &credentials);

        let conn = postgres::Connection::connect(&url_base[..], postgres::TlsMode::None)?;

        if let Err(error) = conn.execute(&_CREATE_WALLETS_DATABASE, &[]) {
            if error.code() != Some(&postgres::error::DUPLICATE_DATABASE) {
                conn.finish()?;
                return Err(WalletStorageError::IOError(format!("Error occurred while creating the database: {}", error)))
            }
        }
        conn.finish()?;
        
        let conn = match postgres::Connection::connect(&url[..], postgres::TlsMode::None) {
            Ok(conn) => conn,
            Err(error) => {
                return Err(WalletStorageError::IOError(format!("Error occurred while connecting to wallet schema: {}", error)));
            }
        };

        for sql in &_CREATE_SCHEMA {
            if let Err(error) = conn.execute(sql, &[]) {
                conn.finish()?;
                return Err(WalletStorageError::IOError(format!("Error occurred while creating wallet schema: {}", error)));
            }
        }
        conn.finish()?;
        Ok(())
    }

    ///
    /// Creates the Postgres DB schema with the provided name in the id specified in the config file,
    /// and initializes the encryption keys needed for encryption and decryption of data.
    ///
    /// # Arguments
    ///
    ///  * `id` - name of the Postgres DB schema
    ///  * `config` - config containing the location of postgres db
    ///  * `credentials` - DB credentials
    ///  * `metadata` - encryption keys that need to be stored in the newly created DB
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `()`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `AlreadyExists` - Schema with a given name already exists in the database
    ///  * `IOError("IO error during storage operation:...")` - Connection to the DB failed
    ///  * `IOError("Error occurred while creating wallet file:..)"` - Creation of schema failed
    ///  * `IOError("Error occurred while inserting the keys...")` - Insertion of keys failed
    ///
    fn create_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>, metadata: &[u8]) -> Result<(), WalletStorageError> {

        let config = config
            .map(serde_json::from_str::<PostgresConfig>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize config: {:?}", err)))?;
        let credentials = credentials
            .map(serde_json::from_str::<PostgresCredentials>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize credentials: {:?}", err)))?;

        let config = match config {
            Some(config) => config,
            None => return Err(WalletStorageError::ConfigError)
        };
        let credentials = match credentials {
            Some(credentials) => credentials,
            None => return Err(WalletStorageError::ConfigError)
        };

        // if admin user and password aren't provided then bail
        if credentials.admin_account == None || credentials.admin_password == None {
            return Ok(())
        }

        let url = PostgresStorageType::_postgres_url(_WALLETS_DB, &config, &credentials);

        let conn = match postgres::Connection::connect(&url[..], postgres::TlsMode::None) {
            Ok(conn) => conn,
            Err(error) => {
                return Err(WalletStorageError::IOError(format!("Error occurred while connecting to wallet schema: {}", error)));
            }
        };

        // We allow error on conflict since this indicates AlreadyExists error
        let ret = match conn.execute("INSERT INTO metadata(wallet_id, value) VALUES($1, $2)", &[&id, &metadata]) {
            Ok(_) => Ok(()),
            Err(error) => {
                if error.code() == Some(&postgres::error::UNIQUE_VIOLATION) {
                    Err(WalletStorageError::AlreadyExists)
                } else {
                    Err(WalletStorageError::IOError(format!("Error occurred while inserting into metadata: {}", error)))
                }    
            }
        };
        conn.finish()?;
        ret
    }

    ///
    /// Establishes a connection to the Postgres DB with the provided connection info
    /// specified in the config. In case of a successful connection returns a Storage object
    /// embedding the connection and the encryption keys that will be used for encryption and
    /// decryption operations.
    ///
    ///
    /// # Arguments
    ///
    ///  * `id` - wallet id
    ///  * `config` - config containing the connection info
    ///  * `credentials` - DB credentials
    ///
    /// # Returns
    ///
    /// Result that can be either:
    ///
    ///  * `(Box<Storage>, Vec<u8>)` - Tuple of `PostgresStorage` and `encryption keys`
    ///  * `WalletStorageError`
    ///
    /// # Errors
    ///
    /// Any of the following `WalletStorageError` type_ of errors can be throw by this method:
    ///
    ///  * `WalletStorageError::NotFound` - File with the provided id not found
    ///  * `IOError("IO error during storage operation:...")` - Failed connection or SQL query
    ///
    fn open_storage(&self, id: &str, config: Option<&str>, credentials: Option<&str>) -> Result<Box<PostgresStorage>, WalletStorageError> {

        let config = config
            .map(serde_json::from_str::<PostgresConfig>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize config: {:?}", err)))?;
        let credentials = credentials
            .map(serde_json::from_str::<PostgresCredentials>)
            .map_or(Ok(None), |v| v.map(Some))
            .map_err(|err| CommonError::InvalidStructure(format!("Cannot deserialize credentials: {:?}", err)))?;

        let config = match config {
            Some(config) => config,
            None => return Err(WalletStorageError::ConfigError)
        };
        let credentials = match credentials {
            Some(credentials) => credentials,
            None => return Err(WalletStorageError::ConfigError)
        };

        let url = PostgresStorageType::_postgres_url(_WALLETS_DB, &config, &credentials);

        // don't need a connection, but connect just to verify we can
        let _conn = match postgres::Connection::connect(&url[..], postgres::TlsMode::None) {
            Ok(conn) => conn,
            Err(_) => return Err(WalletStorageError::NotFound)
        };

        // determine if the wallet exists
        let rows = _conn.query("SELECT wallet_id FROM metadata WHERE wallet_id = $1", &[&id]);
        if rows.unwrap().iter().next().is_none() {
            return Err(WalletStorageError::NotFound)
        }

        let manager = match PostgresConnectionManager::new(&url[..], TlsMode::None) {
            Ok(manager) => manager,
            Err(_) => return Err(WalletStorageError::NotFound)
        };
        let pool = match r2d2::Pool::builder().min_idle(Some(0)).max_size(2).idle_timeout(Some(Duration::new(5, 0))).build(manager) {
            Ok(pool) => pool,
            Err(_) => return Err(WalletStorageError::NotFound)
        };

        Ok(Box::new(PostgresStorage { 
            pool: pool,
            wallet_id: id.to_string()
        }))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    use utils::test;

    #[test]
    fn postgres_storage_type_init_works() {
        _cleanup();

        let storage_type = PostgresStorageType::new();
        storage_type.init_storage(Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..])).unwrap();
    }

    #[test]
    fn postgres_storage_type_create_works() {
        _cleanup();

        let storage_type = PostgresStorageType::new();
        storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata()).unwrap();
    }

    #[test]
    fn postgres_storage_type_create_works_for_twice() {
        _cleanup();

        let storage_type = PostgresStorageType::new();
        storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata()).unwrap();

        let res = storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata());
        assert_match!(Err(WalletStorageError::AlreadyExists), res);
    }

    #[test]
    fn postgres_storage_get_storage_metadata_works() {
        _cleanup();

        let storage = _storage();
        let metadata = storage.get_storage_metadata().unwrap();

        assert_eq!(metadata, _metadata());
    }

    #[test]
    fn postgres_storage_type_delete_works() {
        _cleanup();

        let storage_type = PostgresStorageType::new();
        storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata()).unwrap();

        storage_type.delete_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..])).unwrap();
    }

    #[test]
    fn postgres_storage_type_delete_works_for_non_existing() {
        _cleanup();

        let storage_type = PostgresStorageType::new();
        storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata()).unwrap();

        let res = storage_type.delete_storage("unknown", Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]));
        assert_match!(Err(WalletStorageError::NotFound), res);

        storage_type.delete_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..])).unwrap();
    }

    #[test]
    fn postgres_storage_type_open_works() {
        _cleanup();
        _storage();
    }

    #[test]
    fn postgres_storage_type_open_works_for_not_created() {
        _cleanup();

        let storage_type = PostgresStorageType::new();

        let res = storage_type.open_storage("unknown", Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]));
        assert_match!(Err(WalletStorageError::NotFound), res);
    }

    #[test]
    fn postgres_storage_add_works_for_is_802() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.add(&_type1(), &_id1(), &_value1(), &_tags());
        assert_match!(Err(WalletStorageError::ItemAlreadyExists), res);

        let res = storage.add(&_type1(), &_id1(), &_value1(), &_tags());
        assert_match!(Err(WalletStorageError::ItemAlreadyExists), res);
    }

    #[test]
    fn postgres_storage_set_get_works() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();

        assert_eq!(record.value.unwrap(), _value1());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_tags()));
    }

    #[test]
    fn postgres_storage_set_get_works_for_twice() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.add(&_type1(), &_id1(), &_value2(), &_tags());
        assert_match!(Err(WalletStorageError::ItemAlreadyExists), res);
    }

    #[test]
    fn postgres_storage_set_get_works_for_reopen() {
        _cleanup();

        {
            _storage().add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        }

        let storage_type = PostgresStorageType::new();
        let storage = storage_type.open_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..])).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();

        assert_eq!(record.value.unwrap(), _value1());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_tags()));
    }

    #[test]
    fn postgres_storage_get_works_for_wrong_key() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.get(&_type1(), &_id2(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##);
        assert_match!(Err(WalletStorageError::ItemNotFound), res)
    }

    #[test]
    fn postgres_storage_delete_works() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_tags()));

        storage.delete(&_type1(), &_id1()).unwrap();
        let res = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##);
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    #[test]
    fn postgres_storage_delete_works_for_non_existing() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.delete(&_type1(), &_id2());
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    #[test]
    fn postgres_storage_create_and_find_multiple_works() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        let record1 = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record1.value.unwrap(), _value1());
        assert_eq!(_sort(record1.tags.unwrap()), _sort(_tags()));

        storage.add(&_type2(), &_id2(), &_value2(), &_tags()).unwrap();
        let record2 = storage.get(&_type2(), &_id2(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record2.value.unwrap(), _value2());
        assert_eq!(_sort(record2.tags.unwrap()), _sort(_tags()));
    }

    #[test]
    fn postgres_storage_get_all_works() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        storage.add(&_type2(), &_id2(), &_value2(), &_tags()).unwrap();

        let mut storage_iterator = storage.get_all().unwrap();

        let record = storage_iterator.next().unwrap().unwrap();
        assert_eq!(record.type_.unwrap(), _type1());
        assert_eq!(record.value.unwrap(), _value1());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_tags()));

        let record = storage_iterator.next().unwrap().unwrap();
        assert_eq!(record.type_.unwrap(), _type2());
        assert_eq!(record.value.unwrap(), _value2());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_tags()));

        let record = storage_iterator.next().unwrap();
        assert!(record.is_none());
    }

    #[test]
    fn postgres_storage_get_all_works_for_empty() {
        _cleanup();

        let storage = _storage();
        let mut storage_iterator = storage.get_all().unwrap();

        let record = storage_iterator.next().unwrap();
        assert!(record.is_none());
    }

    #[test]
    fn postgres_storage_update_works() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        storage.update(&_type1(), &_id1(), &_value2()).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value2());
    }

    #[test]
    fn postgres_storage_update_works_for_non_existing_id() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        let res = storage.update(&_type1(), &_id2(), &_value2());
        assert_match!(Err(WalletStorageError::ItemNotFound), res)
    }

    #[test]
    fn postgres_storage_update_works_for_non_existing_type() {
        _cleanup();

        let storage = _storage();

        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();
        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        let res = storage.update(&_type2(), &_id1(), &_value2());
        assert_match!(Err(WalletStorageError::ItemNotFound), res)
    }

    #[test]
    fn postgres_storage_add_tags_works() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        storage.add_tags(&_type1(), &_id1(), &_new_tags()).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        let expected_tags = {
            let mut tags = _tags();
            tags.extend(_new_tags());
            _sort(tags)
        };

        assert_eq!(_sort(record.tags.unwrap()), expected_tags);
    }

    #[test]
    fn postgres_storage_add_tags_works_for_non_existing_id() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.add_tags(&_type1(), &_id2(), &_new_tags());
        assert_match!(Err(WalletStorageError::ItemNotFound), res)
    }

    #[test]
    fn postgres_storage_add_tags_works_for_non_existing_type() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.add_tags(&_type2(), &_id1(), &_new_tags());
        assert_match!(Err(WalletStorageError::ItemNotFound), res)
    }

    #[test]
    fn postgres_storage_add_tags_works_for_already_existing() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let tags_with_existing = {
            let mut tags = _tags();
            tags.extend(_new_tags());
            tags
        };

        storage.add_tags(&_type1(), &_id1(), &tags_with_existing).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        let expected_tags = {
            let mut tags = _tags();
            tags.extend(_new_tags());
            _sort(tags)
        };

        assert_eq!(_sort(record.tags.unwrap()), expected_tags);
    }

    #[test]
    fn postgres_storage_update_tags_works() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        storage.update_tags(&_type1(), &_id1(), &_new_tags()).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());
        assert_eq!(_sort(record.tags.unwrap()), _sort(_new_tags()));
    }

    #[test]
    fn postgres_storage_update_tags_works_for_non_existing_id() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.update_tags(&_type1(), &_id2(), &_new_tags());
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    #[test]
    fn postgres_storage_update_tags_works_for_non_existing_type() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let res = storage.update_tags(&_type1(), &_id2(), &_new_tags());
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    #[test]
    fn postgres_storage_update_tags_works_for_already_existing() {
        _cleanup();

        let storage = _storage();
        storage.add(&_type1(), &_id1(), &_value1(), &_tags()).unwrap();

        let tags_with_existing = {
            let mut tags = _tags();
            tags.extend(_new_tags());
            tags
        };

        storage.update_tags(&_type1(), &_id1(), &tags_with_existing).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.value.unwrap(), _value1());

        let expected_tags = {
            let mut tags = _tags();
            tags.extend(_new_tags());
            _sort(tags)
        };

        assert_eq!(_sort(record.tags.unwrap()), expected_tags);
    }

    #[test]
    fn postgres_storage_delete_tags_works() {
        _cleanup();

        let storage = _storage();

        let tag_name1 = vec![0, 0, 0];
        let tag_name2 = vec![1, 1, 1];
        let tag_name3 = vec![2, 2, 2];
        let tag1 = Tag::Encrypted(tag_name1.clone(), vec![0, 0, 0]);
        let tag2 = Tag::PlainText(tag_name2.clone(), "tag_value_2".to_string());
        let tag3 = Tag::Encrypted(tag_name3.clone(), vec![2, 2, 2]);
        let tags = vec![tag1.clone(), tag2.clone(), tag3.clone()];

        storage.add(&_type1(), &_id1(), &_value1(), &tags).unwrap();

        let tag_names = vec![TagName::OfEncrypted(tag_name1.clone()), TagName::OfPlain(tag_name2.clone())];
        storage.delete_tags(&_type1(), &_id1(), &tag_names).unwrap();

        let record = storage.get(&_type1(), &_id1(), r##"{"retrieveType": false, "retrieveValue": true, "retrieveTags": true}"##).unwrap();
        assert_eq!(record.tags.unwrap(), vec![tag3]);
    }

    #[test]
    fn postgres_storage_delete_tags_works_for_non_existing_type() {
        _cleanup();

        let storage = _storage();

        let tag_name1 = vec![0, 0, 0];
        let tag_name2 = vec![1, 1, 1];
        let tag_name3 = vec![2, 2, 2];
        let tag1 = Tag::Encrypted(tag_name1.clone(), vec![0, 0, 0]);
        let tag2 = Tag::PlainText(tag_name2.clone(), "tag_value_2".to_string());
        let tag3 = Tag::Encrypted(tag_name3.clone(), vec![2, 2, 2]);
        let tags = vec![tag1.clone(), tag2.clone(), tag3.clone()];

        storage.add(&_type1(), &_id1(), &_value1(), &tags).unwrap();

        let tag_names = vec![TagName::OfEncrypted(tag_name1.clone()), TagName::OfPlain(tag_name2.clone())];
        let res = storage.delete_tags(&_type2(), &_id1(), &tag_names);
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    #[test]
    fn postgres_storage_delete_tags_works_for_non_existing_id() {
        _cleanup();

        let storage = _storage();

        let tag_name1 = vec![0, 0, 0];
        let tag_name2 = vec![1, 1, 1];
        let tag_name3 = vec![2, 2, 2];
        let tag1 = Tag::Encrypted(tag_name1.clone(), vec![0, 0, 0]);
        let tag2 = Tag::PlainText(tag_name2.clone(), "tag_value_2".to_string());
        let tag3 = Tag::Encrypted(tag_name3.clone(), vec![2, 2, 2]);
        let tags = vec![tag1.clone(), tag2.clone(), tag3.clone()];

        storage.add(&_type1(), &_id1(), &_value1(), &tags).unwrap();

        let tag_names = vec![TagName::OfEncrypted(tag_name1.clone()), TagName::OfPlain(tag_name2.clone())];
        let res = storage.delete_tags(&_type1(), &_id2(), &tag_names);
        assert_match!(Err(WalletStorageError::ItemNotFound), res);
    }

    fn _cleanup() {
        let storage_type = PostgresStorageType::new();
        let _ret = storage_type.delete_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]));
        let res = test::cleanup_storage();
        res
    }

    fn _wallet_id() -> &'static str {
        "walle1"
    }

    fn _storage() -> Box<WalletStorage> {
        let storage_type = PostgresStorageType::new();
        storage_type.create_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..]), &_metadata()).unwrap();
        let res = storage_type.open_storage(_wallet_id(), Some(&_wallet_config()[..]), Some(&_wallet_credentials()[..])).unwrap();
        res
    }

    fn _wallet_config() -> String {
        let config = json!({
            // TODO switch back to localhost
            "url": "postgres-db:5432".to_owned()
        }).to_string();
        config
    }

    fn _wallet_credentials() -> String {
        let creds = json!({
            "account": "postgres".to_owned(),
            "password": "mysecretpassword".to_owned(),
            "admin_account": Some("postgres".to_owned()),
            "admin_password": Some("mysecretpassword".to_owned())
        }).to_string();
        creds
    }

    fn _metadata() -> Vec<u8> {
        return vec![
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8,
            1, 2, 3, 4, 5, 6, 7, 8
        ];
    }

    fn _type(i: u8) -> Vec<u8> {
        vec![i, 1 + i, 2 + i]
    }

    fn _type1() -> Vec<u8> {
        _type(1)
    }

    fn _type2() -> Vec<u8> {
        _type(2)
    }

    fn _id(i: u8) -> Vec<u8> {
        vec![3 + i, 4 + i, 5 + i]
    }

    fn _id1() -> Vec<u8> {
        _id(1)
    }

    fn _id2() -> Vec<u8> {
        _id(2)
    }

    fn _value(i: u8) -> EncryptedValue {
        EncryptedValue { data: vec![6 + i, 7 + i, 8 + i], key: vec![9 + i, 10 + i, 11 + i] }
    }

    fn _value1() -> EncryptedValue {
        _value(1)
    }

    fn _value2() -> EncryptedValue {
        _value(2)
    }

    fn _tags() -> Vec<Tag> {
        let mut tags: Vec<Tag> = Vec::new();
        tags.push(Tag::Encrypted(vec![1, 5, 8], vec![3, 5, 6]));
        tags.push(Tag::PlainText(vec![1, 5, 8, 1], "Plain value 1".to_string()));
        tags.push(Tag::Encrypted(vec![2, 5, 8], vec![3, 5, 7]));
        tags.push(Tag::PlainText(vec![2, 5, 8, 1], "Plain value 2".to_string()));
        tags
    }

    fn _new_tags() -> Vec<Tag> {
        vec![
            Tag::Encrypted(vec![1, 1, 1], vec![2, 2, 2]),
            Tag::PlainText(vec![1, 1, 1], String::from("tag_value_3"))
        ]
    }

    fn _sort(mut v: Vec<Tag>) -> Vec<Tag> {
        v.sort();
        v
    }
}
