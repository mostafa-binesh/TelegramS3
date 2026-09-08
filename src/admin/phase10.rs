use super::*;

impl AdminUiState {
    pub(super) async fn setup_account(&self, request: Request<Incoming>) -> Response<Body> {
        // Only the configured same-origin JSON UI can provision the first account.
        if !same_origin(&request) {
            return json_error(StatusCode::FORBIDDEN, "cross-origin setup is not allowed");
        }
        let keys = LoginLimiter::keys_for(None, "first-account-setup");
        if let Some(error) = self.limiter.check(&keys) {
            return auth_error_response(&error);
        }
        self.limiter.record_failure(&keys);
        if !matches!(self.store().setup_required(), Ok(true)) {
            return json_error(StatusCode::CONFLICT, "setup is already complete");
        }
        let body = match read_json::<CreateUserRequest>(request).await {
            Ok(v) => v,
            Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid account payload"),
        };
        let username = match auth::normalize_username(&body.username) {
            Ok(v) => v,
            Err(e) => return auth_error_response(&e),
        };
        if let Err(e) = auth::validate_password(&body.password) {
            return auth_error_response(&e);
        }
        let object = Arc::clone(&self.object_format);
        let result = tokio::task::spawn_blocking(move || {
            let hash = auth::hash_password(&body.password).map_err(|e| e.to_string())?;
            object
                .metadata_store()
                .create_first_user(&username, &hash, &body.display_name)
                .map_err(|e| e.to_string())
        })
        .await;
        let user = match result {
            Ok(Ok(user)) => user,
            Ok(Err(_)) => {
                return json_error(
                    StatusCode::CONFLICT,
                    "setup could not complete; reload to check account state",
                );
            }
            Err(_) => {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, "account worker failed");
            }
        };
        let issued = match self.issue_session(&user, None) {
            Ok(v) => v,
            Err(e) => return auth_error_response(&e),
        };
        let mut response = json_response(
            StatusCode::CREATED,
            SessionResponse {
                authenticated: true,
                user: Some(issued.user),
                issued_at: Some(issued.issued_at),
                expires_at: Some(issued.expires_at),
                csrf_token: Some(issued.csrf_token),
            },
        );
        with_set_cookie(&mut response, issued.cookie_value);
        response
    }

    pub(super) async fn enqueue_upload(&self, request: Request<Incoming>) -> Response<Body> {
        let mut params = request
            .uri()
            .query()
            .map(parse_list_params)
            .unwrap_or_default();
        let bucket = params.remove("bucket").unwrap_or_default();
        let key = params.remove("key").unwrap_or_default();
        if bucket.is_empty() || !is_safe_object_key(&key) {
            return json_error(
                StatusCode::BAD_REQUEST,
                "bucket and valid object key required",
            );
        }
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        match self
            .object_format
            .enqueue_stream(
                &bucket,
                &key,
                &content_type,
                Some(body_to_streaming_blob(request.into_body())),
                None,
            )
            .await
        {
            Ok(job) => json_response(
                StatusCode::ACCEPTED,
                serde_json::json!({"job_id":job.id,"status_url":format!("/_admin/api/jobs/{}",job.id),"job":job}),
            ),
            Err(error) => {
                let (status, message) = match &error {
                    crate::object_format::ObjectFormatError::Metadata(
                        crate::metadata::MetadataError::BucketNotFound(_),
                    ) => (StatusCode::NOT_FOUND, "bucket not found"),
                    crate::object_format::ObjectFormatError::Metadata(
                        crate::metadata::MetadataError::InvalidManifest(m),
                    ) if m.contains("capacity") => (
                        StatusCode::INSUFFICIENT_STORAGE,
                        "staging capacity exhausted; free space or finish pending transfers",
                    ),
                    _ => (
                        StatusCode::BAD_REQUEST,
                        "upload reception failed; staged data retained, resend source file",
                    ),
                };
                json_error(status, message)
            }
        }
    }

    pub(super) fn job_api(&self, request: Request<Incoming>, rest: &str) -> Response<Body> {
        if rest == "jobs" && request.method() == Method::GET {
            let params = request
                .uri()
                .query()
                .map(parse_list_params)
                .unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(50)
                .clamp(1, 100);
            let offset = params
                .get("offset")
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            return match self.store().transfers(limit, offset) {
                Ok(jobs) => json_response(
                    StatusCode::OK,
                    serde_json::json!({"jobs":jobs,"limit":limit,"offset":offset,"next_offset":if jobs.len()==limit as usize {Some(offset.saturating_add(limit))}else{None}}),
                ),
                Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "job store unavailable"),
            };
        }
        let segments: Vec<_> = rest.split('/').collect();
        let Some(id) = segments
            .get(1)
            .copied()
            .filter(|id| Uuid::parse_str(id).is_ok())
        else {
            return json_error(StatusCode::NOT_FOUND, "job not found");
        };
        if request.method() == Method::GET && segments.len() == 2 {
            return match self.store().transfer(id) {
                Ok(Some(job)) => json_response(StatusCode::OK, job),
                Ok(None) => json_error(StatusCode::NOT_FOUND, "job not found"),
                Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "job store unavailable"),
            };
        }
        if request.method() == Method::POST && segments.len() == 3 {
            return match self.store().transfer_action(id, segments[2]) {
                Ok(true) => json_response(StatusCode::OK, serde_json::json!({"ok":true})),
                Ok(false) => json_error(
                    StatusCode::CONFLICT,
                    "action unavailable in this job state; active publication must finish",
                ),
                Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "job update failed"),
            };
        }
        json_error(StatusCode::NOT_FOUND, "route not found")
    }
}

pub(super) fn same_origin(request: &Request<Incoming>) -> bool {
    if request
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        == Some("cross-site")
    {
        return false;
    }
    match request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        None => true,
        Some(origin) => url::Url::parse(origin).ok().is_some_and(|u| {
            let authority = match u.port() {
                Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
                None => u.host_str().unwrap_or("").to_string(),
            };
            request
                .headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                == Some(authority.as_str())
        }),
    }
}
