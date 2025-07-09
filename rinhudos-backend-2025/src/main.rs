use actix_web::{App, HttpResponse, HttpServer, Responder, post, web};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}

#[post("/payments")]
async fn enqueue_payment(payment: web::Json<PaymentRequest>) -> impl Responder {
    HttpResponse::Ok().json(payment.into_inner())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| App::new().service(enqueue_payment))
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
