/// A brand-new, unrelated table for tracking job-dispatch backlog entries.
/// Minimal reproduction of a real collision found live:
/// `crates/payment_api/src/job_runner.rs::record_executor_backlog`.
async fn record_executor_backlog() {
    let _ = sqlx::query(
        "CREATE TABLE IF NOT EXISTS executor_backlog (\
           method TEXT PRIMARY KEY, reason TEXT NOT NULL)")
        .execute(db)
        .await;
}
