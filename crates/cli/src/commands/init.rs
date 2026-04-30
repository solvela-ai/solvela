//! `solvela init` — scaffold a `.env` and a wallet for a fresh project.
//!
//! Two modes:
//!
//! * `--devnet` (default) — RPC pointed at devnet, demo mode auto-activates,
//!   recommend `solvela wallet airdrop` to get SOL.
//! * `--mainnet` — RPC pointed at mainnet, **no airdrop**, the user must fund
//!   the wallet themselves.
//!
//! When `.env` already exists in CWD the command prompts for confirmation
//! unless `--force` is passed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::commands::wallet;

const DEVNET_RPC: &str = "https://api.devnet.solana.com";
const MAINNET_RPC: &str = "https://api.mainnet-beta.solana.com";

/// Path to the bundled `.env.example` (copied next to the workspace root).
const ENV_EXAMPLE_FILENAME: &str = ".env.example";

/// Result of patching env contents — used in unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchedEnv {
    pub contents: String,
}

/// Apply the `--devnet` / `--mainnet` patches to `.env.example` contents.
///
/// * Sets `SOLVELA_SOLANA_RPC_URL` to the chosen RPC URL (in-place edit when
///   the line is present, append otherwise).
/// * Leaves provider keys untouched (so they remain blank → demo mode).
pub(crate) fn patch_env_contents(template: &str, rpc_url: &str) -> PatchedEnv {
    let mut found = false;
    let mut out = String::with_capacity(template.len());
    for line in template.lines() {
        if line.starts_with("SOLVELA_SOLANA_RPC_URL=") {
            out.push_str(&format!("SOLVELA_SOLANA_RPC_URL={rpc_url}"));
            out.push('\n');
            found = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("SOLVELA_SOLANA_RPC_URL={rpc_url}\n"));
    }
    PatchedEnv { contents: out }
}

/// Find the `.env.example` file. Searches CWD, then walks up to four parents.
fn find_env_example() -> Result<PathBuf> {
    let mut cur = std::env::current_dir().context("current_dir")?;
    for _ in 0..5 {
        let candidate = cur.join(ENV_EXAMPLE_FILENAME);
        if candidate.exists() {
            return Ok(candidate);
        }
        if !cur.pop() {
            break;
        }
    }
    Err(anyhow!(
        "could not locate {ENV_EXAMPLE_FILENAME} in CWD or parents"
    ))
}

/// Prompt for y/N confirmation on stdin. Returns `false` on empty/no.
fn confirm_overwrite(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Mode selector for [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitMode {
    Devnet,
    Mainnet,
}

impl InitMode {
    fn rpc_url(self) -> &'static str {
        match self {
            InitMode::Devnet => DEVNET_RPC,
            InitMode::Mainnet => MAINNET_RPC,
        }
    }
}

/// Public entrypoint — invoked from `main.rs`.
pub async fn run(mode: InitMode, force: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("current_dir")?;
    let env_path = cwd.join(".env");

    // 1. Refuse to overwrite without consent.
    if env_path.exists() && !force {
        let confirmed = confirm_overwrite(&format!(
            ".env already exists at {} — overwrite?",
            env_path.display()
        ))?;
        if !confirmed {
            println!("Aborted. Re-run with --force to overwrite non-interactively.");
            return Ok(());
        }
    }

    // 2. Locate .env.example and patch it.
    let template_path = find_env_example()?;
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("failed to read {}", template_path.display()))?;
    let patched = patch_env_contents(&template, mode.rpc_url());
    fs::write(&env_path, &patched.contents)
        .with_context(|| format!("failed to write {}", env_path.display()))?;

    // 3. Generate a fresh wallet (or reuse the existing one).
    let wallet_path = wallet::wallet_file_path();
    let address = if wallet::wallet_exists() {
        let existing = wallet::load_wallet()?;
        existing["address"]
            .as_str()
            .unwrap_or("<unknown>")
            .to_string()
    } else {
        wallet::generate_and_save_wallet()?
    };

    // 4. Ready summary.
    print_ready_summary(&env_path, &wallet_path, &address, mode);

    Ok(())
}

fn print_ready_summary(env_path: &Path, wallet_path: &Path, address: &str, mode: InitMode) {
    println!();
    println!("Solvela project ready!");
    println!();
    println!("  .env           {}", env_path.display());
    println!("  Wallet file    {}", wallet_path.display());
    println!("  Wallet address {address}");
    println!("  Solana RPC     {}", mode.rpc_url());
    println!();
    println!("Next steps:");
    println!("  # 1. Start the gateway");
    println!("  RUST_LOG=info cargo run -p gateway");
    println!();
    println!("  # 2. Send a test request (demo provider, no payment required)");
    println!("  curl -s http://localhost:8402/v1/chat/completions \\");
    println!("    -H 'Content-Type: application/json' \\");
    println!(
        "    -d '{{\"model\":\"demo\",\"messages\":[{{\"role\":\"user\",\"content\":\"Hello\"}}]}}'"
    );
    println!();

    match mode {
        InitMode::Devnet => {
            println!("Hint: Run `solvela wallet airdrop` to get devnet SOL.");
        }
        InitMode::Mainnet => {
            println!("WARNING: Mainnet mode — no airdrop will be issued.");
            println!("  You must fund {address} with SOL and USDC manually.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Set HOME to a tempdir for the duration of a test.
    fn with_temp_home() -> TempDir {
        let tmp = TempDir::new().expect("create tempdir");
        std::env::set_var("HOME", tmp.path());
        tmp
    }

    #[test]
    fn patch_env_replaces_existing_rpc_line() {
        let template = "SOLVELA_HOST=0.0.0.0\nSOLVELA_SOLANA_RPC_URL=https://api.mainnet-beta.solana.com\nFOO=bar\n";
        let patched = patch_env_contents(template, DEVNET_RPC);
        assert!(patched
            .contents
            .contains(&format!("SOLVELA_SOLANA_RPC_URL={DEVNET_RPC}")));
        assert!(!patched.contents.contains("mainnet-beta"));
        assert!(patched.contents.contains("SOLVELA_HOST=0.0.0.0"));
        assert!(patched.contents.contains("FOO=bar"));
    }

    #[test]
    fn patch_env_appends_when_missing() {
        let template = "FOO=bar\n";
        let patched = patch_env_contents(template, DEVNET_RPC);
        assert!(patched
            .contents
            .contains(&format!("SOLVELA_SOLANA_RPC_URL={DEVNET_RPC}")));
        assert!(patched.contents.contains("FOO=bar"));
    }

    #[test]
    fn patch_env_uses_mainnet_url_in_mainnet_mode() {
        let template = "SOLVELA_SOLANA_RPC_URL=https://api.devnet.solana.com\n";
        let patched = patch_env_contents(template, InitMode::Mainnet.rpc_url());
        assert!(patched.contents.contains("mainnet-beta"));
    }

    #[test]
    fn patch_env_does_not_set_provider_keys() {
        let template = "OPENAI_API_KEY=\nANTHROPIC_API_KEY=\n";
        let patched = patch_env_contents(template, DEVNET_RPC);
        // Provider keys should remain blank (demo mode).
        assert!(patched.contents.contains("OPENAI_API_KEY=\n"));
        assert!(patched.contents.contains("ANTHROPIC_API_KEY=\n"));
    }

    #[test]
    fn init_mode_rpc_url_is_correct() {
        assert_eq!(InitMode::Devnet.rpc_url(), DEVNET_RPC);
        assert_eq!(InitMode::Mainnet.rpc_url(), MAINNET_RPC);
    }

    #[tokio::test]
    async fn init_devnet_writes_env_and_creates_wallet() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _home = with_temp_home();

        // Emulate a workspace: cwd contains .env.example
        let workdir = TempDir::new().expect("workdir");
        std::fs::write(
            workdir.path().join(".env.example"),
            "SOLVELA_SOLANA_RPC_URL=https://api.mainnet-beta.solana.com\nOPENAI_API_KEY=\n",
        )
        .expect("write template");

        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(workdir.path()).expect("chdir");

        let result = run(InitMode::Devnet, false).await;

        // Restore cwd before any assertion failure that would leak state.
        std::env::set_current_dir(&original_cwd).expect("restore cwd");

        assert!(result.is_ok(), "init should succeed: {:?}", result.err());

        // .env exists with devnet URL
        let env_contents = std::fs::read_to_string(workdir.path().join(".env")).expect("read .env");
        assert!(env_contents.contains("api.devnet.solana.com"));
        assert!(!env_contents.contains("mainnet-beta"));

        // Wallet exists
        assert!(wallet::wallet_exists(), "wallet should be created");
    }

    #[tokio::test]
    async fn init_aborts_when_env_exists_and_no_force() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _home = with_temp_home();

        let workdir = TempDir::new().expect("workdir");
        std::fs::write(
            workdir.path().join(".env.example"),
            "SOLVELA_SOLANA_RPC_URL=https://api.devnet.solana.com\n",
        )
        .expect("write template");
        // Pre-create a .env so init should refuse without --force.
        let existing = "EXISTING=true\n";
        std::fs::write(workdir.path().join(".env"), existing).expect("write existing env");

        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(workdir.path()).expect("chdir");

        // Without --force and without a real TTY for stdin, read_line returns
        // EOF/empty → confirm_overwrite returns false → init aborts cleanly.
        let result = run(InitMode::Devnet, false).await;

        std::env::set_current_dir(&original_cwd).expect("restore cwd");

        assert!(result.is_ok(), "abort path should not error");
        let env_contents = std::fs::read_to_string(workdir.path().join(".env")).expect("read .env");
        assert_eq!(
            env_contents, existing,
            "existing .env should not be overwritten"
        );
    }

    #[tokio::test]
    async fn init_force_overwrites_existing_env() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _home = with_temp_home();

        let workdir = TempDir::new().expect("workdir");
        std::fs::write(
            workdir.path().join(".env.example"),
            "SOLVELA_SOLANA_RPC_URL=https://api.devnet.solana.com\n",
        )
        .expect("write template");
        std::fs::write(workdir.path().join(".env"), "EXISTING=yes\n").expect("write existing");

        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(workdir.path()).expect("chdir");

        let result = run(InitMode::Devnet, true).await;

        std::env::set_current_dir(&original_cwd).expect("restore cwd");

        assert!(result.is_ok(), "force should overwrite without prompting");
        let env_contents = std::fs::read_to_string(workdir.path().join(".env")).expect("read .env");
        assert!(!env_contents.contains("EXISTING=yes"));
        assert!(env_contents.contains("api.devnet.solana.com"));
    }

    #[tokio::test]
    async fn init_mainnet_uses_mainnet_rpc() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _home = with_temp_home();

        let workdir = TempDir::new().expect("workdir");
        std::fs::write(
            workdir.path().join(".env.example"),
            "SOLVELA_SOLANA_RPC_URL=https://api.devnet.solana.com\n",
        )
        .expect("write template");

        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(workdir.path()).expect("chdir");

        let result = run(InitMode::Mainnet, false).await;

        std::env::set_current_dir(&original_cwd).expect("restore cwd");

        assert!(result.is_ok(), "mainnet init should succeed");
        let env_contents = std::fs::read_to_string(workdir.path().join(".env")).expect("read .env");
        assert!(env_contents.contains("mainnet-beta"));
    }

    #[tokio::test]
    async fn init_missing_env_example_fails() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _home = with_temp_home();

        let workdir = TempDir::new().expect("workdir");
        // No .env.example — and ensure no parent has one either by going deep.
        let deep = workdir.path().join("a/b/c/d/e/f");
        std::fs::create_dir_all(&deep).expect("mkdir deep");

        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&deep).expect("chdir");

        let result = run(InitMode::Devnet, false).await;

        std::env::set_current_dir(&original_cwd).expect("restore cwd");

        // This may succeed if a parent of the workdir happens to have a
        // .env.example (unlikely in CI), but the search depth is bounded to
        // 5, so a depth-6 path under a clean tempdir cannot find one.
        assert!(result.is_err(), "expected failure to find .env.example");
        assert!(
            result.unwrap_err().to_string().contains(".env.example"),
            "error should mention .env.example"
        );
    }
}
