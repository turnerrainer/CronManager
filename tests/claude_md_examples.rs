//! Guardrail: every YAML config block advertised in CLAUDE.md as
//! a best-practice baseline must round-trip through `AppConfig`
//! without error. If a future edit to CLAUDE.md drifts from the
//! wire schema, this test fails and names the offending block.

use cronmanager::config::AppConfig;

const LOOPBACK: &str = r#"
port: 8080
dsl_path: /app/DSL/samples

# Server-to-server posture — no browser CORS lane.
allowed_origins: []

limits:
  max_request_bytes: 1048576
  max_response_bytes: 16777216
  request_timeout_secs: 30
  shell_timeout_secs: 300
"#;

const PUBLIC: &str = r#"
port: 8080
dsl_path: /app/DSL
app_root_path: /app

allowed_origins:
  - "https://ops.example.com"

shell_environment:
  BACKUP_DIR: /var/lib/cronmanager/backups
  RETENTION_DAYS: "7"

limits:
  max_request_bytes: 1048576
  max_response_bytes: 16777216
  request_timeout_secs: 30
  shell_timeout_secs: 300

admin:
  bearer_token_env: CRONMANAGER_ADMIN_TOKEN
  trust_network: false

security:
  block_private_networks: true
  allow_dangerous_env_overrides: false
  max_query_params: 64
  max_dsl_file_bytes: 1048576
  dsl_load_timeout_secs: 15
  max_retry_count: 10
  min_cron_interval_secs: 10
  stored_response_body_max_bytes: 65536
  reload_min_interval_secs: 60

database:
  url: postgres://cronmanager@timescaledb:5432/cronmanager
  password_env: CRONMANAGER_DB_PASSWORD
"#;

const JVM_COMPAT: &str = r#"
configPath: /app/DSL
appRootPath: /app
allowedOrigins: "https://ops.example.com,https://ops.example.net"
shellEnvironment:
  BACKUP_DIR: /var/lib/cronmanager/backups
"#;

#[test]
fn claude_md_loopback_baseline_parses() {
    let cfg: AppConfig = serde_yaml_ng::from_str(LOOPBACK)
        .expect("CLAUDE.md loopback baseline must parse as AppConfig");
    assert_eq!(cfg.port, 8080);
    assert!(cfg.allowed_origins.is_empty());
    assert_eq!(cfg.limits.request_timeout_secs, 30);
}

#[test]
fn claude_md_public_baseline_parses() {
    let cfg: AppConfig =
        serde_yaml_ng::from_str(PUBLIC).expect("CLAUDE.md public baseline must parse as AppConfig");
    assert_eq!(cfg.allowed_origins, vec!["https://ops.example.com"]);
    assert_eq!(cfg.admin.bearer_token_env, "CRONMANAGER_ADMIN_TOKEN");
    assert!(!cfg.admin.trust_network);
    assert!(cfg.security.block_private_networks);
    assert_eq!(cfg.security.max_query_params, 64);
    assert_eq!(cfg.security.reload_min_interval_secs, 60);
    assert!(cfg.database.is_some());
}

#[test]
fn claude_md_jvm_compat_baseline_parses() {
    let cfg: AppConfig = serde_yaml_ng::from_str(JVM_COMPAT)
        .expect("CLAUDE.md JVM-compat baseline must parse as AppConfig");
    assert_eq!(
        cfg.dsl_path,
        std::path::PathBuf::from("/app/DSL"),
        "configPath alias must bind to dsl_path"
    );
    assert_eq!(
        cfg.allowed_origins,
        vec!["https://ops.example.com", "https://ops.example.net"],
        "JVM comma-separated allowedOrigins must split as documented"
    );
}
