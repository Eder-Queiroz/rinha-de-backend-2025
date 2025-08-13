use actix_web::{App, HttpResponse, HttpServer, Responder, post, web};
use reqwest::Client;
use std::io::Result;
use tokio::sync::mpsc;

use rinhudos_backend_2025::app_state::AppState;
use rinhudos_backend_2025::structs::PaymentRequest;
use rinhudos_backend_2025::worker;

#[post("/payments")]
async fn enqueue_payment(
    payment: web::Json<PaymentRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let payment_data = payment.into_inner();

    let json_bytes = match serde_json::to_vec(&payment_data) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Erro ao serializar PaymentRequest: {}", e);
            return HttpResponse::BadRequest().body("Invalid payment data");
        }
    };

    let payload = web::Bytes::from(json_bytes);

    if let Err(e) = state.sender.send(payload).await {
        eprintln!("Erro ao enviar para fila: {e}");
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::Accepted().finish()
}

#[actix_web::main]
async fn main() -> Result<()> {
    let reqwest_client = Client::new();
    let (sender, receiver) = mpsc::channel(10_000);

    let app_state = AppState {
        client: reqwest_client.clone(),
        sender,
    };

    let worker_state = app_state.clone();

    // Start worker in a separate task
    tokio::spawn(async move {
        worker::create_workers(receiver, worker_state, 4).await;
    });

    let app_data = web::Data::new(app_state);

    println!("Servidor iniciado em http://0.0.0.0:8080");

    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .service(enqueue_payment)
    })
    .workers(2)
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
