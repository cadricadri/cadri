#[tokio::main]
async fn main() {
    cadri_core::serve(cadri_core::auth::from_env(), None).await;
}
