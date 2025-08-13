use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}
