use actix_web::web::Bytes;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::{self, Duration, Instant};

use crate::{app_state::AppState, structs::PaymentRequest};

#[derive(Clone)]
pub struct PaymentProcessorService {
    state: AppState,
    notify: Arc<Notify>,
}

impl PaymentProcessorService {
    pub fn new(state: AppState) -> Self {
        PaymentProcessorService {
            state,
            notify: Arc::new(Notify::new()),
        }
    }

    async fn send_request(&self, payment_request: PaymentRequest) -> bool {
        let response = self
            .state
            .client
            .post("http://localhost:8001/payments")
            .json(&payment_request)
            .send();

        let timeout_result = tokio::time::timeout(Duration::from_millis(1_000), response).await;

        match timeout_result {
            Ok(Ok(resp)) if resp.status().is_success() => {
                println!("✅ Payment processed successfully");
                true
            }
            Ok(Ok(resp)) => {
                println!("❌ Payment failed with status: {}", resp.status());
                false
            }
            Ok(Err(e)) => {
                println!("❌ Payment request error: {}", e);
                false
            }
            Err(_) => {
                println!("⏳ Payment processing timed out");
                false
            }
        }
    }

    /// Inicia o worker mestre
    pub fn start_master(
        &self,
        mut rx_main: mpsc::Receiver<Bytes>,
        tx_retry: mpsc::Sender<Bytes>,
        trigger_ms: u128,
    ) {
        let notify = self.notify.clone();
        let service = self.clone();

        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(1));

            while let Some(bytes) = rx_main.recv().await {
                if let Ok(payment_request) = serde_json::from_slice::<PaymentRequest>(&bytes) {
                    let start = Instant::now();
                    let success = service.send_request(payment_request).await;
                    let duration = start.elapsed().as_millis();

                    if success && duration <= trigger_ms {
                        notify.notify_waiters();

                        while let Ok(bytes) = rx_main.try_recv() {
                            if let Ok(payment_request) =
                                serde_json::from_slice::<PaymentRequest>(&bytes)
                            {
                                let start = Instant::now();
                                let success = service.send_request(payment_request).await;
                                let duration = start.elapsed().as_millis();

                                if !success || duration > trigger_ms {
                                    let _ = tx_retry.send(bytes).await;
                                    break;
                                }
                            }
                        }
                    } else {
                        let _ = tx_retry.send(bytes).await;
                    }
                }
                interval.tick().await;
            }
        });
    }

    pub fn start_slaves(
        &self,
        rx_retry: Arc<Mutex<mpsc::Receiver<Bytes>>>,
        tx_retry: mpsc::Sender<Bytes>,
        worker_count: i16,
        trigger_ms: u128,
    ) {
        for id in 0..worker_count {
            let service = self.clone();
            let notify = self.notify.clone();
            let rx_retry = rx_retry.clone();
            let tx_retry = tx_retry.clone();

            tokio::spawn(async move {
                loop {
                    notify.notified().await;
                    let mut guard = rx_retry.lock().await;

                    while let Some(bytes) = guard.recv().await {
                        if let Ok(payment_request) =
                            serde_json::from_slice::<PaymentRequest>(&bytes)
                        {
                            let start = Instant::now();
                            let success = service.send_request(payment_request).await;
                            let duration = start.elapsed().as_millis();

                            if !success || duration > trigger_ms {
                                drop(guard);
                                let _ = tx_retry.send(bytes).await;
                                break;
                            }
                        }
                    }
                }
            });
        }
    }
}

/// Função que inicializa todo o sistema
pub async fn create_workers(rx_main: mpsc::Receiver<Bytes>, state: AppState, worker_count: i16) {
    let service = PaymentProcessorService::new(state);

    let (tx_retry, rx_retry) = mpsc::channel::<Bytes>(10_000);
    let rx_retry_arc = Arc::new(Mutex::new(rx_retry));

    let trigger_ms = 200;

    service.start_master(rx_main, tx_retry.clone(), trigger_ms);
    service.start_slaves(
        rx_retry_arc.clone(),
        tx_retry.clone(),
        worker_count,
        trigger_ms,
    );
}
