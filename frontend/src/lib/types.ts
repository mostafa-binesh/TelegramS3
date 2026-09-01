export interface SessionState {
  authenticated: boolean;
  issued_at?: string | null;
  expires_at?: string | null;
  csrf_token?: string | null;
}

export interface CheckItem {
  label: string;
  ok: boolean;
  detail: string;
}

export interface BootstrapState {
  ready: boolean;
  authenticated: boolean;
  session_state: string;
  phone_number?: string | null;
  proxy_mode: string;
  proxy_url?: string | null;
  checks: CheckItem[];
}

export interface OverviewState {
  checked_at: string;
  session: Required<SessionState>;
  endpoint: {
    s3_bind_addr: string;
    admin_bind_addr: string;
    admin_route_prefix: string;
  };
  storage: {
    metadata_path: string;
    data_dir: string;
    session_path: string;
    buckets: number;
    committed_objects: number;
    active_objects: number;
    staged_objects: number;
    recovery_markers: number;
  };
  capacity: {
    chunk_size: number;
    recovery_required_objects: number;
    orphaned_chunks: number;
  };
  telegram: {
    session_state: string;
    proxy_kind: string;
    proxy_url?: string | null;
    phone_number?: string | null;
  };
  bootstrap: BootstrapState;
}
