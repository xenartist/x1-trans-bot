mod config;
mod wallet;
mod transfer;
mod rpc_manager;

use anyhow::Result;
use clap::Parser;
use crate::config::Config;
use crate::transfer::TransferService;

#[derive(Parser, Debug)]
#[clap(author, version, about = "Solana automatic transfer tool")]
struct Args {
    /// Path to config file
    #[clap(short, long, default_value = "config.json")]
    config: String,
    
    /// Generate default config file
    #[clap(short, long)]
    init: bool,
    
    /// Show master wallet address (useful for funding)
    #[clap(short, long)]
    show_wallet: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    if args.init {
        let config = Config::default();
        config.save(&args.config)?;
        println!("Default config generated at: {}", args.config);
        println!("Master wallet public key: {}", config.master_pubkey.unwrap_or_default());
        println!("Please fund this wallet before running transfers");
        return Ok(());
    }
    
    // Load config
    let config = Config::load(&args.config)?;
    
    if args.show_wallet {
        let master_pubkey = config.get_master_pubkey()?;
        println!("Master wallet public key: {}", master_pubkey);
        println!("Please fund this wallet before running transfers");
        return Ok(());
    }
    
    println!("Using RPC endpoints: {}", config.rpc_urls.join(", "));
    println!("Master wallet: {}", config.get_master_pubkey()?);
    println!("Target address: {}", config.target_pubkey);
    
    // Create transfer service
    let transfer_service = TransferService::new(config)?;
    
    // Test RPC endpoints
    println!("Testing RPC endpoints...");
    transfer_service.test_rpc_endpoints().await?;
    
    // Start transfers
    transfer_service.start_transfers().await?;
    
    Ok(())
}
