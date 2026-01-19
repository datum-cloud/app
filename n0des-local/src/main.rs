use n0_error::StdResultExt;

#[tokio::main]
async fn main() -> n0_error::Result<()> {
    tracing_subscriber::fmt::init();
    let (api_secret, router) = n0des_local::bind_and_start().await?;
    println!("n0des endpoint listening at {}", router.endpoint().id());
    println!("export N0DES_API_SECRET='{}'", api_secret);
    tokio::signal::ctrl_c().await?;
    router.shutdown().await.anyerr()?;
    Ok(())
}
