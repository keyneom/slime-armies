use clap::Parser;
use matchbox_server::SignalingServer;

/// Slime Armies Matchbox Signaling Server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Host address to bind to
    #[arg(short, long, default_value = "0.0.0.0")]
    host: String,

    /// Port to listen on
    #[arg(short, long, default_value = "3536")]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let addr = format!("{}:{}", args.host, args.port);

    println!("╔═══════════════════════════════════════════════════╗");
    println!("║     Slime Armies Matchbox Signaling Server        ║");
    println!("╠═══════════════════════════════════════════════════╣");
    println!("║ Listening on: ws://{}                   ║", addr);
    println!("║                                                   ║");
    println!("║ Room URL format:                                  ║");
    println!("║   ws://localhost:{}/ROOMCODE?next=2            ║", args.port);
    println!("╚═══════════════════════════════════════════════════╝");

    SignalingServer::new(addr, SignalingServer::default_configuration())
        .await
        .expect("Failed to start signaling server")
        .await;
}
