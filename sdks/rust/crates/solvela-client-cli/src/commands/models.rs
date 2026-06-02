use std::io::IsTerminal;

use solvela_client::{ClientBuilder, SolvelaClient, Wallet};
use solvela_client_cli_args::GatewayArgs;

pub async fn run(
    provider: Option<&str>,
    json: bool,
    gateway_args: &GatewayArgs,
) -> Result<(), String> {
    // Models endpoint is public — use a throwaway wallet
    let (wallet, _mnemonic) = Wallet::create();
    let config = ClientBuilder::new()
        .gateway_url(&gateway_args.gateway)
        .build_config();

    let client =
        SolvelaClient::new(wallet, config).map_err(|e| format!("failed to create client: {e}"))?;

    let models = client
        .models()
        .await
        .map_err(|e| format!("failed to fetch models: {e}"))?;

    let models = apply_filter_and_sort(models, provider);

    if json || !std::io::stdout().is_terminal() {
        let output = serde_json::to_string_pretty(&models)
            .map_err(|e| format!("failed to serialize models: {e}"))?;
        println!("{output}");
    } else {
        print_table(&models);
    }

    Ok(())
}

fn apply_filter_and_sort(
    mut models: Vec<solvela_protocol::ModelInfo>,
    provider: Option<&str>,
) -> Vec<solvela_protocol::ModelInfo> {
    if let Some(filter) = provider {
        let filter_lower = filter.to_lowercase();
        models.retain(|m| m.provider.to_lowercase() == filter_lower);
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models
}

fn print_table(models: &[solvela_protocol::ModelInfo]) {
    const ID_W: usize = 35;
    const PROV_W: usize = 12;
    const COST_W: usize = 10;
    const CTX_W: usize = 8;

    println!(
        "{:<ID_W$} {:<PROV_W$} {:>COST_W$} {:>COST_W$} {:>CTX_W$}",
        "ID", "Provider", "In $/M", "Out $/M", "Context",
    );
    println!("{}", "-".repeat(ID_W + PROV_W + COST_W * 2 + CTX_W + 4));

    for m in models {
        println!(
            "{:<ID_W$} {:<PROV_W$} {:>COST_W$.4} {:>COST_W$.4} {:>CTX_W$}",
            truncate(&m.id, ID_W),
            truncate(&m.provider, PROV_W),
            m.input_cost_per_million,
            m.output_cost_per_million,
            format_context(m.context_window),
        );
    }

    println!();
    println!("{} model(s)", models.len());
}

fn format_context(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        let m = tokens / 1_000_000;
        format!("{m}M")
    } else if tokens >= 1_000 {
        let k = tokens / 1_000;
        format!("{k}K")
    } else {
        tokens.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = max.saturating_sub(3);
        let prefix: String = s.chars().take(end).collect();
        format!("{prefix}...")
    }
}

#[cfg(test)]
mod tests {
    use solvela_protocol::ModelInfo;

    use super::*;

    fn mi(id: &str, provider: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            provider: provider.to_string(),
            display_name: id.to_string(),
            input_cost_per_million: 1.0,
            output_cost_per_million: 2.0,
            context_window: 128_000,
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
        }
    }

    #[test]
    fn test_format_context_k() {
        assert_eq!(format_context(128_000), "128K");
        assert_eq!(format_context(4_000), "4K");
    }

    #[test]
    fn test_format_context_m() {
        assert_eq!(format_context(1_000_000), "1M");
        assert_eq!(format_context(2_000_000), "2M");
    }

    #[test]
    fn test_format_context_small() {
        assert_eq!(format_context(512), "512");
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        assert_eq!(truncate("abcdefghij", 7), "abcd...");
    }

    #[test]
    fn test_apply_filter_and_sort_no_filter() {
        let input = vec![mi("z/foo", "z"), mi("a/bar", "a"), mi("m/baz", "m")];
        let out = apply_filter_and_sort(input, None);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, "a/bar");
        assert_eq!(out[1].id, "m/baz");
        assert_eq!(out[2].id, "z/foo");
    }

    #[test]
    fn test_apply_filter_and_sort_with_filter() {
        let input = vec![
            mi("openai/gpt-4o", "openai"),
            mi("anthropic/claude", "anthropic"),
            mi("openai/gpt-3.5", "openai"),
        ];
        let out = apply_filter_and_sort(input, Some("openai"));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "openai/gpt-3.5");
        assert_eq!(out[1].id, "openai/gpt-4o");
    }

    #[test]
    fn test_apply_filter_and_sort_filter_is_case_insensitive() {
        let input = vec![
            mi("openai/gpt-4o", "OpenAI"),
            mi("anthropic/claude", "Anthropic"),
        ];
        let out = apply_filter_and_sort(input, Some("OPENAI"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].provider, "OpenAI");
    }

    #[test]
    fn test_apply_filter_and_sort_filter_no_match() {
        let input = vec![mi("openai/gpt-4o", "openai")];
        let out = apply_filter_and_sort(input, Some("nonexistent"));
        assert!(out.is_empty());
    }

    #[test]
    fn test_apply_filter_and_sort_empty_input() {
        let out = apply_filter_and_sort(vec![], Some("openai"));
        assert!(out.is_empty());
        let out = apply_filter_and_sort(vec![], None);
        assert!(out.is_empty());
    }

    #[test]
    fn test_print_table_runs_with_data() {
        // Smoke test: print_table writes via println! and does not panic on
        // names longer than the column width (exercises truncate) or on
        // contexts in the M/K/raw branches (exercises format_context).
        let models = vec![
            mi(
                "very-long-provider/very-long-model-name-that-exceeds-column",
                "longprovidername",
            ),
            mi("a", "x"),
        ];
        print_table(&models);
        print_table(&[]);
    }
}
