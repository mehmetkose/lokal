//! `echo-server --bind <ip> --port <p> --marker <m>`: prints `LISTENING <addr>`.

use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    bind: std::net::IpAddr,
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long, default_value = "echo")]
    marker: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    let l = tokio::net::TcpListener::bind((a.bind, a.port)).await?;
    println!("LISTENING {}", l.local_addr()?);
    use std::io::Write;
    std::io::stdout().flush()?;
    testkit::echo::serve(l, a.marker).await;
    Ok(())
}
