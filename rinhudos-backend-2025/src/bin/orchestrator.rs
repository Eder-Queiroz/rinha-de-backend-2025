use std::{env, sync::Arc};
use redis::{AsyncCommands};

use tokio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Olá do orchestrator");
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let redis_client = Arc::new(
        redis::Client::open(redis_url).expect("Failed to create Redis client")
    );

    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("Failed to connect to Redis: {}", e);
            std::process::exit(1);
        }
    };


    loop {
        let payment = conn.brpop::<_, (String, String)>("payment_queue", 0.0).await?;

        let (queue_name, payment_data) = payment;
        println!("Processing payment from queue '{}': {}", queue_name, payment_data);

    }

    // This will never be reached, but required for the return type
    // Ok(())
}