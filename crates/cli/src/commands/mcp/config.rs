use serde_json::{Map, Value};

/// Parameters that shape the generated MCP server config entry.
#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub gateway_url: String,
    /// Solana wallet pubkey — written into `SOLANA_WALLET_ADDRESS`.
    pub wallet_address: Option<String>,
    /// `SOLVELA_SESSION_BUDGET` value (USDC). Only emitted if `Some`.
    pub budget: Option<String>,
    /// `SOLVELA_SIGNING_MODE` value. Default "auto".
    pub signing_mode: String,
    /// If true, also emit `SOLANA_WALLET_KEY` in the env block (dev/CI escape hatch).
    /// Requires explicit `--include-key` flag. Defaults to false.
    pub include_key: bool,
    /// For Cursor only: use `${env:VAR}` interpolation + `envFile` instead of literal values.
    /// Defaults to true. Disabled when `--no-envfile` is passed or when `--include-key` is set
    /// (so the explicit placeholder actually lands in the file).
    pub cursor_use_envfile: bool,
}

impl InstallConfig {
    pub fn new(gateway_url: String) -> Self {
        Self {
            gateway_url,
            wallet_address: None,
            budget: None,
            signing_mode: "auto".to_owned(),
            include_key: false,
            cursor_use_envfile: true,
        }
    }
}

/// Build the `mcpServers.solvela` entry value for Claude Code / Claude Desktop.
///
/// Uses literal env values. Does NOT include `SOLANA_WALLET_KEY` unless
/// `cfg.include_key` is true.
pub fn build_claude_entry(cfg: &InstallConfig) -> Value {
    let env = build_env_block(cfg, false, false);

    let mut obj = Map::new();
    obj.insert("command".to_owned(), Value::String("npx".to_owned()));
    obj.insert(
        "args".to_owned(),
        Value::Array(vec![
            Value::String("-y".to_owned()),
            Value::String("@solvela/mcp-server".to_owned()),
        ]),
    );
    obj.insert("env".to_owned(), Value::Object(env));
    Value::Object(obj)
}

/// Build the `mcpServers.solvela` entry value for Cursor.
///
/// Cursor requires an additional `"type": "stdio"` field.
///
/// When `cfg.cursor_use_envfile` is true (the default), the entry uses
/// `${env:VAR}` interpolation for `SOLANA_WALLET_ADDRESS` and `SOLANA_RPC_URL`
/// and includes `"envFile": "${userHome}/.solvela/env"` per plan §10.2.
/// When false (--no-envfile or --include-key), falls back to literal values.
pub fn build_cursor_entry(cfg: &InstallConfig) -> Value {
    let env = build_env_block(cfg, false, cfg.cursor_use_envfile);

    let mut obj = Map::new();
    obj.insert("type".to_owned(), Value::String("stdio".to_owned()));
    obj.insert("command".to_owned(), Value::String("npx".to_owned()));
    obj.insert(
        "args".to_owned(),
        Value::Array(vec![
            Value::String("-y".to_owned()),
            Value::String("@solvela/mcp-server".to_owned()),
        ]),
    );
    obj.insert("env".to_owned(), Value::Object(env));

    if cfg.cursor_use_envfile {
        obj.insert(
            "envFile".to_owned(),
            Value::String("${userHome}/.solvela/env".to_owned()),
        );
    }

    Value::Object(obj)
}

/// Build the JSON object that `openclaw mcp set solvela '<json>'` receives.
pub fn build_openclaw_entry(cfg: &InstallConfig) -> Value {
    let env = build_env_block(cfg, true, false);

    let mut obj = Map::new();
    obj.insert("command".to_owned(), Value::String("npx".to_owned()));
    obj.insert(
        "args".to_owned(),
        Value::Array(vec![
            Value::String("-y".to_owned()),
            Value::String("@solvela/mcp-server".to_owned()),
        ]),
    );
    obj.insert("env".to_owned(), Value::Object(env));
    Value::Object(obj)
}

/// Build the env block for the MCP server entry.
///
/// - `is_openclaw`: OpenClaw format omits `SOLANA_WALLET_ADDRESS` inline (set via CLI).
/// - `use_env_interpolation`: Cursor-only; emits `${env:SOLANA_WALLET_ADDRESS}` and
///   `${env:SOLANA_RPC_URL}` instead of literal values. Cursor resolves these at
///   runtime from the shell environment or the `envFile`.
fn build_env_block(
    cfg: &InstallConfig,
    is_openclaw: bool,
    use_env_interpolation: bool,
) -> Map<String, Value> {
    let mut env = Map::new();

    env.insert(
        "SOLVELA_API_URL".to_owned(),
        Value::String(cfg.gateway_url.clone()),
    );

    if let Some(ref budget) = cfg.budget {
        env.insert(
            "SOLVELA_SESSION_BUDGET".to_owned(),
            Value::String(budget.clone()),
        );
    }

    env.insert(
        "SOLVELA_SIGNING_MODE".to_owned(),
        Value::String(cfg.signing_mode.clone()),
    );

    if !is_openclaw {
        if use_env_interpolation {
            // Cursor: emit ${env:VAR} placeholders; Cursor resolves them from shell/envFile.
            env.insert(
                "SOLANA_WALLET_ADDRESS".to_owned(),
                Value::String("${env:SOLANA_WALLET_ADDRESS}".to_owned()),
            );
        } else if let Some(ref addr) = cfg.wallet_address {
            env.insert(
                "SOLANA_WALLET_ADDRESS".to_owned(),
                Value::String(addr.clone()),
            );
        }
    }

    if use_env_interpolation {
        // Cursor: reference RPC URL from environment/envFile.
        env.insert(
            "SOLANA_RPC_URL".to_owned(),
            Value::String("${env:SOLANA_RPC_URL}".to_owned()),
        );
    } else {
        env.insert(
            "SOLANA_RPC_URL".to_owned(),
            Value::String("https://api.mainnet-beta.solana.com".to_owned()),
        );
    }

    // SOLANA_WALLET_KEY is intentionally omitted by default.
    // Only include it when the user explicitly passes --include-key.
    if cfg.include_key {
        env.insert(
            "SOLANA_WALLET_KEY".to_owned(),
            Value::String("<paste-your-base58-private-key-here>".to_owned()),
        );
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> InstallConfig {
        InstallConfig::new("https://api.solvela.ai".to_owned())
    }

    fn cfg_with_all() -> InstallConfig {
        let mut c = default_cfg();
        c.wallet_address = Some("TestPubkey123".to_owned());
        c.budget = Some("2.50".to_owned());
        c.signing_mode = "escrow".to_owned();
        c
    }

    #[test]
    fn test_claude_entry_no_wallet_key_by_default() {
        let entry = build_claude_entry(&default_cfg());
        let env = entry["env"].as_object().unwrap();
        assert!(
            !env.contains_key("SOLANA_WALLET_KEY"),
            "wallet key must not appear by default"
        );
    }

    #[test]
    fn test_claude_entry_wallet_key_with_include_key() {
        let mut cfg = default_cfg();
        cfg.include_key = true;
        let entry = build_claude_entry(&cfg);
        let env = entry["env"].as_object().unwrap();
        assert!(
            env.contains_key("SOLANA_WALLET_KEY"),
            "wallet key should appear with --include-key"
        );
    }

    #[test]
    fn test_claude_entry_budget_omitted_when_none() {
        let entry = build_claude_entry(&default_cfg());
        let env = entry["env"].as_object().unwrap();
        assert!(
            !env.contains_key("SOLVELA_SESSION_BUDGET"),
            "budget must be absent when not set"
        );
    }

    #[test]
    fn test_claude_entry_budget_present_when_set() {
        let cfg = cfg_with_all();
        let entry = build_claude_entry(&cfg);
        let env = entry["env"].as_object().unwrap();
        assert_eq!(env["SOLVELA_SESSION_BUDGET"].as_str().unwrap(), "2.50");
    }

    #[test]
    fn test_claude_entry_command_and_args() {
        let entry = build_claude_entry(&default_cfg());
        assert_eq!(entry["command"].as_str().unwrap(), "npx");
        let args: Vec<&str> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(args, vec!["-y", "@solvela/mcp-server"]);
    }

    #[test]
    fn test_cursor_entry_has_type_stdio() {
        let entry = build_cursor_entry(&default_cfg());
        assert_eq!(entry["type"].as_str().unwrap(), "stdio");
    }

    #[test]
    fn test_cursor_entry_no_wallet_key_by_default() {
        let entry = build_cursor_entry(&default_cfg());
        let env = entry["env"].as_object().unwrap();
        assert!(!env.contains_key("SOLANA_WALLET_KEY"));
    }

    #[test]
    fn test_cursor_entry_envfile_interpolation_by_default() {
        // Default: cursor_use_envfile = true
        let cfg = default_cfg();
        assert!(cfg.cursor_use_envfile);
        let entry = build_cursor_entry(&cfg);

        // envFile field present
        assert_eq!(
            entry["envFile"].as_str().unwrap(),
            "${userHome}/.solvela/env"
        );

        // env vars use ${env:VAR} interpolation
        let env = entry["env"].as_object().unwrap();
        assert_eq!(
            env["SOLANA_WALLET_ADDRESS"].as_str().unwrap(),
            "${env:SOLANA_WALLET_ADDRESS}"
        );
        assert_eq!(
            env["SOLANA_RPC_URL"].as_str().unwrap(),
            "${env:SOLANA_RPC_URL}"
        );
    }

    #[test]
    fn test_cursor_entry_literal_when_no_envfile() {
        let mut cfg = default_cfg();
        cfg.cursor_use_envfile = false;
        cfg.wallet_address = Some("MyWallet123".to_owned());
        let entry = build_cursor_entry(&cfg);

        // envFile field absent
        assert!(entry.get("envFile").is_none());

        // env vars are literals
        let env = entry["env"].as_object().unwrap();
        assert_eq!(
            env["SOLANA_WALLET_ADDRESS"].as_str().unwrap(),
            "MyWallet123"
        );
        assert_eq!(
            env["SOLANA_RPC_URL"].as_str().unwrap(),
            "https://api.mainnet-beta.solana.com"
        );
    }

    #[test]
    fn test_cursor_entry_include_key_disables_envfile() {
        let mut cfg = default_cfg();
        cfg.include_key = true;
        cfg.cursor_use_envfile = false; // --include-key disables envfile in mod.rs
        let entry = build_cursor_entry(&cfg);

        // envFile absent
        assert!(entry.get("envFile").is_none());
        // SOLANA_WALLET_KEY placeholder present
        let env = entry["env"].as_object().unwrap();
        assert!(env.contains_key("SOLANA_WALLET_KEY"));
    }

    #[test]
    fn test_openclaw_entry_shape() {
        let entry = build_openclaw_entry(&default_cfg());
        // OpenClaw entry should have command + args + env
        assert!(entry["command"].is_string());
        assert!(entry["args"].is_array());
        assert!(entry["env"].is_object());
        // No SOLANA_WALLET_ADDRESS in openclaw entry
        let env = entry["env"].as_object().unwrap();
        assert!(!env.contains_key("SOLANA_WALLET_ADDRESS"));
        // But should have RPC URL (literal for openclaw)
        assert!(env.contains_key("SOLANA_RPC_URL"));
        assert!(!env["SOLANA_RPC_URL"].as_str().unwrap().contains("${env:"));
    }

    #[test]
    fn test_openclaw_entry_no_wallet_key_by_default() {
        let entry = build_openclaw_entry(&default_cfg());
        let env = entry["env"].as_object().unwrap();
        assert!(!env.contains_key("SOLANA_WALLET_KEY"));
    }

    #[test]
    fn test_gateway_url_written() {
        let mut cfg = default_cfg();
        cfg.gateway_url = "https://my.gateway.ai".to_owned();
        let entry = build_claude_entry(&cfg);
        let env = entry["env"].as_object().unwrap();
        assert_eq!(
            env["SOLVELA_API_URL"].as_str().unwrap(),
            "https://my.gateway.ai"
        );
    }

    #[test]
    fn test_signing_mode_written() {
        let cfg = cfg_with_all();
        let entry = build_claude_entry(&cfg);
        let env = entry["env"].as_object().unwrap();
        assert_eq!(env["SOLVELA_SIGNING_MODE"].as_str().unwrap(), "escrow");
    }

    #[test]
    fn test_wallet_address_written_for_claude() {
        let cfg = cfg_with_all();
        let entry = build_claude_entry(&cfg);
        let env = entry["env"].as_object().unwrap();
        assert_eq!(
            env["SOLANA_WALLET_ADDRESS"].as_str().unwrap(),
            "TestPubkey123"
        );
    }
}
