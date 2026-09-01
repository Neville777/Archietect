/// A brand-new, unrelated table for tracking experiment-executor dispatch
/// gaps. Minimal reproduction of TITAN's real
/// `crates/titan_api/src/experiment_runner.rs::record_executor_gap`.
async fn record_executor_gap() {
    let _ = sqlx::query(
        "CREATE TABLE IF NOT EXISTS executor_gaps (\
           method TEXT PRIMARY KEY, reason TEXT NOT NULL)")
        .execute(db)
        .await;
}
