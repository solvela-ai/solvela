use std::io::{BufRead, Write};
use std::path::PathBuf;

use solvela_client::{ClientBuilder, SolvelaClient, Wallet};
use solvela_client_cli_args::{load_wallet, save_wallet, GatewayArgs, RpcArgs, WalletArgs};

use crate::WalletAction;

pub async fn run(
    action: WalletAction,
    wallet_args: &WalletArgs,
    gateway_args: &GatewayArgs,
    rpc_args: &RpcArgs,
) -> Result<(), String> {
    match action {
        WalletAction::Create { force } => create(wallet_args, force),
        WalletAction::Import {
            mnemonic,
            keypair,
            force,
        } => {
            if mnemonic {
                import_mnemonic(wallet_args, force)
            } else if keypair {
                import_keypair(wallet_args, force)
            } else {
                Err("specify --mnemonic or --keypair".to_string())
            }
        }
        WalletAction::Balance => balance(wallet_args, gateway_args, rpc_args).await,
        WalletAction::Address => address(wallet_args),
        WalletAction::Export { yes } => export(wallet_args, yes),
    }
}

fn create(wallet_args: &WalletArgs, force: bool) -> Result<(), String> {
    let (wallet, mnemonic, path) = do_create(&wallet_args.wallet_file, force)?;

    println!("===");
    println!("{}", wallet.address());
    println!("===");
    println!();
    println!("Wallet saved to: {}", path.display());
    println!();
    println!("=== BACKUP YOUR SEED PHRASE ===");
    println!("{mnemonic}");
    println!("=== END SEED PHRASE ===");

    Ok(())
}

fn do_create(wallet_file: &str, force: bool) -> Result<(Wallet, String, PathBuf), String> {
    let (wallet, mnemonic) = Wallet::create();
    let bytes = wallet.to_keypair_bytes();
    let path = save_wallet(wallet_file, &bytes, force)?;
    Ok((wallet, mnemonic, path))
}

fn import_mnemonic(wallet_args: &WalletArgs, force: bool) -> Result<(), String> {
    let phrase = read_secret_line("Enter seed phrase: ")?;
    let (address, path) = do_import_mnemonic(&phrase, &wallet_args.wallet_file, force)?;

    println!("Imported address: {address}");
    println!("Wallet saved to: {}", path.display());

    Ok(())
}

fn do_import_mnemonic(
    phrase: &str,
    wallet_file: &str,
    force: bool,
) -> Result<(String, PathBuf), String> {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return Err("empty seed phrase".to_string());
    }

    let wallet = Wallet::from_mnemonic(phrase).map_err(|e| format!("invalid mnemonic: {e}"))?;
    let bytes = wallet.to_keypair_bytes();
    let path = save_wallet(wallet_file, &bytes, force)?;

    Ok((wallet.address(), path))
}

fn import_keypair(wallet_args: &WalletArgs, force: bool) -> Result<(), String> {
    let b58 = read_secret_line("Enter base58 keypair: ")?;
    let (address, path) = do_import_keypair(&b58, &wallet_args.wallet_file, force)?;

    println!("Imported address: {address}");
    println!("Wallet saved to: {}", path.display());

    Ok(())
}

fn do_import_keypair(
    b58: &str,
    wallet_file: &str,
    force: bool,
) -> Result<(String, PathBuf), String> {
    let b58 = b58.trim();
    if b58.is_empty() {
        return Err("empty keypair input".to_string());
    }

    let wallet = Wallet::from_keypair_b58(b58).map_err(|e| format!("invalid keypair: {e}"))?;
    let bytes = wallet.to_keypair_bytes();
    let path = save_wallet(wallet_file, &bytes, force)?;

    Ok((wallet.address(), path))
}

async fn balance(
    wallet_args: &WalletArgs,
    gateway_args: &GatewayArgs,
    rpc_args: &RpcArgs,
) -> Result<(), String> {
    let wallet = load_wallet(wallet_args)?;
    let config = ClientBuilder::new()
        .gateway_url(&gateway_args.gateway)
        .rpc_url(&rpc_args.rpc_url)
        .build_config();

    let client =
        SolvelaClient::new(wallet, config).map_err(|e| format!("failed to create client: {e}"))?;

    let balance = client
        .usdc_balance()
        .await
        .map_err(|e| format!("failed to fetch balance: {e}"))?;

    println!("{balance:.6} USDC");

    Ok(())
}

fn address(wallet_args: &WalletArgs) -> Result<(), String> {
    let address = do_address(wallet_args)?;
    println!("{address}");
    Ok(())
}

fn do_address(wallet_args: &WalletArgs) -> Result<String, String> {
    let wallet = load_wallet(wallet_args)?;
    Ok(wallet.address())
}

fn export(wallet_args: &WalletArgs, yes: bool) -> Result<(), String> {
    let wallet = load_wallet(wallet_args)?;

    if !yes {
        let answer =
            read_stdin_line("WARNING: This will print your private key. Continue? [y/N] ")?;
        if !confirm_export(false, &answer) {
            return Err("aborted".to_string());
        }
    }

    println!("{}", wallet.to_keypair_b58());

    Ok(())
}

fn confirm_export(yes: bool, answer: &str) -> bool {
    if yes {
        return true;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

fn read_stdin_line(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    std::io::stderr()
        .flush()
        .map_err(|e| format!("failed to flush stderr: {e}"))?;

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    Ok(line)
}

// Reads a secret without echoing to the terminal so mnemonics/keypairs don't
// leak into scrollback, shell history, or screen-recording buffers (#322).
// On non-TTY stdin (piped input), rpassword reads the line as-is — that path
// is acceptable because the secret never reaches a terminal in the first place.
fn read_secret_line(prompt: &str) -> Result<String, String> {
    rpassword::prompt_password(prompt).map_err(|e| format!("failed to read secret: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("solvela_walletcmd_{prefix}_{pid}_{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- do_create ----------

    #[test]
    fn test_do_create_writes_valid_keypair_file() {
        let dir = unique_temp_dir("create_ok");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let (wallet, mnemonic, path) =
            do_create(&wallet_file, false).expect("create should succeed");

        assert!(path.exists(), "wallet file should be written");
        assert!(!mnemonic.is_empty(), "mnemonic should be non-empty");
        assert!(
            mnemonic.split_whitespace().count() >= 12,
            "mnemonic should be at least 12 words"
        );

        let raw = std::fs::read_to_string(&path).expect("read wallet file");
        let bytes: Vec<u8> = serde_json::from_str(&raw).expect("parse JSON byte array");
        let reloaded = Wallet::from_keypair_bytes(&bytes).expect("reload keypair");
        assert_eq!(reloaded.address(), wallet.address());

        cleanup(&dir);
    }

    #[test]
    fn test_do_create_refuses_overwrite_without_force() {
        let dir = unique_temp_dir("create_no_force");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        do_create(&wallet_file, false).expect("first create should succeed");
        let err =
            do_create(&wallet_file, false).expect_err("second create should fail without force");
        assert!(err.contains("already exists"), "unexpected error: {err}");

        cleanup(&dir);
    }

    #[test]
    fn test_do_create_overwrites_with_force() {
        let dir = unique_temp_dir("create_force");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let (first, _, _) = do_create(&wallet_file, false).expect("first create");
        let (second, _, _) = do_create(&wallet_file, true).expect("force overwrite");
        assert_ne!(first.address(), second.address());

        cleanup(&dir);
    }

    // ---------- do_import_mnemonic ----------

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_do_import_mnemonic_happy_path() {
        let dir = unique_temp_dir("import_mn_ok");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let (address, path) =
            do_import_mnemonic(TEST_MNEMONIC, &wallet_file, false).expect("import should succeed");

        assert!(!address.is_empty());
        assert!(path.exists());

        let dir2 = unique_temp_dir("import_mn_det");
        let wallet_file2 = dir2.join("wallet.json").to_string_lossy().to_string();
        let (address2, _) = do_import_mnemonic(TEST_MNEMONIC, &wallet_file2, false)
            .expect("second import should succeed");
        assert_eq!(address, address2, "BIP-39 import should be deterministic");

        cleanup(&dir);
        cleanup(&dir2);
    }

    #[test]
    fn test_do_import_mnemonic_trims_whitespace() {
        let dir = unique_temp_dir("import_mn_trim");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let padded = format!("  \t{TEST_MNEMONIC}\n");
        let (address, _) = do_import_mnemonic(&padded, &wallet_file, false)
            .expect("import should succeed with surrounding whitespace");
        assert!(!address.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn test_do_import_mnemonic_rejects_empty() {
        let err = do_import_mnemonic("", "/dev/null", false).expect_err("empty should fail");
        assert!(err.contains("empty"), "unexpected error: {err}");

        let err = do_import_mnemonic("   \n\t  ", "/dev/null", false)
            .expect_err("whitespace-only should fail");
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn test_do_import_mnemonic_rejects_invalid() {
        let dir = unique_temp_dir("import_mn_bad");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let err = do_import_mnemonic("not a real mnemonic at all", &wallet_file, false)
            .expect_err("invalid mnemonic should fail");
        assert!(err.contains("invalid mnemonic"), "unexpected error: {err}");

        cleanup(&dir);
    }

    // ---------- do_import_keypair ----------

    #[test]
    fn test_do_import_keypair_happy_path() {
        let dir = unique_temp_dir("import_kp_ok");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        // Generate a fresh wallet, export it as base58, and re-import via the
        // command path. The reimported address must match the original.
        let (source, _) = Wallet::create();
        let b58 = source.to_keypair_b58();
        let expected = source.address();

        let (address, path) =
            do_import_keypair(&b58, &wallet_file, false).expect("import should succeed");
        assert_eq!(address, expected);
        assert!(path.exists());

        cleanup(&dir);
    }

    #[test]
    fn test_do_import_keypair_rejects_empty() {
        let err = do_import_keypair("", "/dev/null", false).expect_err("empty should fail");
        assert!(err.contains("empty"), "unexpected error: {err}");

        let err =
            do_import_keypair("\t\n", "/dev/null", false).expect_err("whitespace should fail");
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn test_do_import_keypair_rejects_invalid_base58() {
        let dir = unique_temp_dir("import_kp_bad");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let err = do_import_keypair("not-valid-base58!!!", &wallet_file, false)
            .expect_err("invalid base58 should fail");
        assert!(err.contains("invalid keypair"), "unexpected error: {err}");

        cleanup(&dir);
    }

    // ---------- do_address ----------

    #[test]
    fn test_do_address_reads_wallet_file() {
        let dir = unique_temp_dir("address");
        let wallet_file = dir.join("wallet.json");

        // Seed a known wallet on disk in Solana CLI byte-array format.
        let (source, _) = Wallet::create();
        let expected = source.address();
        let bytes = source.to_keypair_bytes();
        std::fs::write(
            &wallet_file,
            serde_json::to_string(&bytes.to_vec()).unwrap(),
        )
        .unwrap();

        let env_var = format!("SOLVELA_TEST_ADDRESS_NOENV_{}", std::process::id());
        std::env::remove_var(&env_var);
        let args = WalletArgs {
            wallet_env: env_var,
            wallet_file: wallet_file.to_string_lossy().to_string(),
        };

        let address = do_address(&args).expect("address lookup should succeed");
        assert_eq!(address, expected);

        cleanup(&dir);
    }

    #[test]
    fn test_do_address_propagates_load_error() {
        let env_var = format!("SOLVELA_TEST_ADDRESS_MISSING_{}", std::process::id());
        std::env::remove_var(&env_var);
        let args = WalletArgs {
            wallet_env: env_var,
            wallet_file: "/nonexistent/path/wallet.json".to_string(),
        };

        let err = do_address(&args).expect_err("missing wallet should error");
        assert!(err.contains("no wallet found"), "unexpected error: {err}");
    }

    // ---------- confirm_export ----------

    #[test]
    fn test_confirm_export_yes_flag_bypasses_prompt() {
        assert!(confirm_export(true, ""));
        assert!(confirm_export(true, "no"));
        assert!(confirm_export(true, "anything"));
    }

    #[test]
    fn test_confirm_export_accepts_y_and_yes() {
        assert!(confirm_export(false, "y"));
        assert!(confirm_export(false, "Y"));
        assert!(confirm_export(false, "yes"));
        assert!(confirm_export(false, "YES"));
        assert!(confirm_export(false, "  y\n"));
        assert!(confirm_export(false, "Yes\n"));
    }

    #[test]
    fn test_confirm_export_rejects_other_input() {
        assert!(!confirm_export(false, ""));
        assert!(!confirm_export(false, "n"));
        assert!(!confirm_export(false, "no"));
        assert!(!confirm_export(false, "maybe"));
        assert!(!confirm_export(false, "\n"));
    }

    // ---------- end-to-end: create then address ----------

    #[test]
    fn test_create_then_address_round_trip() {
        let dir = unique_temp_dir("e2e_create_addr");
        let wallet_file = dir.join("wallet.json").to_string_lossy().to_string();

        let (wallet, _mnemonic, _path) = do_create(&wallet_file, false).expect("create");

        let env_var = format!("SOLVELA_TEST_E2E_NOENV_{}", std::process::id());
        std::env::remove_var(&env_var);
        let args = WalletArgs {
            wallet_env: env_var,
            wallet_file: wallet_file.clone(),
        };

        let address = do_address(&args).expect("address read");
        assert_eq!(address, wallet.address());

        cleanup(&dir);
    }
}
