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
        // ECO: cheapest capable model per tier
        (Profile::Eco, Tier::Simple) => "deepseek/deepseek-chat",
        (Profile::Eco, Tier::Medium) => "google/gemini-2.5-flash-lite",
        (Profile::Eco, Tier::Complex) => "deepseek/deepseek-chat",
        (Profile::Eco, Tier::Reasoning) => "deepseek/deepseek-reasoner",

        // AUTO: balanced cost/quality
        (Profile::Auto, Tier::Simple) => "google/gemini-2.5-flash",
        (Profile::Auto, Tier::Medium) => "xai/grok-code-fast-1",
        (Profile::Auto, Tier::Complex) => "google/gemini-3.1-pro",
        (Profile::Auto, Tier::Reasoning) => "xai/grok-4-fast-reasoning",

        // PREMIUM: best quality regardless of cost
        (Profile::Premium, Tier::Simple) => "openai/gpt-4o",
        (Profile::Premium, Tier::Medium) => "anthropic/claude-sonnet-4-6",
        (Profile::Premium, Tier::Complex) => "anthropic/claude-opus-4-8",
        (Profile::Premium, Tier::Reasoning) => "openai/o3",

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
        "sonnet" | "claude-sonnet" => Some("anthropic/claude-sonnet-4-6"),
        "opus" | "claude-opus" => Some("anthropic/claude-opus-4-8"),
        // The canonical key here MUST match one registered by `from_toml`
        // — see the cross-check test below. Previously this pointed at
        // `claude-3-5-haiku-20241022`, which has never been in
        // `config/models.toml`, so every request using the `haiku`
        // shorthand returned a 500 (registry lookup miss).
        "haiku" | "claude-haiku" => Some("anthropic/claude-haiku-4-5-20251001"),
        "gemini" | "gemini-pro" => Some("google/gemini-3.1-pro"),
        "flash" | "gemini-flash" => Some("google/gemini-2.5-flash"),
        "grok" | "grok-fast" => Some("xai/grok-4-fast-reasoning"),
        "deepseek" | "ds" => Some("deepseek/deepseek-chat"),
        "deepseek-r" | "reasoner" => Some("deepseek/deepseek-reasoner"),
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
            "openai/o3"
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
    /// point at the newest registered Opus (`claude-opus-4-8`). Pins the
    /// 2026-06 repoint off 4.6 onto 4.8 so a future drift back to an older
    /// (or unregistered) Opus is a test failure, not a silent regression.
    #[test]
    fn opus_alias_and_premium_complex_route_to_opus_4_8() {
        assert_eq!(resolve_alias("opus"), Some("anthropic/claude-opus-4-8"));
        assert_eq!(
            resolve_alias("claude-opus"),
            Some("anthropic/claude-opus-4-8")
        );
        assert_eq!(
            resolve_model(Profile::Premium, Tier::Complex),
            "anthropic/claude-opus-4-8"
        );
        // The alias and the Premium/Complex tier must agree on the Opus target.
        assert_eq!(
            resolve_alias("opus"),
            Some(resolve_model(Profile::Premium, Tier::Complex))
        );
    }

    /// The two newest Opus entries MUST load from the production
    /// `config/models.toml` with the agreed Opus-tier pricing and the repo's
    /// uniform 200K context-window convention. A pricing typo in the TOML
    /// would otherwise mis-bill every Opus request (the 5% fee is applied to
    /// these per-million rates).
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
            assert_eq!(m.context_window, 200_000, "{id} context window");
            assert!(m.reasoning, "{id} must be reasoning-capable");
        }
    }

    #[test]
    fn test_resolve_alias() {
        assert_eq!(resolve_alias("gpt5"), Some("openai/gpt-5.2"));
        assert_eq!(resolve_alias("sonnet"), Some("anthropic/claude-sonnet-4-6"));
        assert_eq!(resolve_alias("opus"), Some("anthropic/claude-opus-4-8"));
        // R1 regression: this used to point at the unregistered
        // `claude-3-5-haiku-20241022`. The canonical key MUST match a
        // model registered in `config/models.toml`.
        assert_eq!(
            resolve_alias("haiku"),
            Some("anthropic/claude-haiku-4-5-20251001")
        );
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
            "sonnet",
            "opus",
            "haiku",
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
