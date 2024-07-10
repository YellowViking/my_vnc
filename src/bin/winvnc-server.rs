use clap::Parser;
use tracing::info;

use my_vnc;
use my_vnc::server::Args;
use my_vnc::{server, settings};

#[tokio::main(flavor = "multi_thread")]
#[tracing::instrument(level = "info")]
async fn main() {
    let args = Args::parse();
    println!(
        "init logger for server Cargo version: {}",
        env!("CARGO_PKG_VERSION")
    );
    settings::init_logger();
    info!("args: {:?}", args);

    let bind = format!("{}:{}", args.host, args.port);
    server::main_args(args, bind).await;
}
