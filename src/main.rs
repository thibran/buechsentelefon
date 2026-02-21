mod args;
mod config;
mod server;
mod web;

use args::{Args, Commands, RoleArg};
use clap::Parser;
use config::{add_or_update_user, update_password, update_room_password, AppConfig, UserRole};
use std::path::PathBuf;
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("buechsentelefon=info")
        .init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let args = Args::parse();
    let config_path = resolve_config_path(args.config.as_deref());

    match args.command {
        None => {
            info!("Starting buechsentelefon...");

            let (config, was_created) = match AppConfig::load_or_create(&config_path) {
                Ok(result) => result,
                Err(e) => {
                    error!("Failed to load configuration: {}", e);
                    std::process::exit(1);
                }
            };

            if was_created {
                std::process::exit(0);
            }

            if let Err(e) = server::start(config, config_path).await {
                error!("Server crashed: {}", e);
            }
        }
        Some(Commands::Config) => {
            println!("Configuration file path: {}", config_path.display());
            if config_path.exists() {
                match std::fs::read_to_string(&config_path) {
                    Ok(content) => {
                        println!("\n--- Current Configuration ---\n");
                        println!("{}", content);
                    }
                    Err(e) => error!("Could not read config file: {}", e),
                }
            } else {
                println!("(File does not exist yet - will be created on first start)");
            }
        }
        Some(Commands::SetPassword { password, room }) => {
            if !config_path.exists() {
                let _ = AppConfig::load_or_create(&config_path)?;
            }

            if let Some(room_name) = room {
                println!(
                    "Updating password for room '{}' in: {}",
                    room_name,
                    config_path.display()
                );
                match update_room_password(&config_path, &room_name, &password) {
                    Ok(_) => {
                        if password.is_empty() {
                            println!("Success: Room password removed!");
                        } else {
                            println!("Success: Room password updated!");
                        }
                    }
                    Err(e) => error!("Failed to update room password: {}", e),
                }
            } else {
                println!("Updating server password in: {}", config_path.display());
                match update_password(&config_path, &password) {
                    Ok(_) => println!("Success: Password updated!"),
                    Err(e) => error!("Failed to update password: {}", e),
                }
            }
        }

        Some(Commands::AddUser {
            username,
            password,
            role,
        }) => {
            if !config_path.exists() {
                let _ = AppConfig::load_or_create(&config_path)?;
            }

            let user_role = match role {
                RoleArg::Admin => UserRole::Admin,
                RoleArg::Standard => UserRole::Standard,
                RoleArg::Guest => UserRole::Guest,
            };

            println!(
                "Adding/updating user '{}' ({}) in: {}",
                username,
                user_role,
                config_path.display()
            );
            match add_or_update_user(&config_path, &username, &password, &user_role) {
                Ok(_) => println!("Success: User '{}' saved with role '{}'!", username, user_role),
                Err(e) => error!("Failed to save user: {}", e),
            }
        }
    }

    Ok(())
}

fn resolve_config_path(cli_path: Option<&str>) -> PathBuf {
    if let Some(path) = cli_path {
        return PathBuf::from(path);
    }

    if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "buechsentelefon") {
        let config_dir = proj_dirs.config_dir();
        if !config_dir.exists() {
            let _ = std::fs::create_dir_all(config_dir);
        }
        return config_dir.join("config.toml");
    }

    PathBuf::from("config.toml")
}
