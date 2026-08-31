use crate::config::AppConfig;
use crate::multipart::MultipartPartPlan;
use crate::object_format::{ObjectFormatService, sha256_hex};
use crate::redact;
use crate::telegram::{TelegramTransport, TelegramTransportStatus};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use http::{StatusCode, header};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use s3s::S3ErrorCode;
use s3s::auth::SimpleAuth;
use s3s::dto::{
    AbortMultipartUploadInput, AbortMultipartUploadOutput, Bucket, CompleteMultipartUploadInput,
    CompleteMultipartUploadOutput, CopyObjectInput, CopyObjectOutput, CopySource,
    CreateBucketInput, CreateBucketOutput, CreateMultipartUploadInput, CreateMultipartUploadOutput,
    DeleteBucketInput, DeleteBucketOutput, DeleteMarkerEntry, DeleteObjectInput,
    DeleteObjectOutput, ETag, ETagCondition, GetObjectInput, GetObjectOutput, HeadBucketInput,
    HeadBucketOutput, HeadObjectInput, HeadObjectOutput, ListBucketsInput, ListBucketsOutput,
    ListMultipartUploadsInput, ListMultipartUploadsOutput, ListObjectVersionsInput,
    ListObjectVersionsOutput, ListObjectsV2Input, ListObjectsV2Output, ListPartsInput,
    ListPartsOutput, MultipartUpload, Object, ObjectStorageClass, ObjectVersion,
    ObjectVersionStorageClass, Part, StreamingBlob, Timestamp, UploadPartCopyInput,
    UploadPartCopyOutput, UploadPartInput, UploadPartOutput,
};
use s3s::service::{S3Service, S3ServiceBuilder};
use s3s::{Body, S3, S3Request, S3Response, S3Result};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use time::OffsetDateTime;
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

impl TelegramS3Backend {
    fn resolve_manifest(
        &self,
        bucket: &str,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<Option<crate::manifest::ObjectManifest>, crate::object_format::ObjectFormatError>
    {
        if let Some(version_id) = version_id {
            self.object_format
                .get_manifest_by_version(bucket, key, version_id)
        } else {
            Ok(self.object_format.get_active_manifest(bucket, key)?)
        }
    }
}

#[derive(Clone, Debug)]
struct AllowAllAccess;

#[derive(Clone)]
struct AdminState {
    object_format: Arc<ObjectFormatService>,
    transport_status: TelegramTransportStatus,
    s3_addr: SocketAddr,
    admin_addr: SocketAddr,
}

#[async_trait]
impl s3s::access::S3Access for AllowAllAccess {
    async fn check(&self, _context: &mut s3s::access::S3AccessContext<'_>) -> S3Result<()> {
        Ok(())
    }
}

pub struct S3Server {
    addr: SocketAddr,
    admin_addr: SocketAddr,
    service: S3Service,
    object_format: Arc<ObjectFormatService>,
    transport_status: TelegramTransportStatus,
}

impl S3Server {
    pub async fn bootstrap(config: &AppConfig) -> Result<Self, S3ServerError> {
        config.validate()?;
        let transport = TelegramTransport::open(config.clone()).await?;
        let transport_status = transport.bootstrap().await?;
        let object_format = Arc::new(ObjectFormatService::open(config)?);
        let object_status = object_format.bootstrap()?;
        let admin_addr = config.admin_bind_addr()?;

        println!("object format bootstrap: {:?}", object_status);
        println!("telegram bootstrap: {:?}", transport_status.session_state);
        println!(
            "telegram bootstrap: session path {}",
            redact::redact_path(&transport_status.session_path.display().to_string())
        );

        let backend = TelegramS3Backend {
            object_format: Arc::clone(&object_format),
        };
        let mut builder = S3ServiceBuilder::new(backend);
        builder.set_auth(SimpleAuth::from_single(
            config.rustfs_access_key.clone().unwrap_or_default(),
            config.rustfs_secret_key.clone().unwrap_or_default(),
        ));
        builder.set_access(AllowAllAccess);
        let service = builder.build();
        Ok(Self {
            addr: config.s3_bind_addr()?,
            admin_addr,
            service,
            object_format,
            transport_status,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.addr
    }

    pub fn admin_address(&self) -> SocketAddr {
        self.admin_addr
    }

    pub async fn serve(self) -> Result<(), S3ServerError> {
        let listener = TcpListener::bind(self.addr).await?;
        println!("listening on {}", listener.local_addr()?);
        let admin_listener = TcpListener::bind(self.admin_addr).await?;
        let admin_addr = admin_listener.local_addr()?;
        println!("admin listening on {}", admin_addr);
        let service = self.service.clone();
        let admin_state = AdminState {
            object_format: Arc::clone(&self.object_format),
            transport_status: self.transport_status.clone(),
            s3_addr: self.addr,
            admin_addr,
        };
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
                accepted = admin_listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let admin_state = admin_state.clone();
                    connections.spawn(async move {
                        let handler = service_fn(move |request| handle_admin_request(request, admin_state.clone()));
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
    match timeout(
        Duration::from_secs(60),
        service.call(request.map(Body::from)),
    )
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => {
            eprintln!("s3 request failed: {error:?}");
            Ok(text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error\n".to_string(),
                "text/plain; charset=utf-8",
            ))
        }
        Err(_) => {
            eprintln!("s3 request timed out");
            Ok(text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "request timed out\n".to_string(),
                "text/plain; charset=utf-8",
            ))
        }
    }
}

async fn handle_admin_request(
    request: hyper::Request<Incoming>,
    state: AdminState,
) -> Result<hyper::Response<Body>, Box<dyn Error + Send + Sync>> {
    let path = request.uri().path();
    match path {
        "/healthz" => Ok(text_response(
            StatusCode::OK,
            format!(
                "ok\ns3_addr={}\nadmin_addr={}\ntransport_session_state={:?}\n",
                state.s3_addr, state.admin_addr, state.transport_status.session_state
            ),
            "text/plain; charset=utf-8",
        )),
        "/metrics" => {
            let metadata_status = state.object_format.metadata_status()?;
            let object_status = state.object_format.status()?;
            let body = format!(
                concat!(
                    "# HELP telegram_s3_bootstrap_ok Bootstrap completion state\n",
                    "# TYPE telegram_s3_bootstrap_ok gauge\n",
                    "telegram_s3_bootstrap_ok 1\n",
                    "# HELP telegram_s3_metadata_buckets Total visible buckets\n",
                    "# TYPE telegram_s3_metadata_buckets gauge\n",
                    "telegram_s3_metadata_buckets {}\n",
                    "# HELP telegram_s3_metadata_committed_objects Total committed objects\n",
                    "# TYPE telegram_s3_metadata_committed_objects gauge\n",
                    "telegram_s3_metadata_committed_objects {}\n",
                    "# HELP telegram_s3_metadata_staged_objects Total staged objects\n",
                    "# TYPE telegram_s3_metadata_staged_objects gauge\n",
                    "telegram_s3_metadata_staged_objects {}\n",
                    "# HELP telegram_s3_metadata_recovery_markers Total recovery markers\n",
                    "# TYPE telegram_s3_metadata_recovery_markers gauge\n",
                    "telegram_s3_metadata_recovery_markers {}\n",
                    "# HELP telegram_s3_object_format_recovery_required_objects Objects needing recovery\n",
                    "# TYPE telegram_s3_object_format_recovery_required_objects gauge\n",
                    "telegram_s3_object_format_recovery_required_objects {}\n",
                    "# HELP telegram_s3_object_format_orphaned_chunks Orphaned chunk directories\n",
                    "# TYPE telegram_s3_object_format_orphaned_chunks gauge\n",
                    "telegram_s3_object_format_orphaned_chunks {}\n",
                    "# HELP telegram_s3_transport_session_state Telegram session state snapshot\n",
                    "# TYPE telegram_s3_transport_session_state gauge\n",
                    "telegram_s3_transport_session_state{{state=\"{:?}\"}} 1\n"
                ),
                metadata_status.buckets,
                metadata_status.committed_objects,
                metadata_status.staged_objects,
                metadata_status.recovery_markers,
                object_status.recovery_required_objects,
                object_status.orphaned_chunks,
                state.transport_status.session_state,
            );
            Ok(text_response(
                StatusCode::OK,
                body,
                "text/plain; version=0.0.4; charset=utf-8",
            ))
        }
        _ => Ok(text_response(
            StatusCode::NOT_FOUND,
            "not found\n".to_string(),
            "text/plain; charset=utf-8",
        )),
    }
}

fn text_response(
    status: StatusCode,
    body: String,
    content_type: &'static str,
) -> hyper::Response<Body> {
    let mut response = hyper::Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    response
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
        let current_manifest = self
            .object_format
            .get_active_manifest(&input.bucket, &input.key)
            .map_err(map_object_error)?;
        enforce_write_conditionals(
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            current_manifest.as_ref(),
        )?;
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

    async fn create_multipart_upload(
        &self,
        req: S3Request<CreateMultipartUploadInput>,
    ) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
        let input = req.input;
        if !self
            .object_format
            .bucket_exists(&input.bucket)
            .map_err(map_object_error)?
        {
            return Err(s3s::s3_error!(NoSuchBucket, "bucket does not exist"));
        }
        let session = self
            .object_format
            .initiate_multipart_upload(
                &input.bucket,
                &input.key,
                input
                    .content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
                input
                    .checksum_algorithm
                    .as_ref()
                    .map(|algorithm| algorithm.as_str()),
            )
            .map_err(map_object_error)?;
        Ok(S3Response::new(CreateMultipartUploadOutput {
            bucket: Some(input.bucket),
            key: Some(input.key),
            upload_id: Some(session.upload_id.to_string()),
            checksum_algorithm: input.checksum_algorithm,
            ..Default::default()
        }))
    }

    async fn upload_part(
        &self,
        req: S3Request<UploadPartInput>,
    ) -> S3Result<S3Response<UploadPartOutput>> {
        let input = req.input;
        let upload_id = uuid::Uuid::parse_str(&input.upload_id).map_err(|error| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, error.to_string())
        })?;
        let part_number: u32 = input.part_number.try_into().map_err(|_| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, "invalid part number")
        })?;
        let part = self
            .object_format
            .upload_multipart_part(upload_id, part_number, input.body, None)
            .await
            .map_err(map_object_error)?;
        Ok(S3Response::new(UploadPartOutput {
            e_tag: Some(ETag::Strong(part.e_tag)),
            ..Default::default()
        }))
    }

    async fn upload_part_copy(
        &self,
        req: S3Request<UploadPartCopyInput>,
    ) -> S3Result<S3Response<UploadPartCopyOutput>> {
        let input = req.input;
        let source = parse_copy_source(&input.copy_source)?;
        let source_manifest = self
            .resolve_manifest(&source.bucket, &source.key, source.version_id.as_deref())
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "source object does not exist"))?;
        enforce_copy_source_conditionals(
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
            &source_manifest,
        )?;
        let range = input
            .copy_source_range
            .as_ref()
            .map(|range| parse_byte_range(range, source_manifest.content_length))
            .transpose()?
            .unwrap_or(0..source_manifest.content_length);
        let bytes = self
            .object_format
            .read_bytes(&source_manifest.bucket, &source_manifest.key, range)
            .map_err(map_object_error)?;
        let upload_id = uuid::Uuid::parse_str(&input.upload_id).map_err(|error| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, error.to_string())
        })?;
        let part_number: u32 = input.part_number.try_into().map_err(|_| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, "invalid part number")
        })?;
        let part = self
            .object_format
            .upload_multipart_part(
                upload_id,
                part_number,
                Some(StreamingBlob::from_bytes(Bytes::from(bytes))),
                None,
            )
            .await
            .map_err(map_object_error)?;
        Ok(S3Response::new(UploadPartCopyOutput {
            copy_part_result: Some(s3s::dto::CopyPartResult {
                e_tag: Some(ETag::Strong(part.e_tag)),
                last_modified: Some(Timestamp::from(source_manifest.created_at)),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    async fn complete_multipart_upload(
        &self,
        req: S3Request<CompleteMultipartUploadInput>,
    ) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
        let input = req.input;
        let upload_id = uuid::Uuid::parse_str(&input.upload_id).map_err(|error| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, error.to_string())
        })?;
        let completed = input
            .multipart_upload
            .and_then(|upload| upload.parts)
            .ok_or_else(|| s3s::s3_error!(InvalidArgument, "missing multipart parts"))?;
        let mut parts = Vec::with_capacity(completed.len());
        for part in completed {
            let part_number: u32 = part
                .part_number
                .ok_or_else(|| s3s::s3_error!(InvalidPart, "missing part number"))?
                .try_into()
                .map_err(|_| s3s::s3_error!(InvalidPart, "invalid part number"))?;
            let e_tag = match part.e_tag {
                Some(ETag::Strong(value)) => value,
                Some(ETag::Weak(value)) => value,
                None => return Err(s3s::s3_error!(InvalidPart, "missing part etag")),
            };
            parts.push(MultipartPartPlan {
                part_number,
                offset: 0,
                size: 0,
                checksum: e_tag.clone(),
                e_tag,
            });
        }
        let session = self
            .object_format
            .get_multipart_session(upload_id)
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchUpload, "multipart upload does not exist"))?;
        let current_manifest = self
            .object_format
            .get_active_manifest(&input.bucket, &input.key)
            .map_err(map_object_error)?;
        enforce_write_conditionals(
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            current_manifest.as_ref(),
        )?;
        let actual_parts = self
            .object_format
            .list_multipart_parts(upload_id)
            .map_err(map_object_error)?;
        let mut plan_parts = Vec::with_capacity(parts.len());
        for request_part in &parts {
            let stored = actual_parts
                .iter()
                .find(|part| part.part_number == request_part.part_number)
                .ok_or_else(|| s3s::s3_error!(InvalidPart, "missing part"))?;
            plan_parts.push(MultipartPartPlan {
                part_number: request_part.part_number,
                offset: 0,
                size: stored.size,
                checksum: stored.checksum.clone(),
                e_tag: stored.e_tag.clone(),
            });
        }
        let manifest = self
            .object_format
            .complete_multipart_upload(crate::multipart::MultipartCompletionPlan {
                upload_id,
                object_id: upload_id,
                bucket: session.bucket,
                key: session.key,
                content_type: session.content_type,
                checksum_algorithm: session.checksum_algorithm,
                content_length: plan_parts.iter().map(|part| part.size).sum(),
                parts: plan_parts,
            })
            .map_err(map_object_error)?;
        Ok(S3Response::new(CompleteMultipartUploadOutput {
            bucket: Some(manifest.bucket),
            key: Some(manifest.key),
            e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
            version_id: manifest.version_id,
            ..Default::default()
        }))
    }

    async fn abort_multipart_upload(
        &self,
        req: S3Request<AbortMultipartUploadInput>,
    ) -> S3Result<S3Response<AbortMultipartUploadOutput>> {
        let input = req.input;
        let upload_id = uuid::Uuid::parse_str(&input.upload_id).map_err(|error| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, error.to_string())
        })?;
        self.object_format
            .abort_multipart_upload(upload_id)
            .map_err(map_object_error)?;
        Ok(S3Response::new(AbortMultipartUploadOutput::default()))
    }

    async fn list_multipart_uploads(
        &self,
        req: S3Request<ListMultipartUploadsInput>,
    ) -> S3Result<S3Response<ListMultipartUploadsOutput>> {
        let input = req.input;
        let uploads = self
            .object_format
            .list_multipart_uploads(&input.bucket, input.prefix.as_deref())
            .map_err(map_object_error)?
            .into_iter()
            .map(|session| MultipartUpload {
                key: Some(session.key),
                initiated: Some(Timestamp::from(session.created_at)),
                upload_id: Some(session.upload_id.to_string()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        Ok(S3Response::new(ListMultipartUploadsOutput {
            bucket: Some(input.bucket),
            prefix: input.prefix,
            uploads: Some(uploads),
            ..Default::default()
        }))
    }

    async fn list_parts(
        &self,
        req: S3Request<ListPartsInput>,
    ) -> S3Result<S3Response<ListPartsOutput>> {
        let input = req.input;
        let upload_id = uuid::Uuid::parse_str(&input.upload_id).map_err(|error| {
            s3s::S3Error::with_message(S3ErrorCode::InvalidArgument, error.to_string())
        })?;
        let parts = self
            .object_format
            .list_multipart_parts(upload_id)
            .map_err(map_object_error)?
            .into_iter()
            .map(|part| Part {
                e_tag: Some(ETag::Strong(part.e_tag)),
                last_modified: Some(Timestamp::from(part.created_at)),
                part_number: Some(part.part_number.try_into().unwrap_or(i32::MAX)),
                size: Some(part.size as i64),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        Ok(S3Response::new(ListPartsOutput {
            bucket: Some(input.bucket),
            key: Some(input.key),
            upload_id: Some(input.upload_id),
            parts: Some(parts),
            ..Default::default()
        }))
    }

    async fn copy_object(
        &self,
        req: S3Request<CopyObjectInput>,
    ) -> S3Result<S3Response<CopyObjectOutput>> {
        let input = req.input;
        let source = parse_copy_source(&input.copy_source)?;
        let manifest = self
            .resolve_manifest(&source.bucket, &source.key, source.version_id.as_deref())
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "source object does not exist"))?;
        enforce_copy_source_conditionals(
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
            &manifest,
        )?;
        let bytes = self
            .object_format
            .read_bytes(&manifest.bucket, &manifest.key, 0..manifest.content_length)
            .map_err(map_object_error)?;
        let content_type = input
            .content_type
            .clone()
            .unwrap_or(manifest.content_type.clone());
        let copied = self
            .object_format
            .put_bytes(&input.bucket, &input.key, &content_type, &bytes)
            .map_err(map_object_error)?;
        let copy_result = s3s::dto::CopyObjectResult {
            e_tag: Some(ETag::Strong(copied.checksum.whole_object.clone())),
            last_modified: Some(Timestamp::from(copied.created_at)),
            ..Default::default()
        };
        Ok(S3Response::new(CopyObjectOutput {
            copy_object_result: Some(copy_result),
            version_id: copied.version_id,
            ..Default::default()
        }))
    }

    async fn head_object(
        &self,
        req: S3Request<HeadObjectInput>,
    ) -> S3Result<S3Response<HeadObjectOutput>> {
        let input = req.input;
        let manifest = self
            .resolve_manifest(&input.bucket, &input.key, input.version_id.as_deref())
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "object does not exist"))?;
        if manifest.commit_state != crate::manifest::CommitState::Committed {
            return Err(s3s::s3_error!(NoSuchKey, "object does not exist"));
        }
        enforce_read_conditionals(
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            input.if_modified_since.as_ref(),
            input.if_unmodified_since.as_ref(),
            &manifest,
        )?;
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
        let input = req.input;
        let manifest = self
            .resolve_manifest(&input.bucket, &input.key, input.version_id.as_deref())
            .map_err(map_object_error)?
            .ok_or_else(|| s3s::s3_error!(NoSuchKey, "object does not exist"))?;
        if manifest.commit_state != crate::manifest::CommitState::Committed {
            return Err(s3s::s3_error!(NoSuchKey, "object does not exist"));
        }
        enforce_read_conditionals(
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            input.if_modified_since.as_ref(),
            input.if_unmodified_since.as_ref(),
            &manifest,
        )?;
        let headers = req.headers.clone();
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
            let plaintext = if manifest.encryption.enabled {
                object_format
                    .decrypt_chunk(manifest.object_id, span.order, &chunk)
                    .map_err(map_object_error)?
            } else {
                chunk
            };
            let actual_checksum = sha256_hex(&plaintext);
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
            if plaintext.len() < end {
                return Err(s3s::S3Error::with_message(
                    S3ErrorCode::InternalError,
                    "chunk shorter than planned span",
                ));
            }
            let actual = &plaintext[start..end];
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
        let manifest_encryption_enabled = manifest.encryption.enabled;
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
                let plaintext = if manifest_encryption_enabled {
                    match object_format
                        .decrypt_chunk(object_id, span.order, &chunk)
                        .map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
                        }) {
                        Ok(plaintext) => plaintext,
                        Err(error) => {
                            return Some((
                                Err::<Bytes, std::io::Error>(error),
                                (index, spans, true),
                            ));
                        }
                    }
                } else {
                    chunk
                };
                let actual_checksum = sha256_hex(&plaintext);
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
                if plaintext.len() < end {
                    return Some((
                        Err::<Bytes, std::io::Error>(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "chunk shorter than planned span",
                        )),
                        (index, spans, true),
                    ));
                }
                let actual = &plaintext[start..end];
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
        let deleted = if let Some(version_id) = input.version_id.as_deref() {
            let manifest = self
                .resolve_manifest(&input.bucket, &input.key, Some(version_id))
                .map_err(map_object_error)?
                .ok_or_else(|| s3s::s3_error!(NoSuchKey, "object does not exist"))?;
            enforce_delete_conditionals(
                input.if_match.as_ref(),
                input.if_match_last_modified_time.as_ref(),
                input.if_match_size,
                &manifest,
            )?;
            Some(
                self.object_format
                    .tombstone_manifest(manifest.object_id, "deleted via S3")
                    .map_err(map_object_error)?,
            )
        } else {
            if let Some(manifest) = self
                .object_format
                .get_active_manifest(&input.bucket, &input.key)
                .map_err(map_object_error)?
            {
                enforce_delete_conditionals(
                    input.if_match.as_ref(),
                    input.if_match_last_modified_time.as_ref(),
                    input.if_match_size,
                    &manifest,
                )?;
            }
            self.object_format
                .delete_object(&input.bucket, &input.key)
                .map_err(map_object_error)?
        };
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

    async fn list_object_versions(
        &self,
        req: S3Request<ListObjectVersionsInput>,
    ) -> S3Result<S3Response<ListObjectVersionsOutput>> {
        let input = req.input;
        if !self
            .object_format
            .bucket_exists(&input.bucket)
            .map_err(map_object_error)?
        {
            return Err(s3s::s3_error!(NoSuchBucket, "bucket does not exist"));
        }
        let prefix = input.prefix.clone().unwrap_or_default();
        let mut manifests = self
            .object_format
            .list_manifests()
            .map_err(map_object_error)?
            .into_iter()
            .filter(|manifest| manifest.bucket == input.bucket)
            .filter(|manifest| prefix.is_empty() || manifest.key.starts_with(&prefix))
            .collect::<Vec<_>>();
        manifests.sort_by(|left, right| right.created_at.cmp(&left.created_at));

        let mut latest_key = HashMap::<String, String>::new();
        for manifest in &manifests {
            latest_key
                .entry(manifest.key.clone())
                .or_insert_with(|| manifest.object_id.to_string());
        }

        let mut versions = Vec::new();
        let mut delete_markers = Vec::new();
        for manifest in manifests {
            let is_latest = latest_key
                .get(&manifest.key)
                .map(|object_id| object_id == &manifest.object_id.to_string())
                .unwrap_or(false);
            match manifest.commit_state {
                crate::manifest::CommitState::Committed => versions.push(ObjectVersion {
                    key: Some(manifest.key),
                    last_modified: Some(Timestamp::from(manifest.created_at)),
                    e_tag: Some(ETag::Strong(manifest.checksum.whole_object)),
                    is_latest: Some(is_latest),
                    size: Some(manifest.content_length as i64),
                    storage_class: Some(ObjectVersionStorageClass::from_static(
                        ObjectVersionStorageClass::STANDARD,
                    )),
                    version_id: Some(manifest.object_id.to_string()),
                    ..Default::default()
                }),
                crate::manifest::CommitState::Tombstoned => {
                    delete_markers.push(DeleteMarkerEntry {
                        key: Some(manifest.key),
                        last_modified: Some(Timestamp::from(manifest.created_at)),
                        is_latest: Some(is_latest),
                        version_id: Some(manifest.object_id.to_string()),
                        ..Default::default()
                    })
                }
                _ => {}
            }
        }

        Ok(S3Response::new(ListObjectVersionsOutput {
            name: Some(input.bucket),
            prefix: Some(prefix),
            versions: Some(versions),
            delete_markers: Some(delete_markers),
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

fn etag_condition_matches(condition: &ETagCondition, actual: &str) -> bool {
    match condition {
        ETagCondition::Any => true,
        ETagCondition::ETag(etag) => {
            etag.as_strong() == Some(actual) || etag.as_weak() == Some(actual)
        }
    }
}

fn enforce_read_conditionals(
    if_match: Option<&ETagCondition>,
    if_none_match: Option<&ETagCondition>,
    if_modified_since: Option<&Timestamp>,
    if_unmodified_since: Option<&Timestamp>,
    manifest: &crate::manifest::ObjectManifest,
) -> S3Result<()> {
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(condition) = if_match {
        if !etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match precondition failed"
            ));
        }
        return Ok(());
    }
    if let Some(condition) = if_unmodified_since {
        let since: OffsetDateTime = condition.clone().into();
        if manifest.created_at > since {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-unmodified-since precondition failed"
            ));
        }
    }
    if let Some(condition) = if_none_match {
        if etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(NotModified, "object not modified"));
        }
    }
    if let Some(condition) = if_modified_since {
        let since: OffsetDateTime = condition.clone().into();
        if manifest.created_at <= since {
            return Err(s3s::s3_error!(NotModified, "object not modified"));
        }
    }
    Ok(())
}

fn enforce_write_conditionals(
    if_match: Option<&ETagCondition>,
    if_none_match: Option<&ETagCondition>,
    current_manifest: Option<&crate::manifest::ObjectManifest>,
) -> S3Result<()> {
    let Some(manifest) = current_manifest else {
        if if_match.is_some() {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match precondition failed"
            ));
        }
        return Ok(());
    };
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(condition) = if_match {
        if !etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match precondition failed"
            ));
        }
        return Ok(());
    }
    if let Some(condition) = if_none_match {
        if etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-none-match precondition failed"
            ));
        }
    }
    Ok(())
}

fn enforce_copy_source_conditionals(
    if_match: Option<&ETagCondition>,
    if_none_match: Option<&ETagCondition>,
    if_modified_since: Option<&Timestamp>,
    if_unmodified_since: Option<&Timestamp>,
    manifest: &crate::manifest::ObjectManifest,
) -> S3Result<()> {
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(condition) = if_match {
        if !etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "copy-source if-match precondition failed"
            ));
        }
        return Ok(());
    }
    if let Some(condition) = if_unmodified_since {
        let since: OffsetDateTime = condition.clone().into();
        if manifest.created_at > since {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "copy-source if-unmodified-since precondition failed"
            ));
        }
    }
    if let Some(condition) = if_none_match {
        if etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "copy-source if-none-match precondition failed"
            ));
        }
    }
    if let Some(condition) = if_modified_since {
        let since: OffsetDateTime = condition.clone().into();
        if manifest.created_at <= since {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "copy-source if-modified-since precondition failed"
            ));
        }
    }
    Ok(())
}

fn enforce_delete_conditionals(
    if_match: Option<&ETagCondition>,
    if_match_last_modified_time: Option<&Timestamp>,
    if_match_size: Option<i64>,
    manifest: &crate::manifest::ObjectManifest,
) -> S3Result<()> {
    let current_etag = manifest.checksum.whole_object.as_str();
    if let Some(condition) = if_match {
        if !etag_condition_matches(condition, current_etag) {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match precondition failed"
            ));
        }
    }
    if let Some(condition) = if_match_last_modified_time {
        let expected: OffsetDateTime = condition.clone().into();
        if manifest.created_at != expected {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match-last-modified-time precondition failed"
            ));
        }
    }
    if let Some(condition) = if_match_size {
        if manifest.content_length as i64 != condition {
            return Err(s3s::s3_error!(
                PreconditionFailed,
                "if-match-size precondition failed"
            ));
        }
    }
    Ok(())
}

struct CopySourceRef {
    bucket: String,
    key: String,
    version_id: Option<String>,
}

fn parse_copy_source(copy_source: &CopySource) -> Result<CopySourceRef, s3s::S3Error> {
    match copy_source {
        CopySource::Bucket {
            bucket,
            key,
            version_id,
        } => Ok(CopySourceRef {
            bucket: bucket.to_string(),
            key: key.to_string(),
            version_id: version_id.as_ref().map(|value| value.to_string()),
        }),
        _ => Err(s3s::S3Error::with_message(
            S3ErrorCode::InvalidArgument,
            "unsupported copy source format",
        )),
    }
}

fn parse_byte_range(
    value: &str,
    content_length: u64,
) -> Result<std::ops::Range<u64>, s3s::S3Error> {
    let value = value.strip_prefix("bytes=").ok_or_else(|| {
        s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "unsupported range header")
    })?;
    let (start, end) = value.split_once('-').ok_or_else(|| {
        s3s::S3Error::with_message(S3ErrorCode::InvalidRange, "invalid range header")
    })?;
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
    Ok(start..end)
}
