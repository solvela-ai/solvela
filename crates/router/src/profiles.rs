use serde::{Deserialize, Serialize};

/// Complexity tier assigned by the 15-dimension scorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Simple,
    Medium,
    Complex,
    Reasoning,
}

impl Tier {
    /// Map a raw score to a complexity tier.
    pub fn from_score(score: f64) -> Self {
        if score < 0.0 {
            Tier::Simple
        } else if score < 0.2 {
            Tier::Medium
        } else if score < 0.4 {
            Tier::Complex
        } else {
            Tier::Reasoning
        }
    }
}

/// Routing profile that determines model selection strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Cheapest possible model for each tier.
    Eco,
    /// Balanced cost/quality (default).
    Auto,
    /// Best available model regardless of cost.
    Premium,
    /// Free-tier only models.
    Free,
}

impl Profile {
    /// Parse a profile from a string (e.g., model alias).
    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias.to_lowercase().as_str() {
            "eco" | "cheap" | "budget" => Some(Profile::Eco),
            "auto" | "balanced" | "default" => Some(Profile::Auto),
            "premium" | "best" | "quality" => Some(Profile::Premium),
            "free" | "oss" | "open" => Some(Profile::Free),
            _ => None,
        }
    }
}

/// Routing table: maps (Profile, Tier) → model ID.
///
/// Based on the BlockRun ClawRouter routing table, adapted for our
/// Solana-first model catalog.
pub fn resolve_model(profile: Profile, tier: Tier) -> &'static str {
    match (profile, tier) {
        // ECO: cheapest capable model per tier. DeepSeek V4.1 Flash is the
        // documented DeepSeek name (thinking on by default, so it covers the
        // Reasoning tier too); the legacy `deepseek-chat` / `deepseek-reasoner`
        // names stay registered for explicit callers but are no longer
        // listed by DeepSeek, so nothing is auto-routed to them.
        (Profile::Eco, Tier::Simple) => "deepseek/deepseek-flash",
        (Profile::Eco, Tier::Medium) => "google/gemini-2.5-flash-lite",
        (Profile::Eco, Tier::Complex) => "deepseek/deepseek-flash",
        (Profile::Eco, Tier::Reasoning) => "deepseek/deepseek-flash",

        // AUTO: balanced cost/quality
        (Profile::Auto, Tier::Simple) => "google/gemini-2.5-flash",
        (Profile::Auto, Tier::Medium) => "xai/grok-code-fast-1",
        (Profile::Auto, Tier::Complex) => "google/gemini-3.1-pro",
        (Profile::Auto, Tier::Reasoning) => "xai/grok-4-fast-reasoning",

        // PREMIUM: best quality regardless of cost — the Claude 5 generation,
        // Haiku → Sonnet → Opus by complexity, Fable 5.2 (Anthropic's most
        // capable widely released model) for the Reasoning tier.
        (Profile::Premium, Tier::Simple) => "anthropic/claude-haiku-5-5",
        (Profile::Premium, Tier::Medium) => "anthropic/claude-sonnet-5-5",
        (Profile::Premium, Tier::Complex) => "anthropic/claude-opus-5-5",
        (Profile::Premium, Tier::Reasoning) => "anthropic/claude-fable-5-2",

        // FREE: NVIDIA NIM free-tier Nemotron models ($0/$0), tiered by
        // complexity (nano → super → ultra). The canonical Solvela key is
        // `nvidia/<model_id>` where model_id is itself the publisher-qualified
        // NVIDIA id (`nvidia/llama-...`), so these keys carry a doubled
        // `nvidia/` — that is the real registry key (see config/models.toml +
        // crates/router/src/models.rs `format!("{provider}/{model_id}")`).
        // Every target here MUST be a $0/$0 model — enforced by
        // `free_profile_targets_are_all_zero_cost` below — so free-tier callers
        // are never charged. A NIM outage/rate-limit falls back to
        // google/gemini-3.1-flash-lite (see `model_fallback_chain` in gateway).
        (Profile::Free, Tier::Simple) => "nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1",
        (Profile::Free, Tier::Medium) => "nvidia/nvidia/llama-3.3-nemotron-super-49b-v1",
        (Profile::Free, Tier::Complex) => "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1",
        (Profile::Free, Tier::Reasoning) => "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1",
    }
}

/// Model alias resolution: maps shorthand names to canonical model IDs.
pub fn resolve_alias(alias: &str) -> Option<&'static str> {
    match alias.to_lowercase().as_str() {
        "gpt5" | "gpt-5" => Some("openai/gpt-5.2"),
        "gpt5.5" | "gpt-5.5" => Some("openai/gpt-5.5"),
        "gpt6" | "gpt-6" | "sol" => Some("openai/gpt-6.2-sol"),
        "gpt6.1" | "gpt-6.1" => Some("openai/gpt-6.1-sol"),
        "luna" | "gpt-6-luna" => Some("openai/gpt-6-luna"),
        // Bare family names track the CURRENT Claude generation (5.5 / Fable
        // 5.2); the previous generation stays reachable via versioned aliases
        // below. The canonical key here MUST match one registered by
        // `from_toml` — see the cross-check test below. Previously `haiku`
        // pointed at `claude-3-5-haiku-20241022`, which has never been in
        // `config/models.toml`, so every request using the `haiku` shorthand
        // returned a 500 (registry lookup miss).
        "fable" | "claude-fable" => Some("anthropic/claude-fable-5-2"),
        "opus" | "claude-opus" => Some("anthropic/claude-opus-5-5"),
        "sonnet" | "claude-sonnet" => Some("anthropic/claude-sonnet-5-5"),
        "haiku" | "claude-haiku" => Some("anthropic/claude-haiku-5-5"),
        "opus4.8" | "opus-4.8" => Some("anthropic/claude-opus-4-8"),
        "sonnet4.6" | "sonnet-4.6" => Some("anthropic/claude-sonnet-4-6"),
        "haiku4.5" | "haiku-4.5" => Some("anthropic/claude-haiku-4-5-20251001"),
        "gemini" | "gemini-pro" => Some("google/gemini-3.1-pro"),
        "flash" | "gemini-flash" => Some("google/gemini-2.5-flash"),
        "grok" | "grok-fast" => Some("xai/grok-4-fast-reasoning"),
        // DeepSeek V4.1 Flash thinks by default, so `reasoner` lands there too;
        // the retired `deepseek-chat` / `deepseek-reasoner` names are only
        // reachable by their canonical id.
        "deepseek" | "ds" => Some("deepseek/deepseek-flash"),
        "deepseek-r" | "reasoner" => Some("deepseek/deepseek-flash"),
        "deepseek-pro" | "ds-pro" => Some("deepseek/deepseek-v4-pro"),
        // Must agree with Profile::Free's Simple target (the free-tier entry
        // point) — see `free_and_oss_aliases_resolve_to_free_nemotron`.
        "free" | "oss" => Some("nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1"),
        "o3-mini" | "o3mini" => Some("openai/o3-mini"),
        "o4-mini" | "o4mini" => Some("openai/o4-mini"),
        "gpt4.1" | "gpt-4.1" | "gpt41" => Some("openai/gpt-4.1"),
        "gpt4.1-mini" | "gpt-4.1-mini" => Some("openai/gpt-4.1-mini"),
        "gpt4.1-nano" | "gpt-4.1-nano" => Some("openai/gpt-4.1-nano"),
        "sonnet4.5" | "sonnet-4.5" => Some("anthropic/claude-sonnet-4-5-20250929"),
        "grok3" | "grok-3" => Some("xai/grok-3"),
        "grok3-mini" | "grok-3-mini" => Some("xai/grok-3-mini"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_boundaries() {
        assert_eq!(Tier::from_score(-0.5), Tier::Simple);
        assert_eq!(Tier::from_score(-0.01), Tier::Simple);
        assert_eq!(Tier::from_score(0.0), Tier::Medium);
        assert_eq!(Tier::from_score(0.19), Tier::Medium);
        assert_eq!(Tier::from_score(0.2), Tier::Complex);
        assert_eq!(Tier::from_score(0.39), Tier::Complex);
        assert_eq!(Tier::from_score(0.4), Tier::Reasoning);
        assert_eq!(Tier::from_score(1.0), Tier::Reasoning);
    }

    #[test]
    fn test_profile_from_alias() {
        assert_eq!(Profile::from_alias("eco"), Some(Profile::Eco));
        assert_eq!(Profile::from_alias("AUTO"), Some(Profile::Auto));
        assert_eq!(Profile::from_alias("premium"), Some(Profile::Premium));
        assert_eq!(Profile::from_alias("free"), Some(Profile::Free));
        assert_eq!(Profile::from_alias("unknown"), None);
    }

    #[test]
    fn test_resolve_model() {
        assert_eq!(
            resolve_model(Profile::Free, Tier::Reasoning),
            "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1"
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Reasoning),
            "anthropic/claude-fable-5-2"
        );
        assert_eq!(
            resolve_model(Profile::Auto, Tier::Simple),
            "google/gemini-2.5-flash"
        );
    }

    /// Free tier routes to NVIDIA NIM free Nemotron models, tiered by
    /// complexity (nano → super → ultra). Replaces the prior all-tiers→Gemini
    /// mapping (2026-06 repoint; cf BlockRun, where NVIDIA *is* the free tier).
    /// `every_alias_and_profile_tier_resolves_to_a_registered_model` guarantees
    /// these are registered; `free_profile_targets_are_all_zero_cost` guarantees
    /// they bill $0; the gateway's `model_fallback_chain` falls them back to
    /// Gemini if NIM is unavailable.
    #[test]
    fn free_profile_tiers_to_nvidia_nemotron() {
        assert_eq!(
            resolve_model(Profile::Free, Tier::Simple),
            "nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1"
        );
        assert_eq!(
            resolve_model(Profile::Free, Tier::Medium),
            "nvidia/nvidia/llama-3.3-nemotron-super-49b-v1"
        );
        assert_eq!(
            resolve_model(Profile::Free, Tier::Complex),
            "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1"
        );
        assert_eq!(
            resolve_model(Profile::Free, Tier::Reasoning),
            "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1"
        );
        // Every Free target is an nvidia-provider model.
        for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
            assert!(
                resolve_model(Profile::Free, tier).starts_with("nvidia/"),
                "Profile::Free tier {tier:?} must route to an NVIDIA free model"
            );
        }
    }

    /// The `free` / `oss` aliases resolve to the same canonical free-tier key
    /// as `Profile::Free`'s Simple tier (the free-tier entry point). Guards
    /// against the alias and the profile drifting apart (which would silently
    /// route alias users to a different model).
    #[test]
    fn free_and_oss_aliases_resolve_to_free_nemotron() {
        assert_eq!(
            resolve_alias("free"),
            Some("nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1")
        );
        assert_eq!(
            resolve_alias("oss"),
            Some("nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1")
        );
        // Alias and profile must agree on the free-tier entry point.
        assert_eq!(
            resolve_alias("free"),
            Some(resolve_model(Profile::Free, Tier::Simple))
        );
    }

    /// Every Free-tier model key MUST exist in the production `config/models.toml`
    /// registry. A repoint that named an unregistered model would 500 every
    /// free-tier request at the registry lookup (the same class of bug the
    /// `haiku` alias previously had).
    #[test]
    fn free_tier_models_are_registered_in_models_toml() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");
        for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
            let target = resolve_model(Profile::Free, tier);
            assert!(
                registry.get(target).is_some(),
                "free-tier model {target:?} (Profile::Free {tier:?}) must be in models.toml"
            );
        }
    }

    /// MONEY-PATH GUARD: every `Profile::Free` target must be a $0/$0 model so
    /// free-tier callers are never charged USDC. Without this, repointing Free
    /// at a *paid* NVIDIA model (e.g. the paid Nemotron Super v1.5 at $0.10/$0.40
    /// or Nano 9B v2 at $0.04/$0.16) would silently start billing "free" users.
    #[test]
    fn free_profile_targets_are_all_zero_cost() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");
        for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
            let target = resolve_model(Profile::Free, tier);
            let m = registry
                .get(target)
                .unwrap_or_else(|| panic!("free-tier target {target:?} not registered"));
            assert_eq!(
                m.input_cost_per_million, 0.0,
                "Profile::Free {tier:?} → {target:?} must have $0 input cost \
                 (free-tier callers must never be charged)"
            );
            assert_eq!(
                m.output_cost_per_million, 0.0,
                "Profile::Free {tier:?} → {target:?} must have $0 output cost"
            );
        }
    }

    /// The `opus` / `claude-opus` aliases and the Premium/Complex tier MUST
    /// point at the newest registered Opus (`claude-opus-5-5`). Pins the
    /// 2026-11 repoint off 4.8 onto 5.5 (and the 2026-06 one before it) so a
    /// future drift back to an older (or unregistered) Opus is a test
    /// failure, not a silent regression.
    #[test]
    fn opus_alias_and_premium_complex_route_to_opus_5_5() {
        assert_eq!(resolve_alias("opus"), Some("anthropic/claude-opus-5-5"));
        assert_eq!(
            resolve_alias("claude-opus"),
            Some("anthropic/claude-opus-5-5")
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Complex),
            "anthropic/claude-opus-5-5"
        );
        // The alias and the Premium/Complex tier must agree on the Opus target.
        assert_eq!(
            resolve_alias("opus"),
            Some(resolve_model(Profile::Premium, Tier::Complex))
        );
    }

    /// Opus 4.8 / 4.7 MUST load from the production `config/models.toml`
    /// with the agreed Opus-4-tier pricing and their real 1M context window
    /// (`context_window` is also the billing cap on prompt tokens, so the
    /// previous 200K value under-billed prompts longer than that). A pricing
    /// typo in the TOML would otherwise mis-bill every Opus request (the
    /// platform fee is applied to these per-million rates).
    #[test]
    fn opus_4_8_and_4_7_load_with_correct_pricing() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");

        for id in ["anthropic/claude-opus-4-8", "anthropic/claude-opus-4-7"] {
            let m = registry
                .get(id)
                .unwrap_or_else(|| panic!("{id} must be registered in models.toml"));
            assert_eq!(m.input_cost_per_million, 5.00, "{id} input cost");
            assert_eq!(m.output_cost_per_million, 25.00, "{id} output cost");
            assert_eq!(m.context_window, 1_000_000, "{id} context window");
            assert!(m.reasoning, "{id} must be reasoning-capable");
        }
    }

    /// MONEY-PATH PIN: the Claude 5 generation loads from the production
    /// `config/models.toml` at the prices verified against Anthropic's pricing
    /// page on 2026-11-18, with the 1M context window and 128K output cap. A
    /// typo here mis-bills every Premium request.
    #[test]
    fn claude_5_entries_load_with_verified_pricing() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");
        let expected = [
            ("anthropic/claude-fable-5-2", 8.00, 40.00),
            ("anthropic/claude-fable-5-1", 10.00, 50.00),
            ("anthropic/claude-opus-5-5", 4.00, 20.00),
            ("anthropic/claude-opus-5", 5.00, 25.00),
            ("anthropic/claude-sonnet-5-5", 2.00, 10.00),
            ("anthropic/claude-sonnet-5", 2.00, 10.00),
            ("anthropic/claude-haiku-5-5", 0.10, 0.50),
        ];
        for (id, input, output) in expected {
            let m = registry
                .get(id)
                .unwrap_or_else(|| panic!("{id} must be registered in models.toml"));
            assert_eq!(m.input_cost_per_million, input, "{id} input cost");
            assert_eq!(m.output_cost_per_million, output, "{id} output cost");
            assert_eq!(m.context_window, 1_000_000, "{id} context window");
            assert_eq!(m.max_output_tokens, Some(128_000), "{id} max output");
            assert!(m.reasoning, "{id} must be reasoning-capable");
            assert!(m.supports_vision, "{id} supports image input");
            assert!(m.supports_tools, "{id} supports tool use");
        }
    }

    /// MONEY-PATH PIN: the OpenAI GPT-6 line / GPT-5.5 and the DeepSeek V4
    /// entries load from the production `config/models.toml` at the prices,
    /// 272K long-context tiers, context windows and output caps verified
    /// against the provider pages on 2026-11-18, and the three legacy DeepSeek
    /// names at the Flash peak reprice. Deleting a tier from one of these
    /// entries (which the SDK generators would not notice — they omit tier
    /// fields) or fat-fingering a rate fails here.
    #[test]
    fn openai_and_deepseek_entries_load_with_verified_pricing() {
        use solvela_protocol::LongContextPricing;
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");
        let openai_tier = |input, output| {
            Some(LongContextPricing {
                threshold_tokens: 272_000,
                input_cost_per_million: input,
                output_cost_per_million: output,
            })
        };
        // (id, base in, base out, tier, context_window, max_output_tokens)
        let expected = [
            (
                "openai/gpt-6.2-sol",
                2.00,
                10.00,
                openai_tier(4.00, 15.00),
                1_050_000,
                Some(128_000),
            ),
            (
                "openai/gpt-6.1-sol",
                2.00,
                10.00,
                openai_tier(4.00, 15.00),
                1_050_000,
                Some(128_000),
            ),
            (
                "openai/gpt-6-luna",
                0.10,
                0.50,
                openai_tier(0.20, 0.75),
                1_050_000,
                Some(128_000),
            ),
            (
                "openai/gpt-5.5",
                5.00,
                30.00,
                openai_tier(10.00, 45.00),
                1_050_000,
                Some(128_000),
            ),
            (
                "deepseek/deepseek-flash",
                0.30,
                1.20,
                None,
                1_048_576,
                Some(393_216),
            ),
            (
                "deepseek/deepseek-v4-pro",
                1.32,
                3.96,
                None,
                1_048_576,
                Some(393_216),
            ),
            ("deepseek/deepseek-chat", 0.30, 1.20, None, 1_048_576, None),
            (
                "deepseek/deepseek-reasoner",
                0.30,
                1.20,
                None,
                1_048_576,
                None,
            ),
            ("deepseek/deepseek-coder", 0.30, 1.20, None, 1_048_576, None),
        ];
        for (id, input, output, tier, context_window, max_output) in expected {
            let m = registry
                .get(id)
                .unwrap_or_else(|| panic!("{id} must be registered in models.toml"));
            assert_eq!(m.input_cost_per_million, input, "{id} input cost");
            assert_eq!(m.output_cost_per_million, output, "{id} output cost");
            assert_eq!(m.long_context, tier, "{id} long-context tier");
            assert_eq!(m.context_window, context_window, "{id} context window");
            assert_eq!(m.max_output_tokens, max_output, "{id} max output");
            if let Some(t) = tier {
                // Whole-request switch strictly above 272K, from the loaded entry.
                assert_eq!(
                    m.rates_for_prompt(272_000),
                    (input, output),
                    "{id} at threshold"
                );
                assert_eq!(
                    m.rates_for_prompt(272_001),
                    (t.input_cost_per_million, t.output_cost_per_million),
                    "{id} over threshold"
                );
            }
        }
        // Tool advertisement follows the model pages: Sol 6.2 / GPT-5.5 yes,
        // 6.1 Sol / Luna no (Responses-API-only / effort-none-only tool calling).
        assert!(registry.get("openai/gpt-6.2-sol").unwrap().supports_tools);
        assert!(registry.get("openai/gpt-5.5").unwrap().supports_tools);
        assert!(!registry.get("openai/gpt-6.1-sol").unwrap().supports_tools);
        assert!(!registry.get("openai/gpt-6-luna").unwrap().supports_tools);
        // Vision: Flash yes, Pro no (pricing page).
        assert!(
            registry
                .get("deepseek/deepseek-flash")
                .unwrap()
                .supports_vision
        );
        assert!(
            !registry
                .get("deepseek/deepseek-v4-pro")
                .unwrap()
                .supports_vision
        );
    }

    /// Haiku 5.5 is the one Claude model with a long-prompt tier: prompts over
    /// 100K tokens bill the whole request at $0.50 / $2.50. Pins the tier so
    /// the Premium/Simple target can never silently revert to a flat rate
    /// (which would under-bill every long prompt 5x).
    #[test]
    fn haiku_5_5_has_the_100k_long_context_tier() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");
        let m = registry.get("anthropic/claude-haiku-5-5").unwrap();
        let tier = m
            .long_context
            .expect("haiku 5.5 must carry a long-context tier");
        assert_eq!(tier.threshold_tokens, 100_000);
        assert_eq!(tier.input_cost_per_million, 0.50);
        assert_eq!(tier.output_cost_per_million, 2.50);
        assert_eq!(m.rates_for_prompt(100_000), (0.10, 0.50));
        assert_eq!(m.rates_for_prompt(100_001), (0.50, 2.50));
    }

    /// The Premium profile routes every tier to the Claude 5 generation and
    /// the bare family aliases agree with it, so `premium` users and `haiku`
    /// / `sonnet` / `opus` / `fable` alias users land on the same models.
    #[test]
    fn premium_profile_and_family_aliases_route_to_claude_5_generation() {
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Simple),
            "anthropic/claude-haiku-5-5"
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Medium),
            "anthropic/claude-sonnet-5-5"
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Complex),
            "anthropic/claude-opus-5-5"
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Reasoning),
            "anthropic/claude-fable-5-2"
        );
        assert_eq!(
            resolve_alias("haiku"),
            Some(resolve_model(Profile::Premium, Tier::Simple))
        );
        assert_eq!(
            resolve_alias("sonnet"),
            Some(resolve_model(Profile::Premium, Tier::Medium))
        );
        assert_eq!(
            resolve_alias("fable"),
            Some(resolve_model(Profile::Premium, Tier::Reasoning))
        );
        // The previous generation stays reachable by versioned alias.
        assert_eq!(resolve_alias("opus4.8"), Some("anthropic/claude-opus-4-8"));
        assert_eq!(
            resolve_alias("sonnet4.6"),
            Some("anthropic/claude-sonnet-4-6")
        );
        assert_eq!(
            resolve_alias("haiku4.5"),
            Some("anthropic/claude-haiku-4-5-20251001")
        );
    }

    /// Nothing auto-routes to a DeepSeek name DeepSeek no longer documents:
    /// the Eco profile and the `deepseek` / `reasoner` aliases use the
    /// documented `deepseek-flash` (the retired `deepseek-chat` /
    /// `deepseek-reasoner` names stay registered for explicit callers only).
    #[test]
    fn eco_profile_and_deepseek_aliases_use_documented_v4_names() {
        for tier in [Tier::Simple, Tier::Complex, Tier::Reasoning] {
            assert_eq!(
                resolve_model(Profile::Eco, tier),
                "deepseek/deepseek-flash",
                "Eco {tier:?}"
            );
        }
        assert_eq!(resolve_alias("deepseek"), Some("deepseek/deepseek-flash"));
        assert_eq!(resolve_alias("reasoner"), Some("deepseek/deepseek-flash"));
        assert_eq!(
            resolve_alias("deepseek-pro"),
            Some("deepseek/deepseek-v4-pro")
        );
        for (profile, tier) in [
            (Profile::Eco, Tier::Simple),
            (Profile::Eco, Tier::Medium),
            (Profile::Eco, Tier::Complex),
            (Profile::Eco, Tier::Reasoning),
            (Profile::Auto, Tier::Simple),
            (Profile::Auto, Tier::Medium),
            (Profile::Auto, Tier::Complex),
            (Profile::Auto, Tier::Reasoning),
            (Profile::Premium, Tier::Simple),
            (Profile::Premium, Tier::Medium),
            (Profile::Premium, Tier::Complex),
            (Profile::Premium, Tier::Reasoning),
        ] {
            let target = resolve_model(profile, tier);
            assert!(
                !matches!(
                    target,
                    "deepseek/deepseek-chat"
                        | "deepseek/deepseek-reasoner"
                        | "deepseek/deepseek-coder"
                ),
                "{profile:?}/{tier:?} must not auto-route to a retired DeepSeek name, got {target}"
            );
        }
    }

    #[test]
    fn test_resolve_alias() {
        assert_eq!(resolve_alias("gpt5"), Some("openai/gpt-5.2"));
        assert_eq!(resolve_alias("gpt6"), Some("openai/gpt-6.2-sol"));
        assert_eq!(resolve_alias("sonnet"), Some("anthropic/claude-sonnet-5-5"));
        assert_eq!(resolve_alias("opus"), Some("anthropic/claude-opus-5-5"));
        // R1 regression: this used to point at the unregistered
        // `claude-3-5-haiku-20241022`. The canonical key MUST match a
        // model registered in `config/models.toml`.
        assert_eq!(resolve_alias("haiku"), Some("anthropic/claude-haiku-5-5"));
        assert_eq!(resolve_alias("nonexistent"), None);
    }

    /// R1 regression guard: every public alias and every profile-tier
    /// target MUST resolve to a model registered in
    /// `config/models.toml`. The previous `haiku` alias quietly broke
    /// this invariant; this test makes the same class of bug a
    /// compile-then-test failure for any future drift.
    ///
    /// The test loads the *production* `models.toml` from the repo root
    /// (relative to this crate) so it stays in sync as the registry is
    /// edited.
    #[test]
    fn every_alias_and_profile_tier_resolves_to_a_registered_model() {
        let toml_str = include_str!("../../../config/models.toml");
        let registry = crate::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");

        // Every alias the public `resolve_alias` exposes.
        let aliases = [
            "gpt5",
            "gpt5.5",
            "gpt6",
            "gpt6.1",
            "luna",
            "fable",
            "sonnet",
            "opus",
            "haiku",
            "opus4.8",
            "sonnet4.6",
            "haiku4.5",
            "deepseek-pro",
            "gemini",
            "flash",
            "grok",
            "deepseek",
            "deepseek-r",
            "free",
            "o3-mini",
            "o4-mini",
            "gpt4.1",
            "gpt4.1-mini",
            "gpt4.1-nano",
            "sonnet4.5",
            "grok3",
            "grok3-mini",
        ];
        for alias in aliases {
            let canonical = resolve_alias(alias)
                .unwrap_or_else(|| panic!("alias {alias:?} returned None — should resolve"));
            assert!(
                registry.get(canonical).is_some(),
                "alias {alias:?} -> {canonical:?} is not in models.toml — \
                 the alias either has a typo, references a removed model, \
                 or was added before the corresponding model entry."
            );
        }

        // Every (profile, tier) combination.
        for profile in [Profile::Eco, Profile::Auto, Profile::Premium, Profile::Free] {
            for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
                let canonical = resolve_model(profile, tier);
                assert!(
                    registry.get(canonical).is_some(),
                    "profile {profile:?} tier {tier:?} -> {canonical:?} is not in models.toml"
                );
            }
        }
    }
}
