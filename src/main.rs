use clap::{Parser, Subcommand};
use telegram_s3::{
    AppConfig, MetadataStore, S3Server,
    object_format::GARBAGE_COLLECTION_RETENTION_SECONDS,
    object_format::{ObjectFormatError, ObjectFormatService, ObjectFormatStatus},
    redact,
    telegram::{TelegramTransport, TelegramTransportError, TelegramTransportStatus},
};

#[derive(Parser, Debug)]
#[command(name = "telegram-s3", version, about = "Telegram-backed S3 storage")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Server,
    Doctor,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    Repair {
        #[arg(long)]
        dry_run: bool,
    },
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
    Users {
        #[command(subcommand)]
        command: UsersCommand,
    },
    Upstream {
        #[command(subcommand)]
        command: UpstreamCommand,
    },
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    Login,
    Status,
    Logout,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    Check,
}

#[derive(Subcommand, Debug)]
enum IndexCommand {
    Rebuild,
    Verify,
}

#[derive(Subcommand, Debug)]
enum DbCommand {
    Migrate,
    Status,
}

#[derive(Subcommand, Debug)]
enum UpstreamCommand {
    Status,
}

#[derive(Subcommand, Debug)]
enum UsersCommand {
    /// Create an operator account. The first account ever created becomes the
    /// superadmin automatically, so `users create` seeds the control plane on a
    /// pristine database (the fail-safe path while the server is down).
    Create {
        /// Username (case-insensitive).
        username: String,
        /// Password; if omitted, read from TG_ADMIN_PASSWORD (preferred for
        /// scripted first boot) or prompt on a TTY.
        #[arg(long, short = 'p')]
        password: Option<String>,
        /// Role for non-first accounts ('admin' default; first is forced to 'superadmin').
        #[arg(long, default_value = "admin")]
        role: String,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// List operator accounts (prints no password material).
    List,
    /// Change an account's password, revoking all its sessions.
    Password {
        /// Username.
        username: String,
        /// New password; omitted => read from TG_ADMIN_PASSWORD (required).
        #[arg(long, short = 'p')]
        password: Option<String>,
    },
    /// Delete an operator account.
    Delete { username: String },
    /// Show non-secret status for one account.
    Status { username: String },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Auth { command } => run_auth(command).await,
        Command::Server => run_server().await,
        Command::Doctor => run_doctor().await,
        Command::Config { command } => run_config(command),
        Command::Index { command } => run_index(command),
        Command::Db { command } => run_db(command),
        Command::Repair { dry_run } => run_repair(dry_run),
        Command::Gc { dry_run } => run_gc(dry_run),
        Command::Users { command } => run_users(command),
        Command::Upstream { command } => run_upstream(command),
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run_auth(command: AuthCommand) -> Result<(), String> {
    match command {
        AuthCommand::Login => {
            let transport = open_transport().await?;
            let state = transport
                .interactive_login()
                .await
                .map_err(render_transport_error)?;
            print_transport_state("login", &state)?;
            Ok(())
        }
        AuthCommand::Status => {
            let transport = open_transport().await?;
            let status = transport.status().await.map_err(render_transport_error)?;
            print_transport_status("auth status", &status)?;
            Ok(())
        }
        AuthCommand::Logout => {
            let transport = open_transport().await?;
            let state = transport.logout().await.map_err(render_transport_error)?;
            println!("logout complete: {state:?}");
            Ok(())
        }
    }
}

async fn run_doctor() -> Result<(), String> {
    let config = load_config()?;
    let server = S3Server::bootstrap(&config)
        .await
        .map_err(|error| error.to_string())?;
    let object_format = open_object_format(&config)?;
    let object_status = object_format
        .bootstrap()
        .map_err(render_object_format_error)?;
    let metadata_status = object_format
        .metadata_status()
        .map_err(render_object_format_error)?;
    println!("configuration looks structurally valid");
    println!("s3 bind address: {}", server.address());
    println!(
        "metadata schema version: {}",
        metadata_status.schema_version
    );
    println!("committed objects: {}", metadata_status.committed_objects);
    println!("active objects: {}", metadata_status.active_objects);
    println!("staged objects: {}", metadata_status.staged_objects);
    println!("recovery markers: {}", metadata_status.recovery_markers);
    print_object_format_status("object format", &object_status)?;
    Ok(())
}

fn run_config(command: ConfigCommand) -> Result<(), String> {
    match command {
        ConfigCommand::Check => {
            let config = load_config()?;
            println!("configuration looks structurally valid");
            println!(
                "metadata path: {}",
                redact::redact_path(&config.metadata_path().display().to_string())
            );
            println!(
                "data dir: {}",
                redact::redact_path(&config.data_dir().display().to_string())
            );
            Ok(())
        }
    }
}

fn run_index(command: IndexCommand) -> Result<(), String> {
    let config = load_config()?;
    let store = open_metadata_store(&config)?;
    match command {
        IndexCommand::Rebuild => {
            let report = store.rebuild_index().map_err(|error| error.to_string())?;
            println!("rebuild complete");
            println!("committed rows processed: {}", report.committed_rows);
            println!("active rows written: {}", report.active_rows);
            println!("staged rows found: {}", report.staged_rows);
            println!("recovery markers written: {}", report.recovery_markers);
            Ok(())
        }
        IndexCommand::Verify => {
            let report = store.verify_index().map_err(|error| error.to_string())?;
            println!("verify complete");
            println!("expected active rows: {}", report.expected_active_rows);
            println!("actual active rows: {}", report.actual_active_rows);
            println!("mismatched rows: {}", report.mismatched_rows);
            println!("staged rows found: {}", report.staged_rows);
            if report.mismatched_rows > 0 {
                Err("metadata index verification failed".to_string())
            } else {
                Ok(())
            }
        }
    }
}

fn run_db(command: DbCommand) -> Result<(), String> {
    let config = load_config()?;
    let store = open_metadata_store(&config)?;
    match command {
        DbCommand::Migrate => {
            let version = store.migrate().map_err(|error| error.to_string())?;
            println!("database migrated to schema version {version}");
            Ok(())
        }
        DbCommand::Status => {
            let status = store.status().map_err(|error| error.to_string())?;
            println!(
                "metadata path: {}",
                redact::redact_path(
                    &status
                        .path
                        .as_ref()
                        .map_or_else(|| "<memory>".to_string(), |path| path.display().to_string(),)
                )
            );
            println!("schema version: {}", status.schema_version);
            println!("committed objects: {}", status.committed_objects);
            println!("active objects: {}", status.active_objects);
            println!("staged objects: {}", status.staged_objects);
            println!("recovery markers: {}", status.recovery_markers);
            Ok(())
        }
    }
}

fn run_upstream(command: UpstreamCommand) -> Result<(), String> {
    match command {
        UpstreamCommand::Status => {
            println!("RustFS upstream: 47a3f5ef0110ee5af04bbb761a8bb5ed99a9ce15");
            println!("Telegram Drive upstream: 77518a93fbc8a8242f38e23e486a2d87d3f82fb2");
            Ok(())
        }
    }
}

fn run_users(command: UsersCommand) -> Result<(), String> {
    let config = AppConfig::from_env();
    let store = MetadataStore::open(config.metadata_path()).map_err(|error| error.to_string())?;
    match command {
        UsersCommand::Create {
            username,
            password,
            role,
            display_name,
        } => {
            let password = resolve_password(password)?;
            let display = display_name.unwrap_or_default();
            let user =
                telegram_s3::auth::create_account(&store, &username, &password, &role, &display)
                    .map_err(|error| error.to_string())?;
            println!(
                "created account '{}' (role {}, id {})",
                user.username, user.role, user.id
            );
            Ok(())
        }
        UsersCommand::List => {
            let users = store.list_users().map_err(|error| error.to_string())?;
            if users.is_empty() {
                println!("no operator accounts configured");
                println!("hint: telegram-s3 users create <username> --password <pw>");
            } else {
                for user in &users {
                    println!(
                        "{}\t{}\t{}\t{}",
                        user.username,
                        user.role,
                        if user.disabled { "disabled" } else { "enabled" },
                        user.display_name
                    );
                }
            }
            Ok(())
        }
        UsersCommand::Password { username, password } => {
            let password = resolve_password(password)?;
            let normalized = telegram_s3::auth::normalize_username(&username)
                .map_err(|error| error.to_string())?;
            let user = store
                .get_user(&normalized)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("no such account: {username}"))?;
            telegram_s3::auth::change_password(&store, &user.id, &password)
                .map_err(|error| error.to_string())?;
            println!(
                "password updated for '{}' (all sessions revoked)",
                user.username
            );
            Ok(())
        }
        UsersCommand::Delete { username } => {
            let normalized = telegram_s3::auth::normalize_username(&username)
                .map_err(|error| error.to_string())?;
            let user = store
                .get_user(&normalized)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("no such account: {username}"))?;
            if telegram_s3::auth::is_superadmin(&user) && store.user_count().unwrap_or(0) <= 1 {
                return Err("refusing to delete the last superadmin account".to_string());
            }
            store
                .delete_user(&user.id)
                .map_err(|error| error.to_string())?;
            store.revoke_user_sessions(&user.id).ok();
            println!("deleted account '{}'", user.username);
            Ok(())
        }
        UsersCommand::Status { username } => {
            let normalized = telegram_s3::auth::normalize_username(&username)
                .map_err(|error| error.to_string())?;
            let user = store
                .get_user(&normalized)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("no such account: {username}"))?;
            println!("username:      {}", user.username);
            println!("role:          {}", user.role);
            println!("display name:  {}", user.display_name);
            println!(
                "state:         {}",
                if user.disabled { "disabled" } else { "enabled" }
            );
            println!("token version: {}", user.token_version);
            println!("created (unix): {}", user.created_at.unix_timestamp());
            Ok(())
        }
    }
}

fn resolve_password(explicit: Option<String>) -> Result<String, String> {
    if let Some(value) = explicit {
        if value.is_empty() {
            return Err("password must not be empty (use an env var for scripts)".to_string());
        }
        return Ok(value);
    }
    match std::env::var("TG_ADMIN_PASSWORD") {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(
            "no password provided; pass --password or set TG_ADMIN_PASSWORD (for scripts and first boot)".to_string(),
        ),
    }
}

fn run_repair(dry_run: bool) -> Result<(), String> {
    let config = load_config()?;
    let object_format = open_object_format(&config)?;
    if dry_run {
        let status = object_format.status().map_err(render_object_format_error)?;
        println!("repair dry-run: no files changed");
        println!("staged objects: {}", status.staged_objects);
        println!(
            "recovery-required objects: {}",
            status.recovery_required_objects
        );
        println!("orphaned chunks: {}", status.orphaned_chunks);
        println!("committed objects: {}", status.committed_objects);
        return Ok(());
    }

    let report = object_format
        .reconcile()
        .map_err(render_object_format_error)?;
    println!("repair complete");
    println!("staged objects: {}", report.staged_objects);
    println!("committed objects: {}", report.committed_objects);
    println!(
        "recovery-required objects: {}",
        report.recovery_required_objects
    );
    println!("orphaned chunks: {}", report.orphaned_chunks);
    println!("repaired objects: {}", report.repaired_objects);
    println!("quarantined objects: {}", report.quarantined_objects);
    let status = object_format.status().map_err(render_object_format_error)?;
    if status.staged_objects > 0 || status.recovery_required_objects > 0 {
        return Err("repair completed but recovery state remains".to_string());
    }
    Ok(())
}

fn run_gc(dry_run: bool) -> Result<(), String> {
    let config = load_config()?;
    let object_format = open_object_format(&config)?;
    let report = object_format
        .garbage_collect(
            dry_run,
            time::Duration::seconds(GARBAGE_COLLECTION_RETENTION_SECONDS),
        )
        .map_err(render_object_format_error)?;
    if dry_run {
        println!("gc dry-run: no files changed");
    } else {
        println!("gc complete");
    }
    println!("eligible objects: {}", report.eligible_objects);
    println!("manifests removed: {}", report.manifests_removed);
    println!(
        "chunk directories removed: {}",
        report.chunk_directories_removed
    );
    println!(
        "quarantine entries removed: {}",
        report.quarantine_entries_removed
    );
    println!("bytes removed: {}", report.bytes_removed);
    Ok(())
}

fn load_config() -> Result<AppConfig, String> {
    let config = AppConfig::from_env();
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

fn open_metadata_store(config: &AppConfig) -> Result<MetadataStore, String> {
    MetadataStore::open(config.metadata_path()).map_err(|error| error.to_string())
}

async fn run_server() -> Result<(), String> {
    let config = load_config()?;
    let server = S3Server::bootstrap(&config)
        .await
        .map_err(|error| error.to_string())?;
    server.serve().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn open_transport() -> Result<TelegramTransport, String> {
    let config = load_config()?;
    TelegramTransport::open(config)
        .await
        .map_err(render_transport_error)
}

fn render_transport_error(error: TelegramTransportError) -> String {
    error.to_string()
}

fn open_object_format(config: &AppConfig) -> Result<ObjectFormatService, String> {
    ObjectFormatService::open(config).map_err(render_object_format_error)
}

fn render_object_format_error(error: ObjectFormatError) -> String {
    error.to_string()
}

fn print_transport_status(label: &str, status: &TelegramTransportStatus) -> Result<(), String> {
    println!(
        "{label}: session path {}",
        redact::redact_path(&status.session_path.display().to_string())
    );
    println!("{label}: proxy kind {:?}", status.proxy_kind);
    if let Some(proxy_url) = &status.proxy_url {
        println!("{label}: proxy url {}", redact_proxy_url(proxy_url));
    }
    println!("{label}: session state {:?}", status.session_state);
    Ok(())
}

fn print_object_format_status(label: &str, status: &ObjectFormatStatus) -> Result<(), String> {
    println!(
        "{label}: data dir {}",
        redact::redact_path(&status.data_dir.display().to_string())
    );
    println!("{label}: chunk size {}", status.chunk_size);
    println!("{label}: committed objects {}", status.committed_objects);
    println!("{label}: staged objects {}", status.staged_objects);
    println!(
        "{label}: recovery-required objects {}",
        status.recovery_required_objects
    );
    println!("{label}: orphaned chunks {}", status.orphaned_chunks);
    Ok(())
}

fn print_transport_state(
    label: &str,
    state: &telegram_s3::telegram::AuthState,
) -> Result<(), String> {
    println!("{label}: session {:?}", state.session);
    println!("{label}: flow {:?}", state.flow);
    Ok(())
}

fn redact_proxy_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("<unknown>");
            let port = parsed
                .port()
                .map_or(String::new(), |port| format!(":{port}"));
            format!("{}://{}{}", parsed.scheme(), host, port)
        }
        Err(_) => "<redacted-proxy>".to_string(),
    }
}
