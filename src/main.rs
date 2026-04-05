use anyhow::Result;
use do_it::start;

#[tokio::main]
async fn main() -> Result<()> {
    start::run().await
}
