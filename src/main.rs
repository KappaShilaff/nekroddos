use anyhow::Result;
use nekroddos::run_test;

#[tokio::main]
async fn main() -> Result<()> {
    run_test().await
}
