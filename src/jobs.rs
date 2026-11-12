use crate::state::AppState;
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub struct Job {
    pub kind: String,
    pub payload: Value,
}
pub fn enqueue(state: &AppState, kind: &str, payload: Value) {
    if state
        .jobs
        .send(Job {
            kind: kind.to_owned(),
            payload,
        })
        .is_err()
    {
        tracing::warn!(kind, "background worker unavailable");
    }
}
pub fn start(state: AppState, mut receiver: mpsc::UnboundedReceiver<Job>) {
    tokio::spawn(async move {
        while let Some(job) = receiver.recv().await {
            if job.kind == "email" {
                let payload = &job.payload;
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("customer");
                let message = payload.get("message").and_then(Value::as_str).unwrap_or("");
                let body = format!("Hello {name},\n\n{message}\n\nDetails: {payload}");
                state.outbox.write().await.push(json!({
                    "to":payload.get("to"),"subject":payload.get("subject"),"body":body,
                    "queued_at":chrono::Utc::now().timestamp_millis() as f64 / 1000.0
                }));
            } else if job.kind == "notify" {
                if let Some(url) = job.payload.get("url").and_then(Value::as_str) {
                    let body = job.payload.get("body").cloned().unwrap_or(Value::Null);
                    if let Err(error) =
                        crate::services::notifier::send(&state.http, url, &body).await
                    {
                        tracing::warn!(%error, "notification delivery failed");
                    }
                }
            }
        }
    });
}
