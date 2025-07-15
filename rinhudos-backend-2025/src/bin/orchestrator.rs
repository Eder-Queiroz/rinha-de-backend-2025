use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};
use anyhow::Result;

use tokio::{self, sync::RwLock};

const DEFAULT_BASE_URL: &str = "http://localhost:8001";
const FALLBACK_BASE_URL: &str = "http://localhost:8002";
const RESPONSE_TIME_THRESHOLD_MS: u64 = 1000;
const QUEUE_SIZE_THRESHOLD: usize = 30;
const CONSECUTIVE_FAILURES_THRESHOLD: u16 = 3;
const CIRCUIT_CHECK_INTERVAL_SEC: i64 = 30;

#[derive(Serialize, Deserialize, Debug)]
struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct HealthCheckResponse {
    failing: bool,
    min_response_time: u64,
}

#[derive(Debug, Clone)]
struct PaymentProcessorState {
    failing: bool,
    min_response_time: u64,
    timestamp: i64,
    consecutive_failures: u16,
    circuit_open: bool,
    last_circuit_check: i64,
}

#[derive(Debug, Clone)]
struct HealthCheckCache {
    default: PaymentProcessorState,
    fallback: PaymentProcessorState,
}

impl HealthCheckCache {
    fn new() -> Self {
        Self { default: PaymentProcessorState { 
            failing: false,
            min_response_time: 0,
            timestamp: 0,
            consecutive_failures: 0,
            circuit_open: false,
            last_circuit_check: 0,
         }, fallback: PaymentProcessorState { 
            failing: false,
            min_response_time: 0,
            timestamp: 0,
            consecutive_failures: 0,
            circuit_open: false,
            last_circuit_check: 0,
          } }
    }

    fn get_state(&self, processor: &Processor) -> &PaymentProcessorState {
        match processor {
            Processor::Default => &self.default,
            Processor::Fallback => &self.fallback,
        }
    }

    fn get_state_mut(&mut self, processor: &Processor) -> &mut PaymentProcessorState {
        match processor {
            Processor::Default => &mut self.default,
            Processor::Fallback => &mut self.fallback,
        }
    }
}

#[derive(Debug, Clone)]
enum Processor {
    Default,
    Fallback,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client =
        Arc::new(redis::Client::open(redis_url).expect("Failed to create Redis client"));

    let conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Failed to connect to Redis: {}", e);
            std::process::exit(1);
        }
    };

    let health_check_cache = Arc::new(RwLock::new(HealthCheckCache::new()));

    let mut handles = vec![];

    for task_id in 0..4 {
        let health_check_cache_clone = Arc::clone(&health_check_cache);
        let mut conn_clone = conn.clone();

        let handle = tokio::spawn(async move {
            loop {
                if let Err(e) =
                    process_payment(task_id, &mut conn_clone, health_check_cache_clone.clone())
                        .await
                {
                    eprintln!("Error processing payment: {}", e);
                }
            }
        });

        println!(
            "Spawned worker thread for processing payments: {:?}",
            handle.id()
        );

        handles.push(handle);
    }

    futures::future::join_all(handles).await;

    Ok(())
}

async fn process_payment(
    thread_id: usize,
    conn: &mut redis::aio::MultiplexedConnection,
    health_check_cache: Arc<RwLock<HealthCheckCache>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let payment = conn
        .brpop::<_, (String, String)>("payment_queue", 0.0)
        .await?;

    update_health_status(Arc::clone(&health_check_cache)).await?;

    println!(
        "Received health_check: {:?}",
        health_check_cache.read().await
    );

    let queue_size = conn.llen("payment_queue").await.unwrap_or(0);

    println!( "Queue size: {}, Thread ID: {:?}", queue_size, thread_id);

    let processor = choose_payment_processor(
        Arc::clone(&health_check_cache),
        queue_size,
    ).await;
    
    let (queue_name, payment_data) = payment;
    let payment_request: PaymentRequest = serde_json::from_str(&payment_data)?;
    println!(
        "[Thread - {:?}] Processing payment from queue '{}': {}",
        thread_id, queue_name, payment_data
    );

    match send_payment_to_processor(&payment_request, &processor).await {
        Ok(_) => {
            reset_failure_count(Arc::clone(&health_check_cache), &processor).await;
            println!("✅ Payment {} processed successfully by {:#?}", payment_request.correlation_id, processor);
        }
        Err(e) => {
            println!("❌ Failed to process payment {:#?}: {}", processor, e);

            increment_failure_count(Arc::clone(&health_check_cache), &processor).await;

            let fallback_processor = match processor {
                Processor::Default => Processor::Fallback,
                Processor::Fallback => Processor::Default,
            };

            let can_try_fallback = {
                let cache = health_check_cache.read().await;
                let fallback_state = cache.get_state(&fallback_processor);

                !fallback_state.circuit_open
            };

            if can_try_fallback {
                println!("🔄 Retrying send payment processor: {:#?}", fallback_processor);
                match send_payment_to_processor(&payment_request, &fallback_processor).await {
                    Ok(_) => {
                        reset_failure_count(Arc::clone(&health_check_cache), &fallback_processor).await;
                        println!("✅ Payment {} processed successfully by {:#?}", payment_request.correlation_id, fallback_processor);
                    }
                    Err(second_error) => {
                        println!("❌ Failed to process payment {:#?} with fallback: {}", fallback_processor, second_error);
                        increment_failure_count(Arc::clone(&health_check_cache), &fallback_processor).await;
                        println!("❌ Payment {} failed with both processors", payment_request.correlation_id);
                    }
                }
            } else {
                println!("⚠️ Cannot try fallback processor - circuit is open");
                println!("💀 Payment {} could not be processed", payment_request.correlation_id);
            }
            
        }
    }

    Ok(())
}

async fn update_health_status(
    health_check_cache: Arc<RwLock<HealthCheckCache>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().timestamp();

    {
        let cache = health_check_cache.read().await;
        if cache.default.timestamp > 0 && now - cache.default.timestamp < 5 {
            println!("Using cached health check: {:?}", *cache);
            return Ok(());
        } else {
            println!("Cache expired or not available, fetching new health check");
        }
    }

    println!("🔍 Checking health status...");

    let default_health_url = format!("{}/payments/service-health", DEFAULT_BASE_URL);
    let fallback_health_url = format!("{}/payments/service-health", FALLBACK_BASE_URL);

    let default_health = get_processor_health(&default_health_url).await;
    let fallback_health = get_processor_health(&fallback_health_url).await;

    {
        let mut cache = health_check_cache.write().await;
        let current_timestamp = chrono::Utc::now().timestamp();

        if let Ok(health) = default_health {
            cache.default.failing = health.failing;
            cache.default.min_response_time = health.min_response_time;
            cache.default.timestamp = current_timestamp;
            println!("📊 Default health: failing={}, time={}ms", health.failing, health.min_response_time);
        }

        if let Ok(health) = fallback_health {
            cache.fallback.failing = health.failing;
            cache.fallback.min_response_time = health.min_response_time;
            cache.fallback.timestamp = current_timestamp;
            println!("📊 Fallback health: failing={}, time={}ms", health.failing, health.min_response_time);
        }
    }

    Ok(())
}

async fn get_processor_health(url: &str) -> Result<HealthCheckResponse> {
    let client = reqwest::Client::new();
    let response = client
    .get(url)
    .timeout(std::time::Duration::from_secs(5))
    .send()
    .await?;

    if response.status().is_success() {
        let health_response = response.text().await?;
        let health: HealthCheckResponse = serde_json::from_str(&health_response)?;
        Ok(health)
    } else {
        Err(anyhow::anyhow!("Health check failed with status: {}", response.status()).into())
    }
}

async fn choose_payment_processor(
    health_cache: Arc<RwLock<HealthCheckCache>>,
    queue_size: usize,
) -> Processor {
    let cache = health_cache.read().await;

    if cache.default.circuit_open {
        drop(cache);

        if can_close_circuit(Arc::clone(&health_cache), &Processor::Default).await {
            println!("🟡 Testing default processor (HALF-OPEN)");
            return Processor::Default;
        }

        println!("🔴 Default circuit is open, using fallback");
        return Processor::Fallback;
    }

    let cache = health_cache.read().await;

    if cache.default.failing {
        println!("⚠️ Default is failing, using fallback");
        return Processor::Fallback;
    }

    if cache.default.min_response_time > RESPONSE_TIME_THRESHOLD_MS && queue_size > QUEUE_SIZE_THRESHOLD {
        println!("🐌 Default is slow ({} ms) and queue is big ({}), using fallback", cache.default.min_response_time, queue_size);
        return Processor::Fallback;
    }

    println!("✅ Using default processor");
    Processor::Default

}

async fn send_payment_to_processor(
    payment: &PaymentRequest,
    processor: &Processor,
) -> Result<()> {

    let url = match processor {
        Processor::Default => format!("{}/payments/process", DEFAULT_BASE_URL),
        Processor::Fallback => format!("{}/payments/process", FALLBACK_BASE_URL),
    };

    let client = reqwest::Client::new();

    let payload = serde_json::json!({
        "correlationId": payment.correlation_id,
        "amount": payment.amount,
        "requestedAt": chrono::Utc::now().to_rfc3339(),
    });

    println!("📤 Sending payment {} to {:#?}", payment.correlation_id, processor);

    let response = client
        .post(url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await?;

    if response.status().is_success() {
        println!("✅ Payment {} processed successfully by {:#?}", payment.correlation_id, processor);
        Ok(())
    }else {
        let status = response.status();
        println!("❌ Failed to process payment {}: {}", payment.correlation_id, status);
        let error_message = format!("Failed to process payment {}: {}", payment.correlation_id, status);
        Err(anyhow::anyhow!(error_message).into())
    }

}

async fn increment_failure_count(
    health_cache: Arc<RwLock<HealthCheckCache>>,
    processor: &Processor,
) {
    let mut cache = health_cache.write().await;

    let state = cache.get_state_mut(processor);

    state.consecutive_failures += 1;

    if state.consecutive_failures >= CONSECUTIVE_FAILURES_THRESHOLD {
        state.circuit_open = true;
        state.last_circuit_check = chrono::Utc::now().timestamp();
        println!("🔴 Circuit breaker opened for {:#?} (failed {} times)", processor, state.consecutive_failures);
    }

}

async fn reset_failure_count(
    health_cache: Arc<RwLock<HealthCheckCache>>,
    processor: &Processor,
) {
    let mut cache = health_cache.write().await;

    let state = cache.get_state_mut(processor);

    if state.consecutive_failures > 0 {
        println!("✅ Resetting failure count for {:#?} (was {})", processor, state.consecutive_failures);
        state.consecutive_failures = 0;
    }

    if state.circuit_open {
        println!("🔵 Resetting circuit breaker for {:#?}", processor);
        state.circuit_open = false;
    }
}

async fn can_close_circuit(
    health_cache: Arc<RwLock<HealthCheckCache>>,
    processor: &Processor,
) -> bool {
    let mut cache = health_cache.write().await;
    let state = cache.get_state_mut(processor);

    if !state.circuit_open {
        println!("✅ Circuit is already closed for {:#?}", processor);
        return false;
    }

    let now = chrono::Utc::now().timestamp();
    let elapsed = now - state.last_circuit_check;

    if elapsed > CIRCUIT_CHECK_INTERVAL_SEC {
        state.last_circuit_check = now;
        println!("🔄 Checking if circuit can be closed for {:#?}", processor);
        return true;
    }

    false
}