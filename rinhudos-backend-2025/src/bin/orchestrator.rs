use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};

use tokio::{self, sync::RwLock};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct HealthCheckResponse {
    failing: bool,
    min_response_time: u64,
}

#[derive(Debug, Clone)]
struct HealthCheckCache {
    failing: bool,
    min_response_time: u64,
    timestamp: i64,
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

    let health_check_cache = Arc::new(RwLock::new(HealthCheckCache {
        failing: false,
        min_response_time: 0,
        timestamp: 0,
    }));

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

    get_health_status(Arc::clone(&health_check_cache)).await?;

    println!(
        "Received health_check: {:?}",
        health_check_cache.read().await
    );

    let (queue_name, payment_data) = payment;
    println!(
        "[Thread - {:?}] Processing payment from queue '{}': {}",
        thread_id, queue_name, payment_data
    );

    Ok(())
}

async fn get_health_status(
    health_check_cache: Arc<RwLock<HealthCheckCache>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Utc::now().timestamp();

    {
        let cache = health_check_cache.read().await;
        if cache.timestamp > 0 && now - cache.timestamp < 5 {
            println!("Using cached health check: {:?}", *cache);
            return Ok(());
        } else {
            println!("Cache expired or not available, fetching new health check");
        }
    }

    let mut cache = health_check_cache.write().await;
    let response = reqwest::get("http://localhost:8001/payments/service-health")
        .await?
        .text()
        .await;

    let health_check_response = match response {
        Ok(text) => text,
        Err(e) => {
            eprintln!("Failed to fetch health check: {}", e);
            return Err(Box::new(e));
        }
    };

    let health_check: HealthCheckResponse = serde_json::from_str(&health_check_response)?;

    let cache_info = HealthCheckCache {
        failing: health_check.failing,
        min_response_time: health_check.min_response_time,
        timestamp: now,
    };

    *cache = cache_info.clone();
    println!("Health check updated: {:?}", cache);

    Ok(())
}
