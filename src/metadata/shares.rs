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
}

impl MetadataStore {
    pub fn create_share_link(
        &self,
        manifest: &ObjectManifest,
        expires_at: Option<i64>,
    ) -> Result<(String, ShareLinkRecord), MetadataError> {
        let token = Uuid::new_v4().to_string();
        let token_hash = hash_token(&token);
        let record = ShareLinkRecord {
            id: Uuid::new_v4().to_string(),
            object_id: manifest.object_id,
            bucket: manifest.bucket.clone(),
            key: manifest.key.clone(),
            created_at: crate::durable::now(),
            expires_at,
            revoked_at: None,
        };
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO share_links(id,token_hash,object_id,bucket,object_key,created_at,expires_at,revoked_at) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL)",
                params![
                    record.id,
                    token_hash,
                    record.object_id.to_string(),
                    record.bucket,
                    record.key,
                    record.created_at,
                    record.expires_at,
                ],
            )?;
            Ok((token, record))
        })
    }

    pub fn get_share_link(&self, token: &str) -> Result<Option<ShareLinkRecord>, MetadataError> {
        let token_hash = hash_token(token);
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,object_id,bucket,object_key,created_at,expires_at,revoked_at FROM share_links WHERE token_hash=?1",
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
}
