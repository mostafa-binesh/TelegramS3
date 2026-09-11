use super::{MetadataError, MetadataStore};
use crate::manifest::ObjectManifest;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShareLinkRecord {
    pub id: String,
    pub object_id: Uuid,
    pub bucket: String,
    pub key: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub description: String,
    #[serde(skip_serializing)]
    pub token_ciphertext: Option<String>,
}

impl MetadataStore {
    pub fn create_share_link(
        &self,
        manifest: &ObjectManifest,
        expires_at: Option<i64>,
    ) -> Result<(String, ShareLinkRecord), MetadataError> {
        let token = Uuid::new_v4().to_string();
        self.create_share_link_with_token(manifest, expires_at, "", &token, None)
    }

    pub fn create_share_link_with_token(
        &self,
        manifest: &ObjectManifest,
        expires_at: Option<i64>,
        description: &str,
        token: &str,
        token_ciphertext: Option<&str>,
    ) -> Result<(String, ShareLinkRecord), MetadataError> {
        let description = description.trim();
        if description.chars().count() > 240 {
            return Err(MetadataError::InvalidManifest(
                "share description must be 240 characters or fewer".to_string(),
            ));
        }
        let token_hash = hash_token(token);
        let record = ShareLinkRecord {
            id: Uuid::new_v4().to_string(),
            object_id: manifest.object_id,
            bucket: manifest.bucket.clone(),
            key: manifest.key.clone(),
            created_at: crate::durable::now(),
            expires_at,
            revoked_at: None,
            description: description.to_string(),
            token_ciphertext: token_ciphertext.map(str::to_string),
        };
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO share_links(id,token_hash,token_ciphertext,object_id,bucket,object_key,created_at,expires_at,revoked_at,description) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9)",
                params![
                    record.id,
                    token_hash,
                    record.token_ciphertext,
                    record.object_id.to_string(),
                    record.bucket,
                    record.key,
                    record.created_at,
                    record.expires_at,
                    record.description,
                ],
            )?;
            Ok((token.to_string(), record))
        })
    }

    pub fn get_share_link(&self, token: &str) -> Result<Option<ShareLinkRecord>, MetadataError> {
        let token_hash = hash_token(token);
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,object_id,bucket,object_key,created_at,expires_at,revoked_at,token_ciphertext,description FROM share_links WHERE token_hash=?1",
                    [token_hash],
                    |row| {
                        let object_id = row
                            .get::<_, String>(1)?
                            .parse::<Uuid>()
                            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                        Ok(ShareLinkRecord {
                            id: row.get(0)?,
                            object_id,
                            bucket: row.get(2)?,
                            key: row.get(3)?,
                            created_at: row.get(4)?,
                            expires_at: row.get(5)?,
                            revoked_at: row.get(6)?,
                            token_ciphertext: row.get(7)?,
                            description: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub fn revoke_share_link(&self, token: &str) -> Result<bool, MetadataError> {
        let token_hash = hash_token(token);
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE share_links SET revoked_at=?2 WHERE token_hash=?1 AND revoked_at IS NULL",
                params![token_hash, crate::durable::now()],
            )? == 1)
        })
    }

    pub fn list_share_links(
        &self,
        bucket: &str,
        object_key: &str,
    ) -> Result<Vec<ShareLinkRecord>, MetadataError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,object_id,bucket,object_key,created_at,expires_at,revoked_at,token_ciphertext,description FROM share_links WHERE bucket=?1 AND object_key=?2 AND revoked_at IS NULL ORDER BY created_at DESC, id DESC",
            )?;
            let mut links = Vec::new();
            for row in statement.query_map(params![bucket, object_key], |row| {
                let object_id = row
                    .get::<_, String>(1)?
                    .parse::<Uuid>()
                    .map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?;
                Ok(ShareLinkRecord {
                    id: row.get(0)?,
                    object_id,
                    bucket: row.get(2)?,
                    key: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    revoked_at: row.get(6)?,
                    token_ciphertext: row.get(7)?,
                    description: row.get(8)?,
                })
            })? {
                links.push(row?);
            }
            Ok(links)
        })
    }

    pub fn get_share_link_by_id(&self, id: &str) -> Result<Option<ShareLinkRecord>, MetadataError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,object_id,bucket,object_key,created_at,expires_at,revoked_at,token_ciphertext,description FROM share_links WHERE id=?1 AND revoked_at IS NULL",
                    [id],
                    |row| {
                        let object_id = row
                            .get::<_, String>(1)?
                            .parse::<Uuid>()
                            .map_err(|error| {
                                rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                            })?;
                        Ok(ShareLinkRecord {
                            id: row.get(0)?,
                            object_id,
                            bucket: row.get(2)?,
                            key: row.get(3)?,
                            created_at: row.get(4)?,
                            expires_at: row.get(5)?,
                            revoked_at: row.get(6)?,
                            token_ciphertext: row.get(7)?,
                            description: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub fn share_link_count(&self, bucket: &str, object_key: &str) -> Result<u64, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM share_links WHERE bucket=?1 AND object_key=?2 AND revoked_at IS NULL",
                params![bucket, object_key],
                |row| row.get(0),
            )?)
        })
    }

    pub fn update_share_link_expiry(
        &self,
        id: &str,
        expires_at: Option<i64>,
    ) -> Result<Option<ShareLinkRecord>, MetadataError> {
        self.with_connection(|connection| {
            if connection.execute(
                "UPDATE share_links SET expires_at=?2 WHERE id=?1 AND revoked_at IS NULL",
                params![id, expires_at],
            )? == 0 {
                return Ok(None);
            }
            connection
                .query_row(
                    "SELECT id,object_id,bucket,object_key,created_at,expires_at,revoked_at,token_ciphertext,description FROM share_links WHERE id=?1",
                    [id],
                    |row| {
                        let object_id = row
                            .get::<_, String>(1)?
                            .parse::<Uuid>()
                            .map_err(|error| {
                                rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                            })?;
                        Ok(ShareLinkRecord {
                            id: row.get(0)?,
                            object_id,
                            bucket: row.get(2)?,
                            key: row.get(3)?,
                            created_at: row.get(4)?,
                            expires_at: row.get(5)?,
                            revoked_at: row.get(6)?,
                            token_ciphertext: row.get(7)?,
                            description: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(MetadataError::from)
        })
    }

    pub fn revoke_share_link_by_id(&self, id: &str) -> Result<bool, MetadataError> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE share_links SET revoked_at=?2 WHERE id=?1 AND revoked_at IS NULL",
                params![id, crate::durable::now()],
            )? == 1)
        })
    }
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::now;
    use crate::manifest::CommittedManifestArgs;

    #[test]
    fn share_token_is_lookupable_and_revocable() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "file.txt".to_string(),
            content_length: 0,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: "deadbeef".to_string(),
            peer_id: "peer".to_string(),
            message_id: 1,
        });
        let (token, created) = store
            .create_share_link(&manifest, Some(now() + 60))
            .expect("create");
        let found = store.get_share_link(&token).expect("get").expect("link");
        assert_eq!(found.object_id, manifest.object_id);
        assert_eq!(found.expires_at, created.expires_at);
        assert_eq!(found.description, "");
        assert!(store.revoke_share_link(&token).expect("revoke"));
        assert!(
            store
                .get_share_link(&token)
                .expect("get after revoke")
                .unwrap()
                .revoked_at
                .is_some()
        );
    }

    #[test]
    fn share_links_can_be_listed_updated_and_revoked() {
        let store = MetadataStore::open_in_memory().expect("metadata");
        let manifest = ObjectManifest::committed(CommittedManifestArgs {
            bucket: "bucket".to_string(),
            key: "file.txt".to_string(),
            content_length: 0,
            content_type: "text/plain".to_string(),
            checksum_algorithm: "sha256".to_string(),
            whole_object: "deadbeef".to_string(),
            peer_id: "peer".to_string(),
            message_id: 1,
        });
        let (_, created) = store
            .create_share_link_with_token(
                &manifest,
                Some(now() + 60),
                "Team handoff",
                "token-one",
                Some("encrypted-token"),
            )
            .expect("create");
        assert_eq!(
            store.share_link_count("bucket", "file.txt").expect("count"),
            1
        );
        let listed = store.list_share_links("bucket", "file.txt").expect("list");
        assert_eq!(listed[0].id, created.id);
        assert_eq!(listed[0].description, "Team handoff");
        assert_eq!(
            listed[0].token_ciphertext.as_deref(),
            Some("encrypted-token")
        );
        let updated = store
            .update_share_link_expiry(&created.id, Some(now() + 120))
            .expect("update")
            .expect("updated link");
        assert_eq!(updated.expires_at, Some(now() + 120));
        assert!(store.revoke_share_link_by_id(&created.id).expect("revoke"));
        assert_eq!(
            store.share_link_count("bucket", "file.txt").expect("count"),
            0
        );
        assert!(
            store
                .list_share_links("bucket", "file.txt")
                .expect("list")
                .is_empty()
        );
    }
}
