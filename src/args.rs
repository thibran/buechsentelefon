use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand};
use std::sync::LazyLock;

fn get_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Green.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
}

static VERSION_STR: LazyLock<String> = LazyLock::new(|| {
    let version = env!("CARGO_PKG_VERSION");
    let sha = option_env!("VERGEN_GIT_SHA").unwrap_or("");
    if !sha.is_empty() && sha != "VERGEN_IDEMPOTENT_OUTPUT" {
        let short_sha = if sha.len() > 7 { &sha[..7] } else { sha };
        format!("{} ({})", version, short_sha)
    } else {
        version.to_string()
    }
});

#[derive(Parser, Debug)]
#[command(
    name = "buechsentelefon",
    author = env!("CARGO_PKG_AUTHORS"),
    version = VERSION_STR.as_str(),
    about = "A simple, secure WebRTC audio chat server.",
    styles = get_styles(),
    help_template = "{bin} {version}\n{author}\n\n{about}\n\n{usage-heading} {usage}\n\n{all-args}"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to the configuration file.
    /// If not provided, the default OS configuration directory is used.
    #[arg(short, long, global = true)]
    pub config: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Output the current configuration location and values
    Config,

    /// Update a password in the configuration file
    SetPassword {
        /// The new password (will be hashed and stored). Use "" to remove a room password.
        #[arg(value_name = "PASSWORD")]
        password: String,

        /// Set password for a specific room instead of the server password
        #[arg(long, value_name = "ROOM")]
        room: Option<String>,
    },
}
