export interface UserInfo {
  id: string;
  username: string;
  display_name: string;
  role: 'admin' | 'superadmin';
  disabled: boolean;
}

export interface SessionState {
  authenticated: boolean;
  user?: UserInfo | null;
  issued_at?: string | null;
  expires_at?: string | null;
  csrf_token?: string | null;
}

export interface BucketInfo {
  name: string;
  created_at: string;
  replica_accounts?: number;
  access_accounts?: number;
  replica_chunk_size_mismatch?: boolean;
}

export interface BucketsState {
  buckets: BucketInfo[];
  page?: number;
  page_size?: number;
  total?: number;
  has_more?: boolean;
  search?: string;
}

export interface ObjectEntry {
  name: string;
  key: string;
  size: number;
  last_modified: string;
  etag: string;
  expires_at?: string | null;
  shared_links: number;
  /** Parent folder path, supplied for recursive search results. */
  location?: string | null;
  /** True when this row represents a durable upload that is not committed yet. */
  uploading?: boolean;
  upload_state?: string;
  upload_job_id?: string;
  upload_bytes?: number;
  upload_parts_done?: number;
  upload_parts_total?: number;
  upload_error?: string | null;
  replica_accounts?: number;
  access_accounts?: number;
  replica_chunk_size_mismatch?: boolean;
  rechunking?: boolean;
}

export type SortDirection = 'asc' | 'desc';
export type BucketSortKey = 'name' | 'created_at' | 'accounts';
export type ObjectSortKey = 'name' | 'size' | 'last_modified';

export interface SearchResult extends ObjectEntry {
  bucket: string;
}

export interface SearchState {
  results: SearchResult[];
  page?: number;
  page_size?: number;
  total?: number;
  has_more?: boolean;
  search?: string;
}

export interface SharedLink {
  id: string;
  url?: string | null;
  description: string;
  created_at: string;
  expires_at?: string | null;
  status: 'active' | 'expired' | string;
}

export interface SharedLinksState {
  bucket: string;
  key: string;
  links: SharedLink[];
  count: number;
}

export interface ObjectsState {
  prefix: string;
  folders: string[];
  objects: ObjectEntry[];
  page?: number;
  page_size?: number;
  total?: number;
  has_more?: boolean;
  search?: string;
}

export interface UsersState {
  users: UserInfo[];
}

export interface StorageCard {
  buckets: number;
  committed_objects: number;
  active_objects: number;
  staged_objects: number;
  recovery_markers: number;
  chunk_size: number;
  recovery_required_objects: number;
  telegram_files_bytes?: number;
  metadata_path?: string;
  data_dir?: string;
}

export interface DurableMetrics {
  pending_jobs: number;
  oldest_pending_age_seconds: number;
  retries: number;
  failed_jobs: number;
  staging_bytes: number;
  cleanup_backlog: number;
  cleanup_due: number;
  cleanup_scheduled: number;
  cleanup_recovery_required: number;
}

export interface TrafficSnapshot {
  client_upload_bytes: number;
  client_download_bytes: number;
  telegram_upload_bytes: number;
  telegram_download_bytes: number;
}

export interface TrafficMetrics {
  session: TrafficSnapshot;
  total: TrafficSnapshot;
}

export interface DownloadStageSample {
  request_id: number;
  surface: string;
  started_at: string;
  status: string;
  chunks: number;
  client_bytes: number;
  telegram_bytes: number;
  telegram_retries: number;
  prefetch_mode?: string;
  prefetch_window_max?: number;
  prefetch_window_final?: number;
  accounts?: DownloadAccountStageSample[];
  first_chunk_us?: number | null;
  telegram_us: number;
  retry_wait_us: number;
  decrypt_us: number;
  verify_us: number;
  total_us: number;
  error?: string | null;
}

export interface DownloadStageActive {
  request_id: number;
  surface: string;
  object: string;
  mode: string;
  total_chunks: number;
  server_chunks: number;
  client_chunks: number;
  current_chunk?: number | null;
  client_bytes: number;
  telegram_bytes: number;
  telegram_retries: number;
  prefetch_window_max: number;
  prefetch_window_final: number;
}

export interface DownloadAccountStageSample {
  account_id: string;
  chunks: number;
  telegram_bytes: number;
  telegram_retries: number;
  telegram_us: number;
}

export interface DownloadStageMetrics {
  active_requests: number;
  completed_requests: number;
  failed_requests: number;
  test_active_requests: number;
  active?: DownloadStageActive[];
  last_test?: DownloadStageSample | null;
  recent: DownloadStageSample[];
}

export interface RecoveryIssue {
  /** Stable fingerprint used to acknowledge this issue. */
  id: string;
  object_id?: string | null;
  bucket?: string | null;
  key?: string | null;
  path?: string | null;
  commit_state?: string | null;
  kind: string;
  summary: string;
  details: string[];
  account_id?: string | null;
  account_label?: string | null;
  chunk_order?: number | null;
  repair_state?: string | null;
  acknowledged_at?: string | null;
  acknowledged_by?: string | null;
}

export interface RecoveryState {
  /** Every issue the scan found, acknowledged or not. */
  issue_count: number;
  /** The actionable subset — what the Overview leads with. */
  unacknowledged_count: number;
  scan_ok: boolean;
  scan_error?: string | null;
  checked_at?: string | null;
  issues: RecoveryIssue[];
}

export interface VerifierState {
  enabled: boolean;
  interval_secs: number;
  chunks_per_object: number;
  status: 'pending' | 'healthy' | 'attention' | 'unavailable' | string;
  broken_files: number;
  last_run_at?: string | null;
  next_run_at?: string | null;
  scan_runs?: number;
  scan_failures?: number;
  last_scan_started_at?: string | null;
  last_scan_finished_at?: string | null;
  last_scan_duration_ms?: number | null;
  problems: RecoveryIssue[];
}

export interface OverviewState {
  checked_at?: string;
  telegram_last_success_at?: string | null;
  session?: { authenticated: boolean; user?: UserInfo };
  storage?: StorageCard;
  transfers?: DurableMetrics;
  traffic?: TrafficMetrics;
  stage_metrics?: DownloadStageMetrics;
  recovery?: RecoveryState;
  verifier?: VerifierState;
  telegram?: {
    session_state: string;
    connection_state: string;
    detail: string;
    storage_chat_id?: string | null;
    accounts?: Array<{
      id: string;
      label: string;
      state: string;
      detail: string;
      connected: boolean;
      download_enabled: boolean;
    }>;
  };
  checks?: { label: string; ok: boolean; detail: string }[];
  connection_removal?: {
    id: string;
    connection_id: string;
    delete_uploaded_files: boolean;
    state: string;
    object_count: number;
    requested_at: number;
    updated_at: number;
    completed_at?: number | null;
    error?: string | null;
  } | null;
}

export type OverviewLiveState = Pick<
  OverviewState,
  'checked_at' | 'telegram_last_success_at' | 'transfers' | 'traffic' | 'stage_metrics' | 'telegram' | 'connection_removal' | 'checks'
>;

export interface TelegramSettings {
  telegram_api_id: string;
  telegram_api_hash: string;
  telegram_storage_chat_id: string;
  telegram_proxy_url: string;
  telegram_proxy_username: string;
  telegram_proxy_password: string;
  telegram_proxy_mode: string;
  telegram_account_phone?: string | null;
}

export interface TelegramSettingsState {
  settings: TelegramSettings;
  refresh_error?: string | null;
}

export interface StorageSettings {
  chunk_size: number;
  min_chunk_size: number;
  max_chunk_size: number;
  download_prefetch_chunks: number;
  min_download_prefetch_chunks: number;
  max_download_prefetch_chunks: number;
  download_prefetch_mode: 'adaptive' | 'sequential' | string;
  download_prefetch_modes?: string[];
  download_failover_retries: number;
  min_download_failover_retries: number;
  max_download_failover_retries: number;
  recovery_verify_enabled: boolean;
  recovery_verify_startup: boolean;
  recovery_verify_interval_secs: number;
  min_recovery_verify_interval_secs: number;
  max_recovery_verify_interval_secs: number;
  recovery_verify_chunks: number;
  min_recovery_verify_chunks: number;
  max_recovery_verify_chunks: number;
  cleanup_retention_secs: number;
  min_cleanup_retention_secs: number;
  max_cleanup_retention_secs: number;
  source: 'database' | string;
}

export interface StorageSettingsState extends StorageSettings {}

export type WizardPhase = 'idle' | 'code' | 'two_fa' | 'authorized';

export interface WizardState {
  phase: WizardPhase;
  needs_2fa: boolean;
  authorized: boolean;
  connection_ready?: boolean;
  connection_state?: string;
  health_detail?: string;
  owner?: string | null;
  message?: string | null;
}

export interface FileUploadResult {
  size: number;
  etag: string;
  version_id: string;
}

export interface TransferJob {
  id:string; object_id:string; operation_id:string|null; bucket:string; key:string; state:string;
  bytes:number; chunks_done:number; chunks_total:number; attempts:number; next_retry:number;
  error:string|null; created_at:number; updated_at:number;
}

export interface MultipartUpload {
  upload_id: string; bucket: string; key: string; state: string;
  parts_done: number; parts_total: number; updated_at: number;
}

export interface AccountInfo {
  id: string;
  label: string;
  phone?: string | null;
  state: string;
  storage_chat_id?: string | null;
  replica_objects: number;
  access_objects: number;
  download_enabled: boolean;
  created_at: number;
  updated_at: number;
}

export interface ReplicationJob {
  id: string;
  source_account_id: string;
  target_account_id: string;
  bucket: string;
  mode: 'one_time' | 'automatic' | string;
  access_mode: 'replica' | 'access' | string;
  object_keys?: string[];
  state: string;
  objects_total: number;
  objects_done: number;
  chunks_total: number;
  chunks_done: number;
  bytes_done: number;
  next_run?: number | null;
  last_run?: number | null;
  error?: string | null;
  created_at: number;
  updated_at: number;
}

export interface RechunkJob {
  id: string;
  bucket: string;
  key: string;
  object_id: string;
  source_account_id: string;
  new_chunk_size: number;
  apply_to_replicas: boolean;
  replica_targets: {account_id: string; access_mode: string}[];
  state: string;
  chunks_total: number;
  chunks_done: number;
  bytes_done: number;
  error?: string | null;
  created_at: number;
  updated_at: number;
}

export interface ReplicaInfo {
  object_id: string;
  bucket: string;
  key: string;
  chunk_order: number;
  account_id: string;
  account_label: string;
  mode: string;
  peer_id: string;
  message_id: number;
  document_id?: string | null;
  state: string;
  error?: string | null;
  updated_at: number;
}
