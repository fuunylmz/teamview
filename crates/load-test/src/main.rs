use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
#[command(author, version, about = "Synthetic TeamView relay load test scaffold")]
struct Args {
    #[arg(long, default_value_t = 1)]
    publishers: u16,

    #[arg(long, default_value_t = 10)]
    viewers: u16,

    #[arg(long, default_value_t = false)]
    include_slow_viewer: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    info!(?args, "load-test scaffold ready");
    println!(
        "load-test scaffold publishers={} viewers={} include_slow_viewer={}",
        args.publishers, args.viewers, args.include_slow_viewer
    );

    Ok(())
}
