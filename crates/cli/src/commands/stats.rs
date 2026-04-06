use anyhow::Result;

pub async fn show(_api_url: &str, days: u32) -> Result<()> {
    println!("Usage statistics (last {} days):", days);
    println!("\nNote: Usage tracking requires a running gateway with PostgreSQL.");
    println!("Stats endpoint not yet implemented — coming in Phase 5.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stats_show_stub() {
        let result = show("http://localhost:8402", 7).await;
        assert!(result.is_ok(), "stats stub should succeed");
    }

    #[tokio::test]
    async fn test_stats_show_custom_days() {
        let result = show("http://localhost:8402", 30).await;
        assert!(result.is_ok(), "stats stub should accept custom days");
    }
}
