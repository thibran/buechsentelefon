use anyhow::{Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use toml_edit::{value, DocumentMut};
use tracing::info;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    Standard,
    Guest,
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
            UserRole::Standard => write!(f, "standard"),
            UserRole::Guest => write!(f, "guest"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserConfig {
    pub username: String,
    pub password_hash: String,
    pub role: UserRole,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub security: SecurityConfig,
    pub tls: TlsConfig,
    pub webrtc: WebRtcConfig,
    pub rooms: Vec<RoomConfig>,
    #[serde(default)]
    pub users: Vec<UserConfig>,
    #[serde(default)]
    pub branding: BrandingConfig,
    #[serde(default)]
    pub legal: LegalConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub domain: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    pub server_password_hash: String,
    pub session_secret: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsConfig {
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebRtcConfig {
    pub stun_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoomConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct BrandingConfig {
    pub favicon_path: Option<String>,
    pub logo_path: Option<String>,
    pub header_banner_path: Option<String>,
    pub background_image_path: Option<String>,
    pub custom_css_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LegalConfig {
    pub imprint_path: Option<String>,
    pub privacy_policy_path: Option<String>,
}

impl AppConfig {
    /// Load config from path, or create a default config file if none exists.
    /// Returns `(config, was_newly_created)`.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool)> {
        let created = if !path.exists() {
            info!(
                "Config file not found. Creating default config at: {}",
                path.display()
            );
            create_default_config(path)?;
            true
        } else {
            false
        };

        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: AppConfig = toml::from_str(&content)
            .with_context(|| "Failed to parse configuration file. Please check syntax.")?;

        Ok((config, created))
    }

    pub fn verify_password(&self, plain: &str) -> bool {
        verify_hash(&self.security.server_password_hash, plain)
    }

    pub fn find_room(&self, name: &str) -> Option<&RoomConfig> {
        self.rooms.iter().find(|r| r.name == name)
    }

    /// Returns true if named user accounts are configured.
    /// When false, the legacy server password is used for authentication.
    pub fn has_users(&self) -> bool {
        !self.users.is_empty()
    }

    pub fn find_user(&self, username: &str) -> Option<&UserConfig> {
        self.users.iter().find(|u| u.username == username)
    }

    /// Verifies username/password and returns the matching user on success.
    pub fn authenticate_user<'a>(&'a self, username: &str, password: &str) -> Option<&'a UserConfig> {
        let user = self.find_user(username)?;
        if verify_hash(&user.password_hash, password) {
            Some(user)
        } else {
            None
        }
    }
}

pub fn verify_hash(hash: &str, plain: &str) -> bool {
    if hash.is_empty() {
        return false;
    }
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed_hash)
        .is_ok()
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    Ok(argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Password hashing failed: {}", e))?
        .to_string())
}

pub fn update_password(path: &Path, new_password: &str) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let mut doc = content.parse::<DocumentMut>()?;
    let password_hash = hash_password(new_password)?;

    doc["security"]["server_password_hash"] = value(password_hash);
    fs::write(path, doc.to_string())?;

    Ok(())
}

pub fn add_or_update_user(
    path: &Path,
    username: &str,
    password: &str,
    role: &UserRole,
) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let mut doc = content.parse::<DocumentMut>()?;
    let password_hash = hash_password(password)?;
    let role_str = role.to_string();

    if doc.get("users").is_some() {
        let users = doc["users"]
            .as_array_of_tables_mut()
            .context("'users' must be an array of tables ([[users]])")?;

        if let Some(user_table) = users
            .iter_mut()
            .find(|u| u.get("username").and_then(|n| n.as_str()) == Some(username))
        {
            user_table["password_hash"] = value(password_hash);
            user_table["role"] = value(role_str);
        } else {
            let mut new_user = toml_edit::Table::new();
            new_user["username"] = value(username);
            new_user["password_hash"] = value(password_hash);
            new_user["role"] = value(role_str);
            users.push(new_user);
        }
    } else {
        let mut aot = toml_edit::ArrayOfTables::new();
        let mut new_user = toml_edit::Table::new();
        new_user["username"] = value(username);
        new_user["password_hash"] = value(password_hash);
        new_user["role"] = value(role_str);
        aot.push(new_user);
        doc.insert("users", toml_edit::Item::ArrayOfTables(aot));
    }

    fs::write(path, doc.to_string())?;
    Ok(())
}

pub fn update_room_password(path: &Path, room_name: &str, new_password: &str) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let mut doc = content.parse::<DocumentMut>()?;

    let rooms = doc["rooms"]
        .as_array_of_tables_mut()
        .context("'rooms' must be an array of tables ([[rooms]])")?;

    let room = rooms
        .iter_mut()
        .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(room_name))
        .with_context(|| format!("Room '{}' not found in config", room_name))?;

    if new_password.is_empty() {
        room.remove("password_hash");
    } else {
        let hash = hash_password(new_password)?;
        room["password_hash"] = value(hash);
    }

    fs::write(path, doc.to_string())?;
    Ok(())
}

fn generate_secret() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect()
}

fn generate_random_password() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect()
}

fn create_default_config(path: &Path) -> Result<()> {
    let session_secret = generate_secret();
    let initial_password = generate_random_password();
    let password_hash = hash_password(&initial_password)?;

    let config_content = format!(
        r#"# buechsentelefon Configuration
# =============================

[server]
# The IP address to bind to. "0.0.0.0" allows access from the network.
host = "0.0.0.0"
port = 4433
domain = "localhost"

# Displayed in the header and login screen
title = "Buechsentelefon"

# Rooms: Each [[rooms]] entry defines a voice channel.
# Optional: banner_path (image), password_hash (set via CLI)
[[rooms]]
name = "Kaffeeküche"

[[rooms]]
name = "Bunker"

[[rooms]]
name = "Meeting Room"

[[rooms]]
name = "Gamer Ecke"

# Users: Named accounts with roles (admin, standard, guest).
# When users are configured, login requires username + password.
# Add users via CLI: buechsentelefon add-user <USERNAME> <PASSWORD> [--role <ROLE>]
# Roles:
#   admin    - Full access + server administration
#   standard - Can join all rooms (default)
#   guest    - Can only join the Lobby

# [[users]]
# username = "admin"
# password_hash = ""  # Set via: buechsentelefon add-user admin --role admin
# role = "admin"

[branding]
# favicon_path = "./favicon.ico"
# logo_path = "./logo.png"
# header_banner_path = "./header-banner.png"
# background_image_path = "./bg.jpg"
# custom_css_path = "./custom.css"

[legal]
# Path to an HTML or text file for the imprint page (required in DE/EU).
# imprint_path = "./impressum.html"
# Path to an HTML or text file for the privacy policy page.
# privacy_policy_path = "./datenschutz.html"

[security]
# The Argon2 hash of the server password (used when no [[users]] are configured).
server_password_hash = "{server_password_hash}"
session_secret = "{session_secret}"

[tls]
# HTTPS Configuration. Leave commented for auto-generated self-signed certs.
# cert_path = "/path/to/fullchain.pem"
# key_path = "/path/to/privkey.pem"

[webrtc]
stun_servers = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302"
]
"#,
        server_password_hash = password_hash,
        session_secret = session_secret
    );

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, config_content)?;

    info!("Generated default configuration.");
    println!("-------------------------------------------------------");
    println!("IMPORTANT: A new configuration was created.");
    println!("Config path: {}", path.display());
    println!();
    println!("Initial Admin Password: {}", initial_password);
    println!();
    println!("Set your own password, then restart:");
    println!("  buechsentelefon set-password <YOUR_PASSWORD>");
    println!("-------------------------------------------------------");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_and_load_default_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let (config, created) = AppConfig::load_or_create(&path).unwrap();

        assert!(created);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 4433);
        assert_eq!(config.server.domain, "localhost");
        assert_eq!(config.rooms.len(), 4);
        assert_eq!(config.rooms[0].name, "Kaffeeküche");
        assert_eq!(config.rooms[1].name, "Bunker");
        assert!(config.legal.imprint_path.is_none());
        assert!(config.legal.privacy_policy_path.is_none());
    }

    #[test]
    fn test_load_existing_config_not_created() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        let (_, created_first) = AppConfig::load_or_create(&path).unwrap();
        assert!(created_first);

        let (_, created_second) = AppConfig::load_or_create(&path).unwrap();
        assert!(!created_second);
    }

    #[test]
    fn test_password_hash_and_verify() {
        let hash = hash_password("test123").unwrap();
        assert!(verify_hash(&hash, "test123"));
        assert!(!verify_hash(&hash, "wrong"));
        assert!(!verify_hash(&hash, ""));
    }

    #[test]
    fn test_verify_empty_hash_returns_false() {
        assert!(!verify_hash("", "any"));
    }

    #[test]
    fn test_verify_invalid_hash_returns_false() {
        assert!(!verify_hash("not-a-real-hash", "any"));
    }

    #[test]
    fn test_config_verify_password() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (config, _) = AppConfig::load_or_create(&path).unwrap();

        assert!(!config.verify_password("wrong"));
        // Default random password works (we can't test it without capturing output)
    }

    #[test]
    fn test_find_room() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (config, _) = AppConfig::load_or_create(&path).unwrap();

        assert!(config.find_room("Kaffeeküche").is_some());
        assert!(config.find_room("Bunker").is_some());
        assert!(config.find_room("NonExistent").is_none());
        assert!(config.find_room("").is_none());
    }

    #[test]
    fn test_find_room_returns_correct_data() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (config, _) = AppConfig::load_or_create(&path).unwrap();

        let room = config.find_room("Bunker").unwrap();
        assert_eq!(room.name, "Bunker");
        assert!(room.banner_path.is_none());
        assert!(room.password_hash.is_none());
    }

    #[test]
    fn test_update_server_password() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        AppConfig::load_or_create(&path).unwrap();

        update_password(&path, "newpass").unwrap();

        let (config, _) = AppConfig::load_or_create(&path).unwrap();
        assert!(config.verify_password("newpass"));
        assert!(!config.verify_password("oldpass"));
    }

    #[test]
    fn test_set_room_password() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        AppConfig::load_or_create(&path).unwrap();

        update_room_password(&path, "Bunker", "roomsecret").unwrap();

        let (config, _) = AppConfig::load_or_create(&path).unwrap();
        let room = config.find_room("Bunker").unwrap();
        assert!(room.password_hash.is_some());
        assert!(verify_hash(
            room.password_hash.as_ref().unwrap(),
            "roomsecret"
        ));
        assert!(!verify_hash(room.password_hash.as_ref().unwrap(), "wrong"));
    }

    #[test]
    fn test_remove_room_password() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        AppConfig::load_or_create(&path).unwrap();

        update_room_password(&path, "Bunker", "secret").unwrap();
        let (config, _) = AppConfig::load_or_create(&path).unwrap();
        assert!(config.find_room("Bunker").unwrap().password_hash.is_some());

        update_room_password(&path, "Bunker", "").unwrap();
        let (config, _) = AppConfig::load_or_create(&path).unwrap();
        assert!(config.find_room("Bunker").unwrap().password_hash.is_none());
    }

    #[test]
    fn test_room_password_nonexistent_room_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        AppConfig::load_or_create(&path).unwrap();

        let result = update_room_password(&path, "NonExistent", "pw");
        assert!(result.is_err());
    }

    #[test]
    fn test_default_branding_is_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (config, _) = AppConfig::load_or_create(&path).unwrap();

        assert!(config.branding.favicon_path.is_none());
        assert!(config.branding.logo_path.is_none());
        assert!(config.branding.header_banner_path.is_none());
        assert!(config.branding.background_image_path.is_none());
        assert!(config.branding.custom_css_path.is_none());
    }

    #[test]
    fn test_rooms_have_no_passwords_by_default() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (config, _) = AppConfig::load_or_create(&path).unwrap();

        for room in &config.rooms {
            assert!(
                room.password_hash.is_none(),
                "Room {} should have no password",
                room.name
            );
        }
    }

    #[test]
    fn test_multiple_room_passwords_independent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        AppConfig::load_or_create(&path).unwrap();

        update_room_password(&path, "Bunker", "pw1").unwrap();
        update_room_password(&path, "Kaffeeküche", "pw2").unwrap();

        let (config, _) = AppConfig::load_or_create(&path).unwrap();
        let bunker = config.find_room("Bunker").unwrap();
        let kaffee = config.find_room("Kaffeeküche").unwrap();

        assert!(verify_hash(bunker.password_hash.as_ref().unwrap(), "pw1"));
        assert!(!verify_hash(bunker.password_hash.as_ref().unwrap(), "pw2"));
        assert!(verify_hash(kaffee.password_hash.as_ref().unwrap(), "pw2"));
        assert!(!verify_hash(kaffee.password_hash.as_ref().unwrap(), "pw1"));
    }

    #[test]
    fn test_config_survives_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        let (original, _) = AppConfig::load_or_create(&path).unwrap();

        let serialized = toml::to_string(&original).unwrap();
        let deserialized: AppConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(original.server.port, deserialized.server.port);
        assert_eq!(original.rooms.len(), deserialized.rooms.len());
        assert_eq!(
            original.webrtc.stun_servers,
            deserialized.webrtc.stun_servers
        );
    }
}
