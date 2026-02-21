use crate::config::AppConfig;
use crate::web;
use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use rcgen::generate_simple_self_signed;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tower_cookies::Key;
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Clone)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub tx: mpsc::UnboundedSender<axum::extract::ws::Message>,
}

pub struct RateLimiter {
    attempts: Vec<Instant>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            attempts: Vec::new(),
        }
    }

    /// Returns true if the request is allowed, false if rate-limited.
    pub fn check_and_record(&mut self) -> bool {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);
        self.attempts.retain(|t| now.duration_since(*t) < window);

        if self.attempts.len() >= 20 {
            return false;
        }

        self.attempts.push(now);
        true
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub rooms: Arc<RwLock<HashMap<String, Vec<User>>>>,
    pub cookie_key: Arc<RwLock<Key>>,
    pub rate_limiter: Arc<RwLock<RateLimiter>>,
}

pub async fn start(initial_config: AppConfig, config_path: PathBuf) -> Result<()> {
    let cookie_key = Key::from(initial_config.security.session_secret.as_bytes());

    let state = AppState {
        config: Arc::new(RwLock::new(initial_config.clone())),
        rooms: Arc::new(RwLock::new(HashMap::new())),
        cookie_key: Arc::new(RwLock::new(cookie_key)),
        rate_limiter: Arc::new(RwLock::new(RateLimiter::new())),
    };

    let state_for_watcher = state.clone();
    let path_for_watcher = config_path.clone();
    tokio::spawn(async move {
        if let Err(e) = watch_config(path_for_watcher, state_for_watcher).await {
            error!("Config watcher failed: {}", e);
        }
    });

    let tls_config = prepare_tls_config(&initial_config, &config_path).await?;
    let app = web::router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], initial_config.server.port));

    info!("=======================================================");
    info!("   BUECHSENTELEFON - STARTUP");
    info!("=======================================================");
    info!("Config File:   {}", config_path.display());
    info!(
        "Server Title:  {}",
        initial_config
            .server
            .title
            .as_deref()
            .unwrap_or("Buechsentelefon")
    );
    info!("-------------------------------------------------------");
    info!(
        "Local Access:  https://localhost:{}",
        initial_config.server.port
    );
    if initial_config.server.host != "0.0.0.0" && initial_config.server.host != "localhost" {
        info!(
            "Network Access: https://{}:{}",
            initial_config.server.host, initial_config.server.port
        );
    }
    info!("-------------------------------------------------------");

    if initial_config.tls.cert_path.is_some() {
        info!("SSL Mode:      Custom Certificates (Production Mode)");
    } else {
        warn!("SSL Mode:      Self-Signed (Development Mode)");
        warn!("               Browser will show a security warning.");
    }
    info!("-------------------------------------------------------");
    if initial_config.has_users() {
        let user_count = initial_config.users.len();
        info!(
            "Auth Mode:     User Accounts ({} user{} configured)",
            user_count,
            if user_count == 1 { "" } else { "s" }
        );
        for user in &initial_config.users {
            info!("               - {} [{}]", user.username, user.role);
        }
    } else {
        warn!("Auth Mode:     Legacy (single server password)");
        warn!("               Add named users: buechsentelefon add-user <NAME> <PW> --role <ROLE>");
    }
    info!("=======================================================");

    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service())
        .await
        .context("Server failed to start")?;

    Ok(())
}

async fn prepare_tls_config(config: &AppConfig, config_path: &Path) -> Result<RustlsConfig> {
    if let (Some(cert_path), Some(key_path)) = (&config.tls.cert_path, &config.tls.key_path) {
        if Path::new(cert_path).exists() && Path::new(key_path).exists() {
            return RustlsConfig::from_pem_file(cert_path, key_path)
                .await
                .context("Failed to load TLS certificates");
        }
    }

    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    let cert_file = config_dir.join("self_signed_cert.pem");
    let key_file = config_dir.join("self_signed_key.pem");

    if cert_file.exists() && key_file.exists() {
        info!("Loading persisted self-signed certificate...");
        return RustlsConfig::from_pem_file(&cert_file, &key_file)
            .await
            .context("Failed to load persisted self-signed certificates");
    }

    info!("Generating new self-signed certificate...");
    let subject_alt_names = vec![
        "localhost".to_string(),
        config.server.host.clone(),
        config.server.domain.clone(),
    ];

    let certified_key = generate_simple_self_signed(subject_alt_names)?;
    let cert_pem = certified_key.cert.pem();
    let key_pem = certified_key.key_pair.serialize_pem();

    std::fs::write(&cert_file, &cert_pem)?;
    std::fs::write(&key_file, &key_pem)?;
    info!(
        "Self-signed certificate saved to: {}",
        config_dir.display()
    );

    RustlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes())
        .await
        .context("Failed to create self-signed TLS config")
}

async fn watch_config(path: PathBuf, state: AppState) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.blocking_send(res);
        },
        Config::default(),
    )?;

    watcher.watch(&path, RecursiveMode::NonRecursive)?;

    while let Some(res) = rx.recv().await {
        match res {
            Ok(_) => {
                info!("Config file change detected. Reloading...");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                match AppConfig::load_or_create(&path) {
                    Ok((new_config, _)) => {
                        let mut w = state.config.write().await;

                        if w.server.port != new_config.server.port {
                            warn!("Port changed in config. Restart server to apply!");
                        }

                        if w.security.session_secret != new_config.security.session_secret {
                            let new_key =
                                Key::from(new_config.security.session_secret.as_bytes());
                            *state.cookie_key.write().await = new_key;
                            warn!("Session secret changed. Existing sessions invalidated.");
                        }

                        *w = new_config;
                        info!("Configuration reloaded successfully.");
                    }
                    Err(e) => error!("Failed to reload config: {}", e),
                }
            }
            Err(e) => error!("Watch error: {}", e),
        }
    }
    Ok(())
}
