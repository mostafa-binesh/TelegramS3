use std::fs;
use std::process::Command;

#[test]
fn docker_packaging_assets_match_phase7_expectations() {
    let dockerfile = fs::read_to_string("Dockerfile").expect("Dockerfile");
    assert!(dockerfile.contains("FROM node:22-bookworm-slim AS frontend"));
    assert!(dockerfile.contains("FROM rust:1.98-bookworm AS build"));
    assert!(dockerfile.contains("COPY --from=frontend /work/frontend/dist /work/frontend/dist"));
    assert!(dockerfile.contains("ENV TELEGRAM_ADMIN_UI_DIST_DIR=/var/lib/telegram-s3/ui"));
    assert!(dockerfile.contains(
        "ENTRYPOINT [\"/usr/bin/tini\", \"--\", \"/usr/local/bin/telegram-s3-entrypoint\"]"
    ));
    assert!(dockerfile.contains("HEALTHCHECK"));

    let compose = fs::read_to_string("docker-compose.yml").expect("compose");
    assert!(compose.contains("telegram-s3:"));
    assert!(compose.contains("TELEGRAM_S3_BIND_ADDR: 0.0.0.0:9000"));
    assert!(compose.contains("TELEGRAM_ADMIN_BIND_ADDR: 127.0.0.1:9001"));
    assert!(compose.contains("TELEGRAM_ADMIN_BOOTSTRAP_SECRET"));
    assert!(compose.contains("telegram-s3-metadata"));
    assert!(compose.contains("telegram-s3-session"));
    assert!(!compose.contains("setup:"));
}

#[test]
#[ignore = "requires docker"]
fn docker_compose_config_smoke_test() {
    let status = Command::new("docker")
        .args(["compose", "-f", "docker-compose.yml", "config"])
        .status()
        .expect("docker compose config");
    assert!(status.success());
}
