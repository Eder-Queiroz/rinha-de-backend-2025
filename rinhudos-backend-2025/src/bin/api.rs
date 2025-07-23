use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use chrono::{DateTime, NaiveDateTime, Utc};
use redis::{Client, Commands};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json;
use serde_with::{serde_as, DisplayFromStr};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;

struct AppState {
    redis_client: Arc<Client>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}

#[derive(Deserialize, Serialize)]
struct PaymentRequested {
    correlation_id: String,
    amount: f64,
    requested_at: String,
}

#[post("/payments")]
async fn enqueue_payment(
    payment: web::Json<PaymentRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let payment_data = payment.into_inner();

    let payment_json = match serde_json::to_string(&payment_data) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Failed to serialize payment: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to process payment"
            }));
        }
    };

    let mut conn = match state.redis_client.get_connection() {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Failed to connect to Redis: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Queue system unavailable"
            }));
        }
    };

    match conn.lpush::<_, _, ()>("payment_queue", payment_json) {
        Ok(_) => {
            println!(
                "Payment with ID {} queued successfully",
                payment_data.correlation_id
            );
            HttpResponse::Accepted().json(serde_json::json!({
                "status": "queued",
                "id": payment_data.correlation_id
            }))
        }
        Err(e) => {
            eprintln!("Redis error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to queue payment"
            }))
        }
    }
}

fn from_naive_utc<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        Some(s) => {
            let naive = NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S")
                .map_err(serde::de::Error::custom)?;
            Ok(Some(DateTime::<Utc>::from_utc(naive, Utc)))
        }
        None => Ok(None),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    total_requests: u64,
    total_amount: f64,
}

#[serde_as]
#[derive(Deserialize)]
struct SummaryParams {
    #[serde_as(as = "Option<DisplayFromStr>")]
    from: Option<DateTime<Utc>>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    to: Option<DateTime<Utc>>,
}

#[get("/payments-summary")]
async fn payments_summary(
    state: web::Data<AppState>,
    query: web::Query<SummaryParams>,
) -> impl Responder {
    let mut conn = match state.redis_client.get_connection() {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Failed to connect to Redis: {}", e);
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Database unavailable"
            }));
        }
    };

    let summary_default: redis::RedisResult<std::collections::HashMap<String, String>> =
        conn.hgetall("payments_summary-Default");

    let summary_default_filtered = if let Err(e) = summary_default {
        eprintln!("Failed to fetch default summary: {}", e);
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to fetch summary"
        }));
    } else {
        let hashmap = summary_default.unwrap_or_default();

        filter_summary(hashmap, query.from, query.to)
    };

    let summary_fallback: redis::RedisResult<std::collections::HashMap<String, String>> =
        conn.hgetall("payments_summary-Fallback");

    let summary_fallback_filtered = if let Err(e) = summary_fallback {
        eprintln!("Failed to fetch fallback summary: {}", e);
        return HttpResponse::InternalServerError().json(serde_json::json!({
            "error": "Failed to fetch summary"
        }));
    } else {
        let hashmap = summary_fallback.unwrap_or_default();

        filter_summary(hashmap, query.from, query.to)
    };

    HttpResponse::Ok().json(serde_json::json!({
        "default": summary_default_filtered,
        "fallback": summary_fallback_filtered
    }))
}

fn filter_summary(
    summary: HashMap<String, String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Summary {
    let mut total_amount = 0.0;
    let mut total_requests = 0;

    for (_, value) in summary.iter() {
        let payment_requested: PaymentRequested = match serde_json::from_str(value) {
            Ok(payment) => payment,
            Err(e) => {
                eprintln!("Failed to deserialize payment: {}", e);
                continue;
            }
        };

        let requested_at = match DateTime::parse_from_rfc3339(&payment_requested.requested_at) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(e) => {
                eprintln!("Failed to parse date: {}", e);
                continue;
            }
        };

        if let Some(from_date) = from {
            if let Some(to_date) = to {
                if requested_at < from_date || requested_at > to_date {
                    continue;
                }
            } else if requested_at < from_date {
                continue;
            }
        } else if let Some(to_date) = to {
            if requested_at > to_date {
                continue;
            }
        }

        total_requests += 1;
        total_amount += payment_requested.amount;
    }

    Summary {
        total_requests,
        total_amount,
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client =
        Arc::new(redis::Client::open(redis_url).expect("Failed to create Redis client"));

    let state = web::Data::new(AppState { redis_client });

    println!("Starting payment server with Redis queue");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(enqueue_payment)
            .service(payments_summary)
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
