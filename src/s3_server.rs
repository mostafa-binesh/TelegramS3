use crate::config::AppConfig;
use crate::object_format::{ObjectFormatService, sha256_hex};
use crate::redact;
use crate::telegram::TelegramTransport;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use s3s::S3ErrorCode;
use s3s::auth::SimpleAuth;
use s3s::dto::{
    Bucket, CreateBucketInput, CreateBucketOutput, DeleteBucketInput, DeleteBucketOutput,
    DeleteObjectInput, DeleteObjectOutput, ETag, GetObjectInput, GetObjectOutput, HeadBucketInput,
    HeadBucketOutput, HeadObjectInput, HeadObjectOutput, ListBucketsInput, ListBucketsOutput,
    ListObjectsV2Input, ListObjectsV2Output, Object, ObjectStorageClass, StreamingBlob, Timestamp,
};
use s3s::service::{S3Service, S3ServiceBuilder};
use s3s::{Body, S3, S3Request, S3Response, S3Result};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::time::{Duration, timeout};

#[derive(Debug, Error)]
pub enum S3ServerError {
    #[error("{0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("{0}")]
    ObjectFormat(#[from] crate::object_format::ObjectFormatError),
    #[error("{0}")]
    Telegram(#[from] crate::telegram::TelegramTransportError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("server not initialized")]
    NotInitialized,
    #[error("s3 error: {0}")]
    S3(#[from] s3s::S3Error),
}

#[derive(Clone)]
struct TelegramS3Backend {
    object_format: Arc<ObjectFormatService>,
}

impl fmt::Debug for TelegramS3Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelegramS3Backend").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
struct AllowAllAccess;

#[async_trait]
impl s3s::access::S3Access for AllowAllAccess {
    async fn check(&self, _context: &mut s3s::access::S3AccessContext<'_>) -> S3Result<()> {
        Ok(())
    }
}

pub struct S3Server {
    addr: SocketAddr,
    service: S3Service,
}

impl S3Server {
    pub async fn bootstrap(config: &AppConfig) -> Result<Self, S3ServerError> {
        config.validate()?;
        let object_format = Arc::new(ObjectFormatService::open(config)?);
        let object_status = object_format.bootstrap()?;
        let transport = TelegramTransport::open(config.clone()).await?;
        let transport_status = transport.bootstrap().await?;

        println!("object format bootstrap: {:?}", object_status);
        println!("telegram bootstrap: {:?}", transport_status.session_state);
        println!(
            "telegram bootstrap: session path {}",
            redact::redact_path(&transport_status.session_path.display().to_string())
        );

        let backend = TelegramS3Backend { object_format };
        let mut builder = S3ServiceBuilder::new(backend);
        builder.set_auth(SimpleAuth::from_single(
            config.rustfs_access_key.clone().unwrap_or_default(),
            config.rustfs_secret_key.clone().unwrap_or_default(),
        ));
        builder.set_access(AllowAllAccess);
        let service = builder.build();
        Ok(Self {
            addr: config.s3_bind_addr()?,
            service,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.addr
    }

    pub async fn serve(self) -> Result<(), S3ServerError> {
        let listener = TcpListener::bind(self.addr).await?;
        let service = self.service.clone();
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let service = service.clone();
                    connections.spawn(async move {
                        let handler = service_fn(move |request| handle_request(request, service.clone()));
                        let mut connection = http1::Builder::new();
                        connection.keep_alive(false).timer(TokioTimer::new());
                        let _ = connection.serve_connection(TokioIo::new(stream), handler).await;
                    });
                }
                _ = signal::ctrl_c() => break,
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}

async fn handle_request(
    request: hyper::Request<Incoming>,
    service: S3Service,
) -> Result<hyper::Response<Body>, Box<dyn Error + Send + Sync>> {
    let response = timeout(
        Duration::from_secs(60),
        service.call(request.map(Body::from)),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "s3 request timed out"))??;
    Ok(response)
}

#[async_trait]
impl S3 for TelegramS3Backend {
    async fn create_bucket(
        &self,
        req: S3Request<CreateBucketInput>,
    ) -> S3Result<S3Response<CreateBucketOutput>> {
        let bucket = req.input.bucket;
        self.object_format
            .create_bucket(&bucket)
            .map_err(map_object_error)?;
        Ok(S3Response::new(CreateBucketOutput::default()))
    }

    async fn delete_bucket(
        &self,
        req: S3Request<DeleteBucketInput>,
    ) -> S3Result<S3Response<DeleteBucketOutput>> {
        self.object_format
            .delete_bucket(&req.input.bucket)
            .map_err(map_object_error)?;
        Ok(S3Response::new(DeleteBucketOutput::default()))
    }

    async fn head_bucket(
        &self,
        req: S3Request<HeadBucketInput>,
    ) -> S3Result<S3Response<HeadBucketOutput>> {
        let bucket = req.input.bucket;
        if !self
            .object_format
            .bucket_exists(&bucket)
            .map_err(map_object_error)?
        {
            return Err(s3s::s3_error!(NoSuchBucket, "bucket does not exist"));
        }
        Ok(S3Response::new(HeadBucketOutput::default()))
    }

    async fn list_buckets(
        &self,
        _req: S3Request<ListBucketsInput>,
    ) -> S3Result<S3Response<ListBucketsOutput>> {
        let buckets = self
            .object_format
            .list_buckets()
            .map_err(map_object_error)?
            .into_iter()
            .map(|bucket| Bucket {
                creation_date: Some(Timestamp::from(bucket.created_at)),
                name: Some(bucket.name),
                ..Default::default()
            })
            .collect();
        Ok(S3Response::new(ListBucketsOutput {
            buckets: Some(buckets),
            ..Default::default()
        }))
    }

    async fn put_object(
        &self,
        req: S3Request<s3s::dto::PutObjectInput>,
    ) -> S3Result<S3Response<s3s::dto::PutObjectOutput>> {
        let input = req.input;
        if !self
            .object_format
            .bucket_exists(&input.bucket)
            .map_err(map_object_error)?
        {
            return Err(s3s::s3_error!(NoSuchBucket, "bucket does not exist"));
        }
        let content_type = input
            .content_type
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let manifest = self
            .object_format
            .put_stream(&input.bucket, &input.key, &content_type, input.body)
            .await
            .map_err(map_object_error)?;
        Ok(S3Response::new(s3s::dto::PutObjectOutput {
            e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
            version_id: Some(manifest.object_id.to_string()),
            ..Default::default()
        }))
    }

    async fn head_object(
        &self,
        req: S3Request<HeadObjectInput>,
    ) -> S3Result<S3Response<HeadObjectOutput>> {
        let input = req.input;
        let manifest = self
            .object_format
            .get_active_manifest(&input.bucket, &input.key)
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "object does not exist"))?;
        let metadata: HashMap<_, _> = manifest
            .user_metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(S3Response::new(HeadObjectOutput {
            content_length: Some(manifest.content_length as i64),
            content_type: Some(manifest.content_type),
            e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
            last_modified: Some(Timestamp::from(manifest.created_at)),
            metadata: Some(metadata),
            version_id: manifest.version_id,
            content_range: None,
            ..Default::default()
        }))
    }

    async fn get_object(
        &self,
        req: S3Request<GetObjectInput>,
    ) -> S3Result<S3Response<GetObjectOutput>> {
        let headers = req.headers.clone();
        let input = req.input;
        let manifest = self
            .object_format
            .get_active_manifest(&input.bucket, &input.key)
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "object does not exist"))?;
        let (range, content_range) = object_range(&headers, manifest.content_length)?;
        let spans = ObjectFormatService::plan_read(&manifest, range.clone())
            .map_err(map_object_error)?
            .chunks;
        let metadata: HashMap<_, _> = manifest
            .user_metadata
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if spans.len() == 1 {
            let span = spans[0].clone();
            let object_format = Arc::clone(&self.object_format);
            let chunk_path = object_format.chunk_path(manifest.object_id, span.order);
            let chunk = tokio::fs::read(&chunk_path).await.map_err(|error| {
                s3s::S3Error::with_message(S3ErrorCode::InternalError, error.to_string())
            })?;
            let actual_checksum = sha256_hex(&chunk);
            if actual_checksum != span.checksum {
                return Err(s3s::S3Error::with_message(
                    S3ErrorCode::InternalError,
                    format!(
                        "checksum mismatch for chunk {}: expected {}, got {}",
                        span.order, span.checksum, actual_checksum
                    ),
                ));
            }
            let start = span.offset_within_chunk as usize;
            let end = start + span.length as usize;
            if chunk.len() < end {
                return Err(s3s::S3Error::with_message(
                    S3ErrorCode::InternalError,
                    "chunk shorter than planned span",
                ));
            }
            let actual = &chunk[start..end];
            return Ok(S3Response::new(GetObjectOutput {
                body: Some(StreamingBlob::from_bytes(Bytes::copy_from_slice(actual))),
                content_length: Some((range.end - range.start) as i64),
                content_type: Some(manifest.content_type),
                e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
                last_modified: Some(Timestamp::from(manifest.created_at)),
                metadata: Some(metadata),
                version_id: manifest.version_id,
                content_range,
                ..Default::default()
            }));
        }
        let object_format = Arc::clone(&self.object_format);
        let object_id = manifest.object_id;
        let body_stream = stream::unfold((0usize, spans, false), move |state| {
            let object_format = Arc::clone(&object_format);
            async move {
                let (index, spans, done) = state;
                if done {
                    return None;
                }
                let span = spans.get(index)?.clone();
                let chunk_path = object_format.chunk_path(object_id, span.order);
                let chunk = match tokio::fs::read(&chunk_path).await {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        return Some((Err::<Bytes, std::io::Error>(error), (index, spans, true)));
                    }
                };
                let actual_checksum = sha256_hex(&chunk);
                if actual_checksum != span.checksum {
                    return Some((
                        Err::<Bytes, std::io::Error>(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "checksum mismatch for chunk {}: expected {}, got {}",
                                span.order, span.checksum, actual_checksum
                            ),
                        )),
                        (index, spans, true),
                    ));
                }
                let start = span.offset_within_chunk as usize;
                let end = start + span.length as usize;
                if chunk.len() < end {
                    return Some((
                        Err::<Bytes, std::io::Error>(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "chunk shorter than planned span",
                        )),
                        (index, spans, true),
                    ));
                }
                let actual = &chunk[start..end];
                Some((
                    Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(actual)),
                    (index + 1, spans, false),
                ))
            }
        });
        let body = StreamingBlob::wrap(body_stream);
        Ok(S3Response::new(GetObjectOutput {
            body: Some(body),
            content_length: Some((range.end - range.start) as i64),
            content_type: Some(manifest.content_type),
            e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
            last_modified: Some(Timestamp::from(manifest.created_at)),
            metadata: Some(metadata),
            version_id: manifest.version_id,
            content_range,
            ..Default::default()
        }))
    }

    async fn delete_object(
        &self,
        req: S3Request<DeleteObjectInput>,
    ) -> S3Result<S3Response<DeleteObjectOutput>> {
        let input = req.input;
        let deleted = self
            .object_format
            .delete_object(&input.bucket, &input.key)
            .map_err(map_object_error)?;
        let version_id = deleted.map(|manifest| manifest.object_id.to_string());
        Ok(S3Response::new(DeleteObjectOutput {
            delete_marker: Some(version_id.is_some()),
            version_id,
            ..Default::default()
        }))
    }

    async fn list_objects_v2(
        &self,
        req: S3Request<ListObjectsV2Input>,
    ) -> S3Result<S3Response<ListObjectsV2Output>> {
        let input = req.input;
        if !self
            .object_format
            .bucket_exists(&input.bucket)
            .map_err(map_object_error)?
        {
            return Err(s3s::s3_error!(NoSuchBucket, "bucket does not exist"));
        }
        let prefix = input.prefix.unwrap_or_default();
        let max_keys = input.max_keys.unwrap_or(1000).clamp(0, 1000) as usize;
        let start_after = input.start_after.clone().unwrap_or_default();
        let start_after_value = input.start_after.clone();
        let mut manifests = self
            .object_format
            .list_bucket_manifests(&input.bucket, Some(&prefix))
            .map_err(map_object_error)?;
        let manifest_count = manifests.len();
        manifests.sort_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then(left.object_id.cmp(&right.object_id))
        });
        let mut objects = Vec::new();
        let mut started = start_after.is_empty();
        for manifest in manifests {
            if !started {
                if manifest.key > start_after {
                    started = true;
                } else {
                    continue;
                }
            }
            if objects.len() >= max_keys {
                break;
            }
            objects.push(Object {
                key: Some(manifest.key),
                last_modified: Some(Timestamp::from(manifest.created_at)),
                size: Some(manifest.content_length as i64),
                e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
                storage_class: Some(ObjectStorageClass::from_static(
                    ObjectStorageClass::STANDARD,
                )),
                ..Default::default()
            });
        }
        let is_truncated = objects.len() == max_keys && manifest_count > objects.len();
        let next_continuation_token = if is_truncated {
            objects.last().and_then(|object| object.key.clone())
        } else {
            None
        };
        Ok(S3Response::new(ListObjectsV2Output {
            is_truncated: Some(is_truncated),
            key_count: Some(objects.len() as i32),
            max_keys: Some(max_keys as i32),
            name: Some(input.bucket),
            prefix: Some(prefix),
            contents: Some(objects),
            next_continuation_token,
            continuation_token: input.continuation_token,
            start_after: start_after_value,
            ..Default::default()
        }))
    }
}

fn map_object_error(error: crate::object_format::ObjectFormatError) -> s3s::S3Error {
    match error {
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketNotFound(bucket),
        ) => s3s::S3Error::with_message(S3ErrorCode::NoSuchBucket, bucket),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketAlreadyExists(bucket),
        ) => s3s::S3Error::with_message(S3ErrorCode::BucketAlreadyOwnedByYou, bucket),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::BucketNotEmpty(bucket),
        ) => s3s::S3Error::with_message(S3ErrorCode::BucketNotEmpty, bucket),
        crate::object_format::ObjectFormatError::Metadata(
            crate::metadata::MetadataError::ManifestNotFound(_),
        ) => s3s::S3Error::with_message(S3ErrorCode::NoSuchKey, "object does not exist"),
        other => s3s::S3Error::with_message(S3ErrorCode::InternalError, other.to_string()),
    }
}

fn object_range(
    headers: &http::HeaderMap,
    content_length: u64,
) -> Result<(std::ops::Range<u64>, Option<String>), s3s::S3Error> {
    let Some(range_header) = headers
        .get(http::header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok((0..content_length, None));
    };
    let value = range_header.strip_prefix("bytes=").ok_or_else(|| {
        s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "unsupported range header")
    })?;
    let (start, end, content_range) = if let Some((start, end)) = value.split_once('-') {
        if start.is_empty() {
            let suffix_len = end.parse::<u64>().map_err(|_| {
                s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "invalid suffix range")
            })?;
            let start = content_length.saturating_sub(suffix_len);
            let end = content_length;
            (
                start,
                end,
                format!("bytes {start}-{}/{}", end.saturating_sub(1), content_length),
            )
        } else {
            let start = start.parse::<u64>().map_err(|_| {
                s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "invalid range start")
            })?;
            let end = if end.is_empty() {
                content_length
            } else {
                end.parse::<u64>().map_err(|_| {
                    s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "invalid range end")
                })? + 1
            };
            if start > end || end > content_length {
                return Err(s3s::S3Error::with_message(
                    S3ErrorCode::InvalidRange,
                    "requested range is invalid",
                ));
            }
            (
                start,
                end,
                format!("bytes {start}-{}/{}", end.saturating_sub(1), content_length),
            )
        }
    } else {
        return Err(s3s::S3Error::with_message(
            S3ErrorCode::InvalidRange,
            "unsupported range header",
        ));
    };
    Ok((start..end, Some(content_range)))
}
