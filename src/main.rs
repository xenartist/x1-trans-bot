mod config;
mod transfer;
mod wallet;
mod rpc_manager;

use crate::config::Config;
use crate::transfer::TransferService;
use anyhow::Result;
use clap::Parser;

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
        println!("Master wallet public key: {}", config.master_pubkey.unwrap());
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
    if let Some(ref target) = config.target_pubkey {
        println!("Note: target_pubkey is set to {}, but will be ignored", target);
    }
    
    // Print configuration information
    println!("Concurrency: {}", config.concurrency);
    println!("Amount per transfer: {} SOL", config.amount);
    println!("Each wallet will send transactions to itself for maximum concurrency");
    
    // Create transfer service
    let transfer_service = TransferService::new(config)?;
    
    // Start transfers
    transfer_service.start_transfers().await?;
    
    Ok(())
}
