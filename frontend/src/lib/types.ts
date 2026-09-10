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
}

export interface BucketsState {
  buckets: BucketInfo[];
}

export interface ObjectEntry {
  name: string;
  key: string;
  size: number;
  last_modified: string;
  etag: string;
  expires_at?: string | null;
}

export interface ObjectsState {
  prefix: string;
  folders: string[];
  objects: ObjectEntry[];
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
  metadata_path?: string;
  data_dir?: string;
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

export interface OverviewState {
  checked_at?: string;
  session?: { authenticated: boolean; user?: UserInfo };
  storage?: StorageCard;
  recovery?: RecoveryState;
  telegram?: {
    session_state: string;
    connection_state: string;
    detail: string;
    storage_chat_id?: string | null;
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
