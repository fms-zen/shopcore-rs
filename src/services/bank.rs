use std::time::Duration;

use serde_json::{json, Value};

use crate::AppState;

pub async fn charge(state: &AppState, order_id: i32, amount: f64) -> Option<String> {
    let url = format!("{}/charge", state.config.bank_url.trim_end_matches('/'));
    // Stable across transport retries and later attempts after a failed response.
    let key = format!("shopcore-order-{order_id}");
    for _ in 0..3 {
        let response = state
            .http
            .post(&url)
            .header("Idempotency-Key", &key)
            .timeout(Duration::from_secs(2))
            .json(&json!({"order_id": order_id, "amount": amount}))
            .send()
            .await;
        if let Ok(response) = response {
            if response.status() == reqwest::StatusCode::OK {
                if let Ok(data) = response.json::<Value>().await {
                    if let Some(reference) =
                        data["bank_ref"].as_str().filter(|value| !value.is_empty())
                    {
                        return Some(reference.to_owned());
                    }
                }
            }
        }
    }
    None
}
