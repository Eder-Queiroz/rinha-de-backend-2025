use actix_web::{App, HttpResponse, HttpServer, Responder, post, web};
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use redis::{Client, Commands};
use serde_json;

struct AppState {
    redis_client: Arc<Client>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}

#[post("/payments")]
async fn enqueue_payment(
    payment: web::Json<PaymentRequest>,
    state: web::Data<AppState>
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
            println!("Payment with ID {} queued successfully", payment_data.correlation_id);
            HttpResponse::Accepted().json(serde_json::json!({
                "status": "queued",
                "id": payment_data.correlation_id
            }))
        },
        Err(e) => {
            eprintln!("Redis error: {}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({
                "error": "Failed to queue payment"
            }))
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client = Arc::new(
        redis::Client::open(redis_url).expect("Failed to create Redis client")
    );

    let state = web::Data::new(AppState {
        redis_client,
    });

    println!("Starting payment server with Redis queue");

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(enqueue_payment)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
