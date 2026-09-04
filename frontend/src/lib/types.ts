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
  object_id?: string | null;
  bucket?: string | null;
  key?: string | null;
  path?: string | null;
  commit_state?: string | null;
  kind: string;
  summary: string;
  details: string[];
}

export interface RecoveryState {
  issue_count: number;
  scan_ok: boolean;
  scan_error?: string | null;
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
}

export interface TelegramSettings {
  telegram_api_id: string;
  telegram_api_hash: string;
  telegram_storage_chat_id: string;
  telegram_proxy_url: string;
  telegram_proxy_username: string;
  telegram_proxy_password: string;
  telegram_proxy_mode: string;
}

export interface TelegramSettingsState {
  settings: TelegramSettings;
}

export type WizardPhase = 'idle' | 'code' | 'two_fa' | 'authorized';

export interface WizardState {
  phase: WizardPhase;
  needs_2fa: boolean;
  authorized: boolean;
  owner?: string | null;
  message?: string | null;
}

export interface FileUploadResult {
  size: number;
  etag: string;
  version_id: string;
}
