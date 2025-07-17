use serde::{Deserialize, Serialize};
use redis::{Client, Commands};
use std::env;
use std::time::Duration;

#[derive(Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct PaymentRequest {
    correlation_id: String,
    amount: f64,
}

fn main() {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

    println!("Starting payment consumer with Redis at {}", redis_url);

    // Connect to Redis
    let client = match Client::open(redis_url) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Failed to connect to Redis: {}", e);
            std::process::exit(1);
        }
    };

    // Main processing loop
    loop {
        let mut conn = match client.get_connection() {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("Failed to get Redis connection: {}", e);
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        // BRPOP blocks until an element is available
        println!("Waiting for payments...");
        match conn.brpop::<_, (String, String)>("payment_queue", 0) {
            Ok((_, payment_json)) => {
                println!("Received payment from queue");

                // Deserialize payment
                match serde_json::from_str::<PaymentRequest>(&payment_json) {
                    Ok(payment) => {
                        println!("Processing payment: id={}, amount=${}",
                                payment.correlation_id, payment.amount);

                        // Process payment (implement your payment logic here)
                        process_payment(&payment);
                    },
                    Err(e) => {
                        eprintln!("Failed to parse payment JSON: {}", e);
                    }
                }
            },
            Err(e) => {
                eprintln!("Redis error: {}", e);
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

fn process_payment(payment: &PaymentRequest) {
    // Here you would implement the actual payment processing logic
    // For example: call a payment gateway, update a database, etc.
    println!("Payment processed successfully: id={}, amount=${}",
             payment.correlation_id, payment.amount);
}
