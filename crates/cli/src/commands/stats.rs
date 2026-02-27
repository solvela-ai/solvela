use anyhow::Result;

pub async fn show(_api_url: &str, days: u32) -> Result<()> {
    println!("Usage statistics (last {} days):", days);
    println!("\nNote: Usage tracking requires a running gateway with PostgreSQL.");
    println!("Stats endpoint not yet implemented — coming in Phase 5.");
    Ok(())
}
