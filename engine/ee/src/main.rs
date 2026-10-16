#[tokio::main]
async fn main() {
    let (auth, routes) = cadri_ee::from_env().unwrap_or_else(|err| {
        eprintln!("cadri-engine-ee: {err}");
        std::process::exit(1);
    });
    cadri_core::serve(auth, Some(routes)).await;
}
