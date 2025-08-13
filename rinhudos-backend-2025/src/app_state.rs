use actix_web::web::Bytes;
use reqwest::Client;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub sender: mpsc::Sender<Bytes>,
}
