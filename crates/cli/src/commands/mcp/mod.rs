pub mod config;
pub mod diff;
pub mod hosts;
pub mod merge;
pub mod openclaw;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::Value;

use crate::commands::wallet::load_wallet;

use self::config::{build_claude_entry, build_cursor_entry, build_openclaw_entry, InstallConfig};
use self::hosts::{config_path, Host, Scope};
use self::merge::{merge_entry, read_entry, remove_entry};
use self::openclaw::{openclaw_set, openclaw_unset};

/// Signing mode for the MCP server.
#[derive(Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SigningMode {
    Auto,
    Escrow,
    Direct,
    Off,
}

impl SigningMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Escrow => "escrow",
            Self::Direct => "direct",
            Self::Off => "off",
        }
    }
}

/// Top-level `mcp` subcommand.
#[derive(clap::Args)]
pub struct McpArgs {
    #[command(subcommand)]
    pub action: McpAction,
}

#[derive(Subcommand)]
pub enum McpAction {
    /// Install Solvela MCP server into a host's config.
    Install(InstallArgs),
    /// Uninstall Solvela MCP server from a host's config.
    Uninstall(UninstallArgs),
}

/// Validate a gateway URL: must parse as http or https.
fn validate_gateway_url(s: &str) -> Result<String, String> {
    let parsed = url::Url::parse(s).map_err(|e| format!("invalid URL '{}': {}", s, e))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "gateway URL must use http or https scheme, got '{}'",
            parsed.scheme()
        ));
    }
    Ok(s.to_owned())
}

/// Validate a Solana wallet pubkey: base58 chars only, length 32–44.
fn validate_wallet(s: &str) -> Result<String, String> {
    const BASE58_ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if s.len() < 32 || s.len() > 44 {
        return Err(format!(
            "wallet address must be 32–44 characters, got {}",
            s.len()
        ));
    }
    if !s.bytes().all(|b| BASE58_ALPHABET.contains(&b)) {
        return Err(format!(
            "'{}' contains characters not valid in a base58 Solana pubkey",
            s
        ));
    }
    Ok(s.to_owned())
}

/// Validate budget: must be a finite positive number.
fn validate_budget(s: &str) -> Result<String, String> {
    let v: f64 = s
        .parse()
        .map_err(|e: std::num::ParseFloatError| e.to_string())?;
    if !v.is_finite() {
        return Err(format!("budget must be a finite number, got '{}'", s));
    }
    if v <= 0.0 {
        return Err(format!("budget must be > 0, got {}", v));
    }
    Ok(s.to_owned())
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Target host: claude-code | cursor | claude-desktop | openclaw
    #[arg(long)]
    pub host: String,

    /// Config scope: user | project (only for claude-code and cursor; default: user)
    #[arg(long, default_value = "user")]
    pub scope: String,

    /// Solvela gateway URL written into SOLVELA_API_URL
    #[arg(long, default_value = "https://api.solvela.ai", value_parser = validate_gateway_url)]
    pub gateway_url: String,

    /// Solana wallet pubkey written into SOLANA_WALLET_ADDRESS.
    /// If omitted, reads from ~/.solvela/wallet.json.
    #[arg(long, value_parser = validate_wallet)]
    pub wallet: Option<String>,

    /// Write SOLVELA_SESSION_BUDGET=<usdc> into the env block.
    /// Must be a positive number (e.g. "2.50").
    #[arg(long, value_parser = validate_budget)]
    pub budget: Option<String>,

    /// Payment signing mode
    #[arg(long, value_enum, default_value_t = SigningMode::Auto)]
    pub signing_mode: SigningMode,

    /// Print what would be written without touching any files
    #[arg(long)]
    pub dry_run: bool,

    /// Show a diff of what would change vs the existing config
    #[arg(long)]
    pub diff: bool,

    /// Overwrite an existing Solvela entry without prompting
    #[arg(long)]
    pub force: bool,

    /// Also write SOLANA_WALLET_KEY into the env block (dev/CI only).
    /// WARNING: hot key on disk is a security risk — use envFile or shell env instead.
    #[arg(long)]
    pub include_key: bool,

    /// For Cursor: do not emit an envFile reference (default: envFile is included).
    /// When set, falls back to writing literal values in the env block.
    #[arg(long)]
    pub no_envfile: bool,
}

#[derive(clap::Args)]
pub struct UninstallArgs {
    /// Target host: claude-code | cursor | claude-desktop | openclaw
    #[arg(long)]
    pub host: String,

    /// Config scope: user | project (only for claude-code and cursor; default: user)
    #[arg(long, default_value = "user")]
    pub scope: String,

    /// Print what would be done without touching any files or running any commands
    #[arg(long)]
    pub dry_run: bool,
}

const CLAUDE_CLI_INSTALL_HINT: &str = "claude CLI not found on PATH.\n\
     Install Claude Code CLI from https://claude.ai/download\n\
     or use --scope=project to write a .mcp.json file instead.";

/// Return the user's home directory, checking HOME then USERPROFILE.
fn home_dir() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        return Ok(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return Ok(PathBuf::from(h));
    }
    Err(anyhow::anyhow!(
        "Cannot determine user config directory: neither HOME nor USERPROFILE is set. \
         Set HOME in your shell environment or use --scope=project."
    ))
}

/// Install into Claude Code user scope by shelling out to the `claude` CLI.
///
/// Invokes: `claude mcp add -s user -e KEY=VALUE ... solvela -- npx -y @solvela/mcp-server`
/// This is the canonical write path per Claude Code docs; we do NOT edit
/// `~/.claude.json` or `~/.claude/settings.json` directly for user scope.
fn claude_cli_install(cfg: &InstallConfig) -> Result<()> {
    use std::io::ErrorKind;

    let mut cmd = Command::new("claude");
    cmd.args(["mcp", "add", "--scope", "user", "--transport", "stdio"]);

    // Inject env vars via -e flags.
    cmd.arg("-e")
        .arg(format!("SOLVELA_API_URL={}", cfg.gateway_url));
    cmd.arg("-e")
        .arg(format!("SOLVELA_SIGNING_MODE={}", cfg.signing_mode));
    if let Some(ref addr) = cfg.wallet_address {
        cmd.arg("-e").arg(format!("SOLANA_WALLET_ADDRESS={}", addr));
    }
    cmd.arg("-e")
        .arg("SOLANA_RPC_URL=https://api.mainnet-beta.solana.com");
    if let Some(ref budget) = cfg.budget {
        cmd.arg("-e")
            .arg(format!("SOLVELA_SESSION_BUDGET={}", budget));
    }
    if cfg.include_key {
        cmd.arg("-e")
            .arg("SOLANA_WALLET_KEY=<paste-your-base58-private-key-here>");
    }

    // Server name and command.
    cmd.arg("solvela")
        .arg("--")
        .args(["npx", "-y", "@solvela/mcp-server"]);

    tracing::info!("running: claude mcp add --scope user solvela ...");

    let output = cmd.output().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow::anyhow!("{}", CLAUDE_CLI_INSTALL_HINT)
        } else {
            anyhow::anyhow!("failed to run claude CLI: {}", e)
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "claude mcp add failed (exit {}):\n{}",
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

/// Uninstall from Claude Code user scope by shelling out to the `claude` CLI.
///
/// Invokes: `claude mcp remove solvela --scope user`
fn claude_cli_uninstall(dry_run: bool) -> Result<()> {
    use std::io::ErrorKind;

    if dry_run {
        println!("# Dry run — would run: claude mcp remove solvela --scope user");
        return Ok(());
    }

    tracing::info!("running: claude mcp remove solvela --scope user");

    let output = Command::new("claude")
        .args(["mcp", "remove", "solvela", "--scope", "user"])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow::anyhow!("{}", CLAUDE_CLI_INSTALL_HINT)
            } else {
                anyhow::anyhow!("failed to run claude CLI: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_lower = stderr.to_lowercase();
        // Friendly no-op: claude reports the server doesn't exist.
        if stderr_lower.contains("no such") || stderr_lower.contains("not found") {
            tracing::info!(
                "no solvela mcp server registered in claude user scope; nothing to remove"
            );
            return Ok(());
        }
        bail!(
            "claude mcp remove failed (exit {}):\n{}",
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

/// Run the `mcp install` subcommand.
pub async fn run_install(args: InstallArgs) -> Result<()> {
    let host = Host::from_str(&args.host)
        .with_context(|| format!("invalid --host value '{}'", args.host))?;
    let scope = Scope::from_str(&args.scope)
        .with_context(|| format!("invalid --scope value '{}'", args.scope))?;

    // Validate scope compatibility.
    if scope != Scope::User && !host.supports_scope() {
        bail!(
            "--host={} does not support --scope; it uses a single fixed config path",
            args.host
        );
    }

    // Resolve wallet address: explicit arg > wallet.json > emit a note
    let wallet_address = if let Some(w) = args.wallet {
        Some(w)
    } else {
        match load_wallet() {
            Ok(wallet) => wallet["address"].as_str().map(str::to_owned),
            Err(_) => {
                eprintln!(
                    "Note: no wallet found at ~/.solvela/wallet.json. \
                     Run 'solvela wallet init' or pass --wallet=<pubkey>."
                );
                None
            }
        }
    };

    let mut cfg = InstallConfig::new(args.gateway_url);
    cfg.wallet_address = wallet_address;
    cfg.budget = args.budget;
    cfg.signing_mode = args.signing_mode.as_str().to_owned();
    cfg.include_key = args.include_key;
    // Cursor uses envFile by default; --no-envfile or --include-key disables it.
    cfg.cursor_use_envfile = !args.no_envfile && !args.include_key;

    if cfg.include_key {
        eprintln!(
            "WARNING: --include-key writes a hot key placeholder into the config file. \
             Anyone with access to that file can spend your USDC. \
             Prefer setting SOLANA_WALLET_KEY in your shell profile or a .env file \
             with mode 0600."
        );
    }

    // Build the entry value for this host.
    let entry = build_entry(&host, &cfg);

    match &host {
        Host::OpenClaw => {
            if args.dry_run {
                let json =
                    serde_json::to_string_pretty(&entry).context("failed to serialize entry")?;
                println!("# Dry run — would run:");
                println!("openclaw mcp set solvela '{json}'");
                return Ok(());
            }
            if args.diff {
                eprintln!(
                    "Note: --diff is not supported for --host=openclaw \
                     (config is managed by the openclaw CLI)."
                );
            }
            openclaw_set(&entry)?;
            tracing::info!("registered solvela mcp server with openclaw");
        }
        Host::ClaudeCode if scope == Scope::User => {
            // User scope: shell out to the claude CLI (canonical write path).
            if args.dry_run {
                let json =
                    serde_json::to_string_pretty(&entry).context("failed to serialize entry")?;
                println!("# Dry run — would run: claude mcp add --scope user solvela ...");
                println!("{json}");
                return Ok(());
            }
            if args.diff {
                eprintln!(
                    "Note: --diff is not supported for --host=claude-code --scope=user \
                     (config is managed by the claude CLI)."
                );
            }
            claude_cli_install(&cfg)?;
            tracing::info!("registered solvela mcp server via claude CLI (user scope)");
            println!(
                "Note: SOLANA_WALLET_KEY is NOT written to the config file. \
                 Set it in your shell environment or a .env file:\n  \
                 export SOLANA_WALLET_KEY=<base58-keypair>"
            );
        }
        _ => {
            let home = home_dir()?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let path = config_path(&host, &scope, &home, &cwd).ok_or_else(|| {
                anyhow::anyhow!("internal: host {} has no config path (bug)", host)
            })?;

            if args.diff {
                show_diff(&path, &entry)?;
                return Ok(());
            }

            if args.dry_run {
                // Build what the resulting full file would look like.
                let preview = build_dry_run_preview(&path, &entry)?;
                let json = serde_json::to_string_pretty(&preview)
                    .context("failed to serialize preview")?;
                println!("# Dry run — would write to: {}", path.display());
                println!("{json}");
                return Ok(());
            }

            match merge_entry(&path, &entry, args.force)? {
                true => {
                    tracing::info!(path = %path.display(), "installed mcp config");
                    println!("Solvela MCP server installed at {}", path.display());
                    println!(
                        "Note: SOLANA_WALLET_KEY is NOT written to the config file. \
                         Set it in your shell environment or a .env file:\n  \
                         export SOLANA_WALLET_KEY=<base58-keypair>"
                    );
                }
                false => {
                    println!("No change — Solvela MCP entry is already up to date.");
                }
            }
        }
    }

    Ok(())
}

/// Run the `mcp uninstall` subcommand.
pub async fn run_uninstall(args: UninstallArgs) -> Result<()> {
    let host = Host::from_str(&args.host)
        .with_context(|| format!("invalid --host value '{}'", args.host))?;
    let scope = Scope::from_str(&args.scope)
        .with_context(|| format!("invalid --scope value '{}'", args.scope))?;

    if scope != Scope::User && !host.supports_scope() {
        bail!("--host={} does not support --scope", args.host);
    }

    match (&host, &scope) {
        (Host::OpenClaw, _) => {
            openclaw_unset()?;
            tracing::info!("removed solvela mcp server from openclaw");
        }
        (Host::ClaudeCode, Scope::User) => {
            // Mirror the install shell-out — use `claude mcp remove solvela --scope user`.
            return claude_cli_uninstall(args.dry_run);
        }
        _ => {
            let home = home_dir()?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let path = config_path(&host, &scope, &home, &cwd).ok_or_else(|| {
                anyhow::anyhow!("internal: host {} has no config path (bug)", host)
            })?;

            match remove_entry(&path)? {
                true => {
                    tracing::info!(path = %path.display(), "removed mcp config");
                    println!("Solvela MCP entry removed from {}.", path.display());
                }
                false => {
                    eprintln!(
                        "No Solvela entry found at {}; nothing to do.",
                        path.display()
                    );
                }
            }
        }
    }

    Ok(())
}

/// Build the appropriate entry value based on the host type.
fn build_entry(host: &Host, cfg: &InstallConfig) -> Value {
    match host {
        Host::ClaudeCode | Host::ClaudeDesktop => build_claude_entry(cfg),
        Host::Cursor => build_cursor_entry(cfg),
        Host::OpenClaw => build_openclaw_entry(cfg),
    }
}

/// Build the full file preview for --dry-run: merges the new entry into the existing
/// file content (or starts fresh) and returns the resulting JSON value.
fn build_dry_run_preview(path: &std::path::Path, entry: &Value) -> Result<Value> {
    let mut root: Value = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("invalid JSON in {}", path.display()))?
    } else {
        Value::Object(serde_json::Map::new())
    };

    if root.get("mcpServers").is_none() {
        root.as_object_mut()
            .context("config root is not a JSON object")?
            .insert(
                "mcpServers".to_owned(),
                Value::Object(serde_json::Map::new()),
            );
    }

    root["mcpServers"]
        .as_object_mut()
        .context("mcpServers is not a JSON object")?
        .insert("solvela".to_owned(), entry.clone());

    Ok(root)
}

/// Show a diff between the existing Solvela entry and the new entry.
fn show_diff(path: &std::path::Path, new_entry: &Value) -> Result<()> {
    let new_json = serde_json::to_string_pretty(new_entry).context("serialize new entry")?;

    match read_entry(path)? {
        None => {
            println!(
                "# No existing Solvela entry at {} — full new config:",
                path.display()
            );
            println!("{new_json}");
        }
        Some(existing) => {
            let old_json =
                serde_json::to_string_pretty(&existing).context("serialize existing entry")?;
            if old_json == new_json {
                println!("# No changes — existing entry matches.");
            } else {
                println!("# Diff for mcpServers.solvela in {}:", path.display());
                print!("{}", diff::line_diff(&old_json, &new_json));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn default_install_args(host: &str) -> InstallArgs {
        InstallArgs {
            host: host.to_owned(),
            scope: "user".to_owned(),
            gateway_url: "https://api.solvela.ai".to_owned(),
            wallet: None,
            budget: None,
            signing_mode: SigningMode::Auto,
            dry_run: false,
            diff: false,
            force: false,
            include_key: false,
            no_envfile: false,
        }
    }

    /// --dry-run must never write any file; the config path must not exist after the call.
    #[tokio::test]
    async fn test_dry_run_does_not_write_file() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());

        // project scope writes .mcp.json, which we can assert on without needing claude CLI.
        let args = InstallArgs {
            dry_run: true,
            scope: "project".to_owned(),
            ..default_install_args("claude-code")
        };
        run_install(args).await.unwrap();

        // The project-scope config must NOT have been created.
        let config_file = tmp.path().join(".mcp.json");
        assert!(
            !config_file.exists(),
            "--dry-run must not write config file, but found: {}",
            config_file.display()
        );
    }

    /// --dry-run for cursor must also not write any file.
    #[tokio::test]
    async fn test_dry_run_cursor_does_not_write_file() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());

        let args = InstallArgs {
            dry_run: true,
            ..default_install_args("cursor")
        };
        run_install(args).await.unwrap();

        let config_file = tmp.path().join(".cursor").join("mcp.json");
        assert!(
            !config_file.exists(),
            "--dry-run must not write config file, but found: {}",
            config_file.display()
        );
    }

    /// home_dir() returns Ok when HOME is set.
    #[tokio::test]
    async fn test_home_dir_ok() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());
        let result = home_dir();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), tmp.path());
    }

    /// home_dir() returns Err when neither HOME nor USERPROFILE is set.
    #[test]
    fn test_home_dir_err_when_unset() {
        // Cannot use ENV_MUTEX here (sync test) — this is a unit test of the logic only.
        // We remove HOME and USERPROFILE temporarily.
        let old_home = std::env::var("HOME").ok();
        let old_up = std::env::var("USERPROFILE").ok();
        std::env::remove_var("HOME");
        std::env::remove_var("USERPROFILE");

        let result = home_dir();

        // Restore
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
        if let Some(u) = old_up {
            std::env::set_var("USERPROFILE", u);
        }

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("HOME") || msg.contains("USERPROFILE"),
            "msg={msg}"
        );
    }

    /// validate_gateway_url rejects non-http/https schemes.
    #[test]
    fn test_validate_gateway_url_rejects_bad_scheme() {
        assert!(validate_gateway_url("ftp://example.com").is_err());
        assert!(validate_gateway_url("not-a-url").is_err());
        assert!(validate_gateway_url("https://api.solvela.ai").is_ok());
        assert!(validate_gateway_url("http://localhost:8402").is_ok());
    }

    /// validate_wallet rejects invalid pubkeys.
    #[test]
    fn test_validate_wallet() {
        // Too short
        assert!(validate_wallet("abc").is_err());
        // Invalid chars (0OIl)
        assert!(validate_wallet("0OIlaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
        // Valid-ish base58 length
        assert!(validate_wallet("11111111111111111111111111111112").is_ok());
    }

    /// validate_budget rejects non-positive values.
    #[test]
    fn test_validate_budget() {
        assert!(validate_budget("0").is_err());
        assert!(validate_budget("-1.5").is_err());
        assert!(validate_budget("not-a-number").is_err());
        assert!(validate_budget("2.50").is_ok());
        assert!(validate_budget("0.01").is_ok());
    }

    /// validate_budget rejects non-finite values (NaN, Infinity).
    #[test]
    fn test_validate_budget_rejects_non_finite() {
        let nan_result = validate_budget("NaN");
        assert!(nan_result.is_err(), "NaN should be rejected");
        assert!(
            nan_result.unwrap_err().contains("finite"),
            "error should mention 'finite'"
        );

        let inf_result = validate_budget("Infinity");
        assert!(inf_result.is_err(), "Infinity should be rejected");
        assert!(
            inf_result.unwrap_err().contains("finite"),
            "error should mention 'finite'"
        );

        let neg_inf_result = validate_budget("-Infinity");
        assert!(neg_inf_result.is_err(), "-Infinity should be rejected");
    }

    /// claude_cli_uninstall dry_run prints the command and returns Ok without exec.
    #[tokio::test]
    async fn test_claude_cli_uninstall_dry_run() {
        // dry_run path never shells out, so no PATH manipulation needed.
        let result = claude_cli_uninstall(true);
        assert!(result.is_ok(), "dry_run should return Ok: {:?}", result);
    }

    /// claude_cli_uninstall returns the CLAUDE_CLI_INSTALL_HINT error when claude not on PATH.
    #[tokio::test]
    async fn test_claude_cli_uninstall_not_on_path() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = TempDir::new().unwrap();
        let old = std::env::var("PATH").unwrap_or_default();
        // Point PATH at an empty dir so `claude` won't be found.
        std::env::set_var("PATH", tmp.path().to_str().unwrap());

        let result = claude_cli_uninstall(false);

        std::env::set_var("PATH", &old);
        drop(tmp);

        assert!(result.is_err(), "should error when claude not on PATH");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("claude CLI not found") || msg.contains("claude.ai/download"),
            "expected install hint, got: {msg}"
        );
    }

    /// run_uninstall with claude-code + user scope shells out (no config path error).
    /// Uses a fake `claude` that exits 0, verifying the new arm is taken.
    #[tokio::test]
    async fn test_run_uninstall_claude_code_user_scope() {
        let _lock = crate::ENV_MUTEX.lock().await;

        // Build a fake `claude` script that exits 0.
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("claude");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{old}", tmp.path().to_str().unwrap()));

        let args = UninstallArgs {
            host: "claude-code".to_owned(),
            scope: "user".to_owned(),
            dry_run: false,
        };
        let result = run_uninstall(args).await;

        std::env::set_var("PATH", &old);
        drop(tmp);

        assert!(
            result.is_ok(),
            "claude-code + user uninstall should succeed via CLI, got: {:?}",
            result
        );
    }
}
